// Node-API (N-API) bridge to the vendored OpenNest `nfp_nest` C++ engine
// (native/opennest, MIT — see native/opennest/LICENSE and CREDITS.md).
//
// The JS side builds flat, typed-array inputs that map 1:1 onto the C ABI in
// `native/opennest/src/capi/nfp_nest_capi.h`; this addon only marshals memory and
// exposes the solve as an async Promise so the Node event loop stays responsive.
// `cancel()` / `progress()` / `fitness()` / `pollLayout()` call the engine's
// process-global control/snapshot functions while a solve is running.
//
// NOTE: the underlying engine keeps global solve state (cancel/progress/live
// snapshot), so only one `nest()` may run at a time process-wide. A second
// concurrent call rejects with an error; the application layer already limits
// concurrency to one job.
#include <napi.h>

#include <algorithm>
#include <atomic>
#include <cstring>
#include <string>
#include <utility>
#include <vector>

#include "capi/nfp_nest_capi.h"

namespace {

std::atomic<bool> g_busy{false};

std::vector<double> ReadDoubleArray(const Napi::Value& value) {
  if (value.IsEmpty() || value.IsNull() || value.IsUndefined()) {
    return {};
  }
  Napi::Float64Array array = value.As<Napi::Float64Array>();
  const size_t length = array.ElementLength();
  return std::vector<double>(array.Data(), array.Data() + length);
}

std::vector<int32_t> ReadIntArray(const Napi::Value& value) {
  if (value.IsEmpty() || value.IsNull() || value.IsUndefined()) {
    return {};
  }
  Napi::Int32Array array = value.As<Napi::Int32Array>();
  const size_t length = array.ElementLength();
  return std::vector<int32_t>(array.Data(), array.Data() + length);
}

int ParamInt(const Napi::Object& params, const char* key, int fallback) {
  if (!params.Has(key)) {
    return fallback;
  }
  Napi::Value value = params.Get(key);
  return value.IsNumber() ? value.As<Napi::Number>().Int32Value() : fallback;
}

double ParamDouble(const Napi::Object& params, const char* key, double fallback) {
  if (!params.Has(key)) {
    return fallback;
  }
  Napi::Value value = params.Get(key);
  return value.IsNumber() ? value.As<Napi::Number>().DoubleValue() : fallback;
}

NfpParams ReadParams(const Napi::Value& value) {
  NfpParams p{};
  Napi::Object params = value.As<Napi::Object>();
  p.placementType = ParamInt(params, "placementType", 1);
  p.rotations = ParamInt(params, "rotations", 4);
  p.mutationRate = ParamInt(params, "mutationRate", 10);
  p.populationSize = ParamInt(params, "populationSize", 10);
  p.seed = ParamInt(params, "seed", 1);
  p.curveTolerance = ParamDouble(params, "curveTolerance", 0.3);
  p.clipperScale = ParamDouble(params, "clipperScale", 1e7);
  p.spacing = ParamDouble(params, "spacing", 0.0);
  p.sheetSpacing = ParamDouble(params, "sheetSpacing", 0.0);
  p.rotationLimit = ParamDouble(params, "rotationLimit", 360.0);
  p.useHoles = ParamInt(params, "useHoles", 1);
  p.exploreConcave = ParamInt(params, "exploreConcave", 1);
  p.clipByHull = ParamInt(params, "clipByHull", 1);
  p.clipByRects = ParamInt(params, "clipByRects", 1);
  p.simplify = ParamInt(params, "simplify", 0);
  p.mode = ParamInt(params, "mode", 1);
  p.generations = ParamInt(params, "generations", 10);
  p.numSeeds = ParamInt(params, "numSeeds", 4);
  p.useParallel = ParamInt(params, "useParallel", 1);
  p.timeBudgetSecs = ParamDouble(params, "timeBudgetSecs", 0.0);
  p.maxSheets = ParamInt(params, "maxSheets", 0);
  p.edgeSamples = ParamInt(params, "edgeSamples", -1);
  p.compactionPasses = ParamInt(params, "compactionPasses", -1);
  p.tryAllRotations = ParamInt(params, "tryAllRotations", 0);
  p.exactNfp = ParamInt(params, "exactNfp", 0);
  p.stagnationGens = ParamInt(params, "stagnationGens", 0);
  p.exactVoids = ParamInt(params, "exactVoids", 0);
  return p;
}

// Flat copy of the parts/sheets input, decoupled from JS-owned memory before
// the background thread starts.
struct NestRequest {
  std::vector<int32_t> partVertexCounts;
  std::vector<double> partXY;
  std::vector<int32_t> partQuantities;
  std::vector<int32_t> partRotations;
  std::vector<int32_t> partHoleCounts;
  std::vector<int32_t> partHoleVertexCounts;
  std::vector<double> partHoleXY;

  std::vector<int32_t> sheetVertexCounts;
  std::vector<double> sheetXY;
  std::vector<int32_t> sheetHoleCounts;
  std::vector<int32_t> sheetHoleVertexCounts;
  std::vector<double> sheetHoleXY;

