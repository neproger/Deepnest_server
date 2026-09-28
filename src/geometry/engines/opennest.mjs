import { createRequire } from "node:module";
import { normalizeGeometry } from "../canonical.mjs";

/**
 * Optional nesting engine backed by the vendored OpenNest `nfp_nest` C++ engine
 * (native/opennest, MIT) through the `opennest` N-API addon.
 *
 * Select it with `DEEPNEST_ENGINE=opennest`. Implements the same
 * `nest(geometry, renderContext, callback, options)` contract as the built-in
 * Deepnest engine:
 *
 * - parts and sheets may have holes (direct `children`);
 * - the engine exposes real progress + live best-layout snapshots, so this
 *   adapter polls the addon while the async solve runs and forwards
 *   improvements (progress + best result) to the Job;
 * - `abort()` calls the engine's cooperative cancel.
 *
 * The engine keeps process-global solve state, so only one solve runs at a time
 * (the application already limits concurrency to one job).
 *
 * There is no server-side SVG renderer built into the OpenNest addon, but for
 * SVG input the Job provides a `renderContext` with the original DOM elements;
 * this adapter rebuilds the intermediate Deepnest-like object the existing
 * renderer expects, so `GET /result.svg` keeps working for SVG jobs. Geometry
 * jobs still return `RESULT_FORMAT_UNAVAILABLE`.
 */

const require = createRequire(import.meta.url);

let addon = null;

function loadAddon() {
  if (!addon) {
    try {
      addon = require("bindings")("opennest.node");
    } catch (error) {
      const hint =
        "The OpenNest addon is not built. Run `npm install` (or " +
        "`npm run build:native`) on a machine with a C++ toolchain.";
      const wrapped = new Error(`${hint} ${error?.message ?? error}`);
      wrapped.code = "ENGINE_UNAVAILABLE";
      throw wrapped;
    }
  }
  return addon;
}

// The OpenNest engine keeps process-global solve state and the addon rejects a
// concurrent `nest()`. Serialize calls so a queued job waits for the previous
// solve to finish (or to be cancelled) instead of failing.
let engineChain = Promise.resolve();

function runExclusive(task) {
  const result = engineChain.then(task, task);
  engineChain = result.then(
    () => undefined,
    () => undefined
  );
  return result;
}

function placementTypeCode(value) {
  const code = String(value ?? "").trim().toLowerCase();
  if (code === "box") {
    return 0;
  }
  if (code === "squeeze") {
    return 2;
  }
  return 1; // "gravity" (engine default)
}

function flatten(polys) {
  const vertexCounts = [];
  const xy = [];
  const holeCounts = [];
  const holeVertexCounts = [];
  const holeXY = [];

  for (const poly of polys) {
    const outer = poly.outer;
    vertexCounts.push(outer.length);
    for (const point of outer) {
      xy.push(point.x, point.y);
    }
    const holes = poly.holes ?? [];
    holeCounts.push(holes.length);
    for (const hole of holes) {
      holeVertexCounts.push(hole.length);
      for (const point of hole) {
        holeXY.push(point.x, point.y);
      }
    }
  }

  return {
    vertexCounts: Int32Array.from(vertexCounts),
    xy: Float64Array.from(xy),
    holeCounts: Int32Array.from(holeCounts),
    holeVertexCounts: Int32Array.from(holeVertexCounts),
    holeXY: Float64Array.from(holeXY),
  };
}

function buildContext(normalized) {
  const partIds = normalized.parts.map((part) => part.id);
  const quantities = normalized.parts.map((part) =>
    Math.max(1, part.quantity ?? 1)
  );

  const partPolys = normalized.parts.map((part) => ({
    outer: part.polygontree,
    holes: part.polygontree.children ?? [],
  }));

  const sheetPolys = [];
  const sheetTypeOf = [];
  normalized.sheets.forEach((sheet, typeIndex) => {
    const quantity = Math.max(1, sheet.quantity ?? 1);
    for (let instance = 0; instance < quantity; instance += 1) {
      sheetPolys.push({
        outer: sheet.polygontree,
        holes: sheet.polygontree.children ?? [],
      });
      sheetTypeOf.push(typeIndex);
    }
  });

  const instanceCount = quantities.reduce((sum, value) => sum + value, 0);

  return { partIds, quantities, partPolys, sheetPolys, sheetTypeOf, instanceCount };
}

