import { Worker } from "node:worker_threads";
import { DeepNest } from "../../../main/deepnest.js";
import { normalizeGeometry, clonePolygonTree } from "../canonical.mjs";

/**
 * Built-in nesting engine: the original Deepnest engine (JavaScript GA + NFP +
 * the native Minkowski addon). It is registered as `"deepnest"` by
 * `../engine.mjs` and is the default.
 *
 * Implements the engine contract:
 *   nest(geometry, renderContext, callback, options) -> Promise<abort>
 *
 * `renderContext` (adapter-owned DOM/render data) is optional and only used to
 * expose the server-side SVG renderer; canonical nesting passes `null`.
 */

export const DEFAULT_ENGINE_CONFIG = {
  curveTolerance: 0.72,
  clipperScale: 10000000,
  rotations: 4,
  threads: 4,
  populationSize: 10,
  mutationRate: 10,
  placementType: "gravity",
  mergeLines: true,
  timeRatio: 0.5,
  simplify: false,
  dxfImportScale: 1,
  dxfExportScale: 72,
  endpointTolerance: 0.36,
  conversionServer: "http://convert.deepnest.io",
};

/**
 * Resolve engine options into a DeepNest config.
 *
 * `spacing` is expected in canonical coordinate units. The SVG adapter is
 * responsible for converting SVG units before calling the engine.
 */
export function resolveEngineConfig(options = {}) {
  const {
    timeout = 0,
    progressCallback,
    onError,
    engine,
    units = "inch",
    scale = 72,
    spacing = 0,
    render,
    ...config
  } = options;
  const deepNestConfig = {
    ...DEFAULT_ENGINE_CONFIG,
    ...config,
    units,
    scale,
    spacing,
  };
  return { deepNestConfig, timeout, progressCallback, onError };
}

/**
 * Run canonical geometry through the Deepnest engine.
 *
 * @param {{sheets: Array, parts: Array}} geometry
 * @param {{ render: Function, entries?: Array } | null} renderContext
 * @param {(payload: object) => any} callback
 * @param {object} [options]
 * @returns {Promise<() => Promise<void>>} abort function
 */
export async function nest(geometry, renderContext, callback, options = {}) {
  const normalized = normalizeGeometry(geometry);
  const { deepNestConfig, timeout, progressCallback, onError } =
    resolveEngineConfig(options);

  const eventEmitter = new EventTarget();
  const deepNest = new DeepNest(eventEmitter, deepNestConfig);

  // Sheets first (bin occupies parts[0], preserving the historical source
  // offset), then nestable parts.
  const entries = [
    ...normalized.sheets.map((sheet) => ({ ...sheet, sheet: true })),
    ...normalized.parts.map((part) => ({ ...part, sheet: false })),
  ];

  const renderEntries = renderContext ? renderContext.entries : null;

  deepNest.parts.length = 0;
  entries.forEach((entry, index) => {
    const part = {
      polygontree: clonePolygonTree(entry.polygontree),
      quantity: entry.quantity,
      filename: entry.id,
    };
    if (entry.sheet) {
      part.sheet = true;
    }
    if (renderEntries && renderEntries[index]) {
      part.svgelements = renderEntries[index].svgelements;
      part.bounds = renderEntries[index].bounds;
    }
    deepNest.parts.push(part);
  });

  const total = normalized.parts.reduce((sum, part) => sum + part.quantity, 0);

  let timer = 0;
  let aborted = false;
  const worker = new Worker(new URL("../../../main/background.js", import.meta.url));
  eventEmitter.addEventListener("background-start", ({ detail }) =>
    worker.postMessage(detail)
  );
  worker.on("message", ({ type, data }) =>
    eventEmitter.dispatchEvent(new CustomEvent(type, { detail: data }))
  );
  // Engine/worker failures (e.g. sheet exhaustion inside placeParts) must fail
  // the job instead of crashing the process as an unhandled worker "error".
  // Also stop the main-thread worker timer so the process can exit.
  worker.on("error", (error) => {
    clearTimeout(timer);
    deepNest.stop();
    onError?.(error);
  });
  worker.on("exit", (code) => {
    if (!aborted && code !== 0) {
      clearTimeout(timer);
      deepNest.stop();
      onError?.(new Error(`nesting worker exited with code ${code}`));
    }
  });

  eventEmitter.addEventListener("background-progress", ({ detail }) => {
    detail.progress >= 0 && progressCallback?.(detail);
  });

  const abort = async () => {
    if (aborted) {
      return;
    }
    aborted = true;
    clearTimeout(timer);
    process.off("SIGINT", abort);
    deepNest.stop();
    await worker.terminate();
  };
  timer = timeout && setTimeout(abort, timeout);
  process.on("SIGINT", abort);

  eventEmitter.addEventListener("placement", ({ detail: { data, better } }) => {
    const result = data.placements.flatMap(({ sheetplacements }) =>
      sheetplacements.slice().sort((a, b) => a.id - b.id)
    );
    const unplaced = data.unplaced || [];
    return callback({
      result,
      data,
      elements: normalized.parts,
      unplaced,
      status: {
        better,
        complete: result.length === total,
        placed: result.length,
        total,
        unplaced: unplaced.length,
      },
      svg: renderContext
        ? () => renderContext.render(deepNest, data, `${result.length}/${total}`)
        : undefined,
      abort,
    });
  });

  deepNest.start();

  return abort;
}
