// Worker-thread driver for the ironnest native engine.
//
// ironnest's public API is a single synchronous call that returns a complete,
// deterministic solution: there is no progress callback and no mid-solve
// cancellation inside it. To fit the Job contract (progress + abort, best
// result while running) we run the solve here, in a worker, and drive a
// seed-varied restart loop: each iteration is a fresh deterministic solve, and
// only improvements over the best-so-far are forwarded to the caller. `abort()`
// terminates the worker and therefore interrupts an in-flight solve.
//
// Part holes are not represented by ironnest (one outline per part type), so
// only the outer ring is sent; this is conservative for collision (the part is
// treated as solid) and does not affect the client, which re-applies the
// transform to its own geometry. Sheet holes are forwarded as keep-out zones.
import { parentPort, workerData } from "node:worker_threads";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);

const {
  addonPath,
  items,
  partIds,
  qty,
  sheets,
  sheetTypeOf,
  minSep,
  rotations,
  budget,
  restarts,
  strategy,
  separationEffort,
  columnWeight,
  baseSeed,
  total,
} = workerData;

let stopRequested = false;
parentPort.on("message", (message) => {
  if (message && message.type === "stop") {
    stopRequested = true;
  }
});

function ringArea(ring) {
  let area = 0;
  for (let i = 0, n = ring.length; i < n; i += 1) {
    const [x1, y1] = ring[i];
    const [x2, y2] = ring[(i + 1) % n];
    area += x1 * y2 - x2 * y1;
  }
  return Math.abs(area) / 2;
}

const partAreas = items.map(ringArea);
const sheetAreas = sheets.map((sheet) => ringArea(sheet.outline));

function requestFor(seed, effort) {
  return JSON.stringify({
    items,
    qty,
    sheets,
    min_sep: minSep,
    rotations,
    seed,
    budget,
    restarts,
    strategy,
    separation_effort: effort,
    column_weight: columnWeight,
  });
}

// Map ironnest's `{ per_sheet, unplaced }` onto the engine payload contract
// consumed by `src/jobs/input.mjs#toExternalResult`.
function toPayload(raw) {
  const placements = [];
  let nextId = 0;
  let placedArea = 0;
  let capacity = 0;

  raw.per_sheet.forEach((sheetPlacements, sheetInstance) => {
    if (sheetPlacements.length === 0) {
      return;
    }
    capacity += sheetAreas[sheetInstance] ?? 0;
    const sheetplacements = sheetPlacements.map((p) => {
      placedArea += partAreas[p.item] ?? 0;
      return {
        id: nextId++,
        source: p.item,
        filename: partIds[p.item],
        x: p.x,
        y: p.y,
        rotation: p.rotation,
      };
    });
    placements.push({
      sheet: sheetTypeOf[sheetInstance] ?? 0,
      sheetid: sheetInstance,
      sheetplacements,
    });
  });

  const placed = nextId;
  const density = capacity > 0 ? placedArea / capacity : 0;

  return {
    result: placements.flatMap((group) => group.sheetplacements),
    data: {
      placements,
      unplaced: raw.unplaced.map((item) => ({
        id: nextId++,
        filename: partIds[item],
        rotation: 0,
      })),
      mergedLength: 0,
      index: 0,
      fitness: density,
    },
    status: {
      better: true,
      complete: raw.unplaced.length === 0,
      placed,
      total,
      unplaced: raw.unplaced.length,
    },
  };
}

function score(raw) {
  const placed = raw.per_sheet.reduce((sum, group) => sum + group.length, 0);
  const sheetsUsed = raw.per_sheet.filter((group) => group.length > 0).length;
  const payload = toPayload(raw);
  return placed * 1e9 - sheetsUsed * 1e6 + payload.data.fitness;
}

function post(type, value) {
  parentPort.postMessage({ type, ...value });
}

async function main() {
  let addon;
  try {
    addon = require(addonPath);
  } catch (error) {
    post("error", {
      message:
        `ironnest native addon could not be loaded from ${addonPath}. ` +
        `Build it with: npm --prefix native/ironnest-napi run build. ` +
        `Underlying error: ${error && error.message ? error.message : String(error)}`,
      stack: error && error.stack ? error.stack : undefined,
    });
    return;
  }

  post("progress", { data: { progress: 0, phase: "placement", index: 0, threads: 1 } });

  let bestScore = -Infinity;
  let posted = false;
  let iteration = 0;

  try {
    do {
      // Progressive effort: iteration 0 is a cheap construction-only preview
      // (`off`) so a first layout appears almost immediately; iteration 1 runs a
      // quick `fast` pass for a solid layout early; later iterations run the
      // caller-selected effort (e.g. `max`/`full`) to refine. Each seed's result
      // is forwarded only when it improves on the best so far.
      const effort =
        iteration === 0 ? "off" : iteration === 1 ? "fast" : separationEffort;
      const seed = iteration === 0 ? baseSeed : baseSeed + (iteration - 1);
      const raw = JSON.parse(addon.nestMulti(requestFor(seed, effort)));
      const candidate = score(raw);
      if (candidate > bestScore) {
        bestScore = candidate;
        posted = true;
        const payload = toPayload(raw);
        payload.data.index = iteration + 1;
        post("result", payload);
      }
      iteration += 1;
      post("progress", {
        data: { progress: 0, phase: "placement", index: iteration, threads: 1 },
      });
      // Yield so a stop message can be processed between solves.
      await new Promise((resolve) => setImmediate(resolve));
    } while (!stopRequested);

    if (!posted) {
      post("result", {
        result: [],
        data: {
          placements: [],
          unplaced: partIds.flatMap((filename, item) =>
            Array.from({ length: qty[item] }, (_, index) => ({
              id: index,
              filename,
              rotation: 0,
            }))
          ),
          mergedLength: 0,
          index: 0,
          fitness: 0,
        },
        status: { better: true, complete: false, placed: 0, total, unplaced: total },
      });
    }

    post("done", {});
  } catch (error) {
    post("error", {
      message: error && error.message ? error.message : String(error),
      stack: error && error.stack ? error.stack : undefined,
    });
  }
}

main();