function buildParams(options) {
  const rotations = Array.isArray(options.rotations)
    ? options.rotations.length
    : options.rotations ?? 4;
  return {
    placementType: placementTypeCode(options.placementType),
    rotations: Math.max(1, rotations),
    mutationRate: options.mutationRate ?? 10,
    populationSize: options.populationSize ?? 10,
    seed: options.seed ?? 30,
    curveTolerance: options.curveTolerance ?? 0.3,
    clipperScale: 1e7,
    spacing: options.spacing ?? 0,
    sheetSpacing: options.sheetSpacing ?? 0,
    useHoles: options.useHoles === false ? 0 : 1,
    simplify: options.simplify ? 1 : 0,
    mode: options.mode ?? 1,
    generations: options.generations ?? 10,
    useParallel: options.useParallel === false ? 0 : 1,
    timeBudgetSecs: options.timeBudgetSecs ?? 0,
    maxSheets: options.maxSheets ?? 0,
    edgeSamples: options.edgeSamples ?? -1,
    compactionPasses: options.compactionPasses ?? -1,
    // Native default is "all rotations on" (capped at 8 by the engine).
    tryAllRotations: options.tryAllRotations === false ? 0 : 1,
    exactNfp: options.exactNfp ? 1 : 0,
    stagnationGens: options.stagnationGens ?? 0,
    exactVoids: options.exactVoids ? 1 : 0,
  };
}

function toData(result, ctx, total) {
  const groups = new Map();
  const unplaced = [];
  const count = result.tx.length;

  for (let k = 0; k < count; k += 1) {
    const sheetId = result.sheetId[k];
    const partIndex = result.partIndex[k];
    const filename = ctx.partIds[partIndex] ?? `part-${partIndex}`;
    if (sheetId < 0) {
      unplaced.push({ id: k, source: partIndex, filename, rotation: 0 });
    } else {
      if (!groups.has(sheetId)) {
        groups.set(sheetId, []);
      }
      groups.get(sheetId).push({
        id: k,
        source: partIndex,
        filename,
        x: result.tx[k],
        y: result.ty[k],
        rotation: result.angle[k],
      });
    }
  }

  const placements = [...groups.entries()]
    .sort(([a], [b]) => a - b)
    .map(([sheetId, sheetplacements]) => ({
      sheet: ctx.sheetTypeOf[sheetId] ?? 0,
      sheetid: sheetId,
      sheetplacements,
    }));

  return {
    data: {
      placements,
      unplaced,
      mergedLength: 0,
      index: 1,
      fitness: result.fitness ?? 0,
    },
    status: {
      better: true,
      complete: unplaced.length === 0,
      placed: count - unplaced.length,
      total,
      unplaced: unplaced.length,
    },
  };
}

/**
 * Build the intermediate object the shared SVG renderer (`nestingToSVG`)
 * expects: `parts` indexed as [sheets..., parts...], where sheets carry
 * `bounds` and parts carry the original `svgelements`. The renderer reads
 * `parts[group.sheet].bounds` and `parts[source].svgelements`.
 */
function buildRenderDeepNest(normalized, renderContext, options) {
  const entries = renderContext.entries ?? [];
  const sheetCount = normalized.sheets.length;
  const parts = [];
  normalized.sheets.forEach((sheet, index) => {
    parts.push({ bounds: entries[index]?.bounds, svgelements: entries[index]?.svgelements });
  });
  normalized.parts.forEach((part, index) => {
    const entry = entries[sheetCount + index];
    parts.push({ bounds: entry?.bounds, svgelements: entry?.svgelements });
  });
  return {
    parts,
    config: () => ({
      units: options.units ?? "inch",
      scale: options.scale ?? 72,
      mergeLines: options.mergeLines ?? true,
      curveTolerance: options.curveTolerance ?? 0.3,
    }),
  };
}

/** Translate engine groups to the renderer's `part.source` indexing. */
function buildRenderResult(data, sheetCount) {
  return {
    placements: data.placements.map((group) => ({
      sheet: group.sheet,
      sheetplacements: group.sheetplacements.map((placement) => ({
        ...placement,
        source: sheetCount + placement.source,
      })),
    })),
    mergedLength: data.mergedLength ?? 0,
  };
}