  bool hasPartRotations = false;
  NfpParams params{};
};

const int* IntData(const std::vector<int32_t>& v) {
  return v.empty() ? nullptr : v.data();
}
const double* DoubleData(const std::vector<double>& v) {
  return v.empty() ? nullptr : v.data();
}

class NestWorker : public Napi::AsyncWorker {
 public:
  NestWorker(Napi::Env env, Napi::Promise::Deferred deferred, NestRequest request)
      : Napi::AsyncWorker(env),
        deferred_(deferred),
        request_(std::move(request)) {}

  void Execute() override {
    int64_t instanceCount = 0;
    for (int32_t quantity : request_.partQuantities) {
      instanceCount += quantity > 0 ? quantity : 1;
    }
    instanceCount_ = instanceCount;

    tx_.assign(instanceCount_, 0.0);
    ty_.assign(instanceCount_, 0.0);
    angle_.assign(instanceCount_, 0.0);
    sheetId_.assign(instanceCount_, -1);
    partIndex_.assign(instanceCount_, -1);

    const int placed = nfp_nest(
        static_cast<int>(request_.partVertexCounts.size()),
        IntData(request_.partVertexCounts), DoubleData(request_.partXY),
        IntData(request_.partQuantities),
        request_.hasPartRotations ? IntData(request_.partRotations) : nullptr,
        IntData(request_.partHoleCounts), IntData(request_.partHoleVertexCounts),
        DoubleData(request_.partHoleXY),
        static_cast<int>(request_.sheetVertexCounts.size()),
        IntData(request_.sheetVertexCounts), DoubleData(request_.sheetXY),
        IntData(request_.sheetHoleCounts), IntData(request_.sheetHoleVertexCounts),
        DoubleData(request_.sheetHoleXY), &request_.params,
        instanceCount_ > 0 ? tx_.data() : nullptr,
        instanceCount_ > 0 ? ty_.data() : nullptr,
        instanceCount_ > 0 ? angle_.data() : nullptr,
        instanceCount_ > 0 ? sheetId_.data() : nullptr,
        instanceCount_ > 0 ? partIndex_.data() : nullptr, &nSheets_, &fitness_);

    if (placed < 0) {
      SetError("opennest nfp_nest() failed with error code " +
               std::to_string(placed));
      return;
    }
    placed_ = placed;
  }

  void OnOK() override {
    Napi::HandleScope scope(Env());
    g_busy.store(false);

    Napi::Object result = Napi::Object::New(Env());
    result.Set("placed", Napi::Number::New(Env(), placed_));
    result.Set("nSheets", Napi::Number::New(Env(), nSheets_));
    result.Set("fitness", Napi::Number::New(Env(), fitness_));
    result.Set("tx", ToFloat64Array(tx_));
    result.Set("ty", ToFloat64Array(ty_));
    result.Set("angle", ToFloat64Array(angle_));
    result.Set("sheetId", ToInt32Array(sheetId_));
    result.Set("partIndex", ToInt32Array(partIndex_));
    deferred_.Resolve(result);
  }

  void OnError(const Napi::Error& error) override {
    g_busy.store(false);
    deferred_.Reject(error.Value());
  }

 private:
  Napi::Float64Array ToFloat64Array(const std::vector<double>& values) {
    Napi::Float64Array array =
        Napi::Float64Array::New(Env(), values.size());
    if (!values.empty()) {
      std::memcpy(array.Data(), values.data(), values.size() * sizeof(double));
    }
    return array;
  }

  Napi::Int32Array ToInt32Array(const std::vector<int32_t>& values) {
    Napi::Int32Array array = Napi::Int32Array::New(Env(), values.size());
    if (!values.empty()) {
      std::memcpy(array.Data(), values.data(), values.size() * sizeof(int32_t));
    }
    return array;
  }

  Napi::Promise::Deferred deferred_;
  NestRequest request_;

