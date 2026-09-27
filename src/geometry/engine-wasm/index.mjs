// Alternative nesting engine backed by the SVGnest Rust/WASM core.
//
// Implements the same callback/abort contract as `nestGeometry` in
// `../engine.mjs`, so it can be selected without the Job/HTTP layers knowing.
// Enable it with `DEEPNEST_ENGINE=wasm`.
//
// Integration notes:
// - the engine is single-bin (the last polygon is the sheet); multiple sheets
//   are produced by re-running it on the parts that did not fit;
// - parts are sent with the explicit outer/hole hierarchy from Canonical
//   Geometry via the patched `wasm_packer_init_trees`, so unrelated parts are
//   never merged by containment inference;
// - the engine returns the client transform `translate(x,y) rotate(rotation)`
//   applied to the part's own coordinates, which matches our placement model;
// - config ranges are narrow: integer `spacing` 0..31, `curveTolerance` 0..1.5,
//   `rotations`/`populationSize`/`mutationRate` bit-packed.
import { Worker } from "node:worker_threads";
import { normalizeGeometry } from "../canonical.mjs";

const MAX_CURVE_TOLERANCE = 1.5;
const MAX_SPACING = 31;
const MAX_ROTATIONS = 31;
const MAX_POPULATION = 127;
const MAX_MUTATION = 127;

function clampInt(value, min, max) {
  const number = Number.isFinite(value) ? Math.round(value) : min;
  return Math.min(max, Math.max(min, number));
}

function buildEngineConfig(options) {
  return {
    curveTolerance: Math.min(
      MAX_CURVE_TOLERANCE,
      Math.max(0, Number(options.curveTolerance ?? 0.3))
    ),
    spacing: clampInt(options.spacing ?? 0, 0, MAX_SPACING),
    rotations: clampInt(options.rotations ?? 4, 1, MAX_ROTATIONS),
    populationSize: clampInt(options.populationSize ?? 10, 3, MAX_POPULATION),
    mutationRate: clampInt(options.mutationRate ?? 10, 0, MAX_MUTATION),
    useHoles: options.useHoles === true,
  };
}

// Convert a canonical polygon tree into the worker's explicit tree shape.
function toTreeNode(polygon) {
  const ring = [];
  for (const point of polygon) {
    ring.push(point.x, point.y);
  }
  return {
    ring,
    children: (polygon.children || []).map(toTreeNode),
  };
}

function toRing(polygon) {
  const ring = [];
  for (const point of polygon) {
    ring.push(point.x, point.y);
  }
  return ring;
}



/**
 * Nest canonical geometry with the WASM engine.
 *
 * @param {{ sheets: Array, parts: Array }} geometry
 * @param {(payload: object) => any} callback same payload shape as `nestGeometry`
 * @param {object} [options] engine options (curveTolerance, spacing, rotations,
 *   populationSize, mutationRate, useHoles, maxGenerationsPerSheet, chunkSize,
 *   onError)
 * @returns {Promise<() => Promise<void>>} abort function
 */
export async function nestGeometryWasm(geometry, callback, options = {}) {
  const normalized = normalizeGeometry(geometry);
  if (normalized.sheets.length !== 1) {
    throw new Error("The WASM engine currently supports exactly one sheet type");
  }

  // No layout offsets: the explicit-tree entry bypasses containment inference,
  // and the engine's NFP misbehaves when parts are artificially spread out
  // (their rotation is about the drawing origin). Parts are passed in their
  // canonical coordinates and the engine returns the client transform
  // directly, exactly like the original Deepnest placement model.
  const instances = [];
  for (const part of normalized.parts) {
    const quantity = Math.max(1, part.quantity ?? 1);
    for (let instance = 0; instance < quantity; instance += 1) {
      // A fresh tree per instance: encodeTrees assigns `source` ids on these
      // objects, so sharing one tree across quantity copies would collide.
      instances.push({
        id: part.id,
        tree: toTreeNode(part.polygontree),
        offset: [0, 0],
      });
    }
  }
  if (instances.length === 0) {
    throw new Error("Nothing to nest");
  }

  const bin = toRing(normalized.sheets[0].polygontree);
  const maxSheets = Math.max(1, normalized.sheets[0].quantity ?? 1);
  const config = buildEngineConfig(options);

  // The Rust packer evaluates ONE individual per iteration; a GA generation is
  // only produced after all `populationSize` members have been evaluated. So the
  // search budget must be expressed in generations and multiplied out, otherwise
  // (e.g. 40 iterations with population 20) the GA advances just 2 generations.
  const generationsPerSheet = options.generationsPerSheet ?? 100;
  const maxGenerationsPerSheet = Math.max(
    config.populationSize,
    Math.round(generationsPerSheet * config.populationSize)
  );

  const worker = new Worker(new URL("./worker.mjs", import.meta.url), {
    workerData: {
      config,
      bin,
      parts: instances,
      maxSheets,
      maxGenerationsPerSheet,
      chunkSize: options.chunkSize ?? 512,
    },
  });

  let aborted = false;
  const abort = async () => {
    if (aborted) {
      return;
    }
    aborted = true;
    try {
      worker.postMessage({ type: "stop" });
    } catch {
      // worker already gone
    }
    await worker.terminate();
  };

  worker.on("message", (message) => {
    if (aborted) {
      return;
    }
    if (message.type === "result") {
      callback({
        result: message.data.placements.flatMap(
          (group) => group.sheetplacements
        ),
        data: message.data,
        elements: normalized.parts,
        unplaced: message.data.unplaced,
        status: message.status,
        svg: undefined,
        abort,
      });
    } else if (message.type === "progress") {
      options.progressCallback?.(message.data);
    } else if (message.type === "error") {
      options.onError?.(new Error(message.message));
    }
  });
  worker.on("error", (error) => options.onError?.(error));

  return abort;
}