/**
 * Nest canonical geometry with the OpenNest engine.
 *
 * @param {{ sheets: Array, parts: Array }} geometry
 * @param {object|null} renderContext adapter-owned DOM/render data (SVG path)
 * @param {(payload: object) => any} callback
 * @param {object} [options]
 * @returns {Promise<() => Promise<void>>} abort function
 */
export async function nest(geometry, renderContext, callback, options = {}) {
  const api = loadAddon();
  const normalized = normalizeGeometry(geometry);
  if (normalized.parts.length === 0) {
    throw new Error("Nothing to nest");
  }

  const ctx = buildContext(normalized);
  const partFlat = flatten(ctx.partPolys);
  const sheetFlat = flatten(ctx.sheetPolys);
  const request = {
    partVertexCounts: partFlat.vertexCounts,
    partXY: partFlat.xy,
    partQuantities: Int32Array.from(ctx.quantities),
    partHoleCounts: partFlat.holeCounts,
    partHoleVertexCounts: partFlat.holeVertexCounts,
    partHoleXY: partFlat.holeXY,
    sheetVertexCounts: sheetFlat.vertexCounts,
    sheetXY: sheetFlat.xy,
    sheetHoleCounts: sheetFlat.holeCounts,
    sheetHoleVertexCounts: sheetFlat.holeVertexCounts,
    sheetHoleXY: sheetFlat.holeXY,
    params: buildParams(options),
  };

  const renderer = renderContext?.render
    ? {
        deepNest: buildRenderDeepNest(normalized, renderContext, options),
        sheetCount: normalized.sheets.length,
      }
    : null;

  const total = ctx.instanceCount;
  let aborted = false;
  let timer = null;
  let progressOffset = 0;

  const abort = async () => {
    if (aborted) {
      return;
    }
    aborted = true;
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
    try {
      api.cancel();
    } catch {
      // engine already idle
    }
  };

  const emit = (data, status) => {
    if (aborted) {
      return;
    }
    const placed = status.placed;
    callback({
      result: data.placements.flatMap((group) =>
        group.sheetplacements.slice().sort((a, b) => a.id - b.id)
      ),
      data,
      elements: normalized.parts,
      unplaced: data.unplaced,
      status,
      svg: renderer
        ? () =>
            renderContext.render(
              renderer.deepNest,
              buildRenderResult(data, renderer.sheetCount),
              `${placed}/${total}`
            )
        : undefined,
      abort,
    });
  };

  const poll = () => {
    if (aborted) {
      return;
    }
    options.progressCallback?.({
      progress: 0,
      phase: "placement",
      index: progressOffset + api.progress(),
      threads: 1,
    });
  };

  // Always report at least one progress event, even when the solve is fast
  // enough to finish before the first poll tick.
  options.progressCallback?.({
    progress: 0,
    phase: "placement",
    index: 0,
    threads: 1,
  });
  timer = setInterval(poll, 250);

  // OpenNest returns a complete solution per call, so keep running fresh
  // deterministic seeds and forward improvements — that matches the Job model
  // (continuous best result until stopped). With an explicit engine time budget
  // the engine itself stops, so a single call is enough.
  const params = request.params;
  const baseSeed = params.seed;
  const continuous = !(params.timeBudgetSecs > 0);
  let bestScore = -Infinity;

  const solveLoop = (async () => {
    let iteration = 0;
    do {
      const result = await runExclusive(() =>
        api.nest({
          ...request,
          params: { ...params, seed: baseSeed + iteration },
        })
      );
      if (aborted) {
        break;
      }
      const score =
        result.placed * 1e9 - (result.nSheets ?? 0) * 1e6 + (result.fitness ?? 0);
      if (score > bestScore) {
        bestScore = score;
        const { data, status } = toData(result, ctx, total);
        data.index = iteration + 1;
        emit(data, status);
      }
      progressOffset += params.generations;
      iteration += 1;
    } while (continuous && !aborted);
  })();

  // The solve runs in the background: `nest()` resolves with `abort` immediately
  // (like the Deepnest engine), and failures surface through `onError`.
  solveLoop
    .catch((error) => {
      if (!aborted) {
        options.onError?.(error);
      }
    })
    .finally(() => {
      if (timer) {
        clearInterval(timer);
        timer = null;
      }
    });

  return abort;
}
