import { Worker } from "node:worker_threads";
import { DeepNest } from "../../main/deepnest.js";
import { normalizeGeometry, clonePolygonTree } from "./canonical.mjs";

/**
 * Canonical engine entry point.
 *
 * Takes Canonical Geometry and runs it through the existing Deepnest engine
 * (worker_threads / NFP / native addon / GA), with the same
 * progress/result/abort semantics as the SVG path. It knows nothing about SVG,
 * DOM or HTTP.
 *
 * This module must stay importable without DOM globals. It intentionally does
 * not import `main/nestingToSVG.mjs` (the SVG renderer); rendering is injected
 * by the caller via `nestWithRender`.
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
 * Nest canonical geometry. Returns the abort function.
 *
 * @param {{sheets: Array, parts: Array}} geometry
 * @param {(payload: object) => any} callback
 * @param {object} [options]
 */
export async function nestGeometry(geometry, callback, options = {}) {
  return run(geometry, null, callback, options);
}

/**
 * Internal variant used by the SVG path: `renderContext` carries adapter-owned
 * DOM/render data (kept out of canonical geometry).
 */
export async function nestWithRender(geometry, renderContext, callback, options = {}) {
  return run(geometry, renderContext, callback, options);
}

async function run(geometry, renderContext, callback, options) {
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
  const worker = new Worker(new URL("../../main/background.js", import.meta.url));
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