  int64_t instanceCount_ = 0;
  int placed_ = 0;
  int nSheets_ = 0;
  double fitness_ = 0.0;
  std::vector<double> tx_;
  std::vector<double> ty_;
  std::vector<double> angle_;
  std::vector<int32_t> sheetId_;
  std::vector<int32_t> partIndex_;
};

// nest({ partVertexCounts, partXY, partQuantities, partRotations,
//        partHoleCounts, partHoleVertexCounts, partHoleXY,
//        sheetVertexCounts, sheetXY,
//        sheetHoleCounts, sheetHoleVertexCounts, sheetHoleXY,
//        params }) -> Promise<{ placed, nSheets, fitness, tx, ty, angle,
//                               sheetId, partIndex }>
Napi::Value Nest(const Napi::CallbackInfo& info) {
  Napi::Env env = info.Env();
  Napi::Promise::Deferred deferred = Napi::Promise::Deferred::New(env);

  if (info.Length() < 1 || !info[0].IsObject()) {
    deferred.Reject(Napi::TypeError::New(env, "nest() expects a request object")
                        .Value());
    return deferred.Promise();
  }
  if (g_busy.exchange(true)) {
    deferred.Reject(Napi::Error::New(
                        env,
                        "opennest is already running; the engine supports one "
                        "solve at a time")
                        .Value());
    return deferred.Promise();
  }

  Napi::Object input = info[0].As<Napi::Object>();
  NestRequest request;
  try {
    request.partVertexCounts = ReadIntArray(input.Get("partVertexCounts"));
    request.partXY = ReadDoubleArray(input.Get("partXY"));
    request.partQuantities = ReadIntArray(input.Get("partQuantities"));
    Napi::Value rotations = input.Get("partRotations");
    request.hasPartRotations = rotations.IsTypedArray();
    if (request.hasPartRotations) {
      request.partRotations = ReadIntArray(rotations);
    }
    request.partHoleCounts = ReadIntArray(input.Get("partHoleCounts"));
    request.partHoleVertexCounts =
        ReadIntArray(input.Get("partHoleVertexCounts"));
    request.partHoleXY = ReadDoubleArray(input.Get("partHoleXY"));

    request.sheetVertexCounts = ReadIntArray(input.Get("sheetVertexCounts"));
    request.sheetXY = ReadDoubleArray(input.Get("sheetXY"));
    request.sheetHoleCounts = ReadIntArray(input.Get("sheetHoleCounts"));
    request.sheetHoleVertexCounts =
        ReadIntArray(input.Get("sheetHoleVertexCounts"));
    request.sheetHoleXY = ReadDoubleArray(input.Get("sheetHoleXY"));

    request.params = ReadParams(input.Get("params"));
  } catch (const Napi::Error& error) {
    g_busy.store(false);
    deferred.Reject(error.Value());
    return deferred.Promise();
  }

  NestWorker* worker = new NestWorker(env, deferred, std::move(request));
  worker->Queue();
  return deferred.Promise();
}

void Cancel(const Napi::CallbackInfo&) { nfp_cancel(); }

Napi::Value Progress(const Napi::CallbackInfo& info) {
  return Napi::Number::New(info.Env(), static_cast<double>(nfp_progress()));
}

Napi::Value Fitness(const Napi::CallbackInfo& info) {
  return Napi::Number::New(info.Env(), nfp_fitness());
}

// pollLayout(instanceCount) -> { placed, nSheets, tx, ty, angle, sheetId, partIndex }
Napi::Value PollLayout(const Napi::CallbackInfo& info) {
  Napi::Env env = info.Env();
  int instanceCount = 0;
  if (info.Length() >= 1 && info[0].IsNumber()) {
    instanceCount = std::max(0, info[0].As<Napi::Number>().Int32Value());
  }

  std::vector<double> tx(instanceCount, 0.0);
  std::vector<double> ty(instanceCount, 0.0);
  std::vector<double> angle(instanceCount, 0.0);
  std::vector<int32_t> sheetId(instanceCount, -1);
  std::vector<int32_t> partIndex(instanceCount, -1);
  int nSheets = 0;

  const int placed = nfp_poll_layout(
      instanceCount, instanceCount > 0 ? tx.data() : nullptr,
      instanceCount > 0 ? ty.data() : nullptr,
      instanceCount > 0 ? angle.data() : nullptr,
      instanceCount > 0 ? sheetId.data() : nullptr,
      instanceCount > 0 ? partIndex.data() : nullptr, &nSheets);

  auto toF64 = [&](const std::vector<double>& values) {
    Napi::Float64Array array = Napi::Float64Array::New(env, values.size());
    if (!values.empty()) {
      std::memcpy(array.Data(), values.data(), values.size() * sizeof(double));
    }
    return array;
  };
  auto toI32 = [&](const std::vector<int32_t>& values) {
    Napi::Int32Array array = Napi::Int32Array::New(env, values.size());
    if (!values.empty()) {
      std::memcpy(array.Data(), values.data(), values.size() * sizeof(int32_t));
    }
    return array;
  };

  Napi::Object result = Napi::Object::New(env);
  result.Set("placed", Napi::Number::New(env, placed));
  result.Set("nSheets", Napi::Number::New(env, nSheets));
  result.Set("tx", toF64(tx));
  result.Set("ty", toF64(ty));
  result.Set("angle", toF64(angle));
  result.Set("sheetId", toI32(sheetId));
  result.Set("partIndex", toI32(partIndex));
  return result;
}

Napi::Object Init(Napi::Env env, Napi::Object exports) {
  exports.Set("nest", Napi::Function::New(env, Nest));
  exports.Set("cancel", Napi::Function::New(env, Cancel));
  exports.Set("progress", Napi::Function::New(env, Progress));
  exports.Set("fitness", Napi::Function::New(env, Fitness));
  exports.Set("pollLayout", Napi::Function::New(env, PollLayout));
  return exports;
}

}  // namespace

NODE_API_MODULE(opennest_addon, Init)
