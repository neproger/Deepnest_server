// Alternative nesting engine backed by the ironnest Rust core via a Node-API
// native addon (`native/ironnest-napi`).
//
// Implements the same callback/abort contract as `nestGeometry` in
// `../engine.mjs`, so it can be selected without the Job/HTTP layers knowing.
// Enable it with `DEEPNEST_ENGINE=ironnest`.
//
// Integration notes:
// - the engine is a deterministic single-container / multi-sheet placement
//   oracle: one call returns a complete solution (no progress callback, no
//   in-solve cancellation). This adapter runs it in a worker and drives a
//   seed-varied restart loop, forwarding only improvements, to preserve the
//   Job API's progress + abort + best-result semantics;
// - one sheet *type* per job. `sheets[0].quantity` (or the application-level
//   auto policy) is expanded here into that many physical sheet instances,
//   which is what `nest_multi` consumes;
// - parts are sent as a single outer ring: ironnest does not model part holes,
//   which is conservative for collision and does not affect the client (it
//   re-applies the returned transform to its own geometry). Sheet holes are
//   forwarded as keep-out zones;
// - placements use the same `translate(x,y) rotate(rotation)` model as the
//   other engines.
import { Worker } from "node:worker_threads";
import { fileURLToPath } from "node:url";
import { normalizeGeometry } from "../canonical.mjs";

const ADDON_INDEX = fileURLToPath(
  new URL("../../../native/ironnest-napi/index.js", import.meta.url)
);

function toRing(polygon) {
  return polygon.map((point) => [point.x, point.y]);
}

/**
 * Convert a rotation option into ironnest's angle list.
 *
 * A number is a *count* of evenly spaced orientations (`0`/`1` → no rotation,
 * `4` → `[0, 90, 180, 270]`); an array is used verbatim as angles.
 */
function rotationAngles(value) {
  if (Array.isArray(value)) {
    return value.length > 0 ? value : [0.0];
  }
  const count = Number.isFinite(value) ? Math.round(value) : 1;
  if (count <= 1) {
    return [0.0];
  }
  return Array.from({ length: count }, (_, index) => (index * 360) / count);
}

/**
 * Nest canonical geometry with the ironnest engine.
 *
 * @param {{ sheets: Array, parts: Array }} geometry
 * @param {(payload: object) => any} callback same payload shape as `nestGeometry`
 * @param {object} [options] spacing, rotations, budget, strategy,
 *   separationEffort, columnWeight, seed, addonPath, onError
 * @returns {Promise<() => Promise<void>>} abort function
 */
export async function nestGeometryIronnest(geometry, callback, options = {}) {
  const normalized = normalizeGeometry(geometry);
  if (normalized.parts.length === 0) {
    throw new Error("Nothing to nest");
  }

  const items = normalized.parts.map((part) => toRing(part.polygontree));
  const partIds = normalized.parts.map((part) => part.id);
  const qty = normalized.parts.map((part) => Math.max(1, part.quantity ?? 1));

  // Expand every sheet *type* (with its quantity) into the flat, ordered list
  // of physical sheet instances that `nest_multi` consumes. `sheetTypeOf`
  // remembers, per instance, which canonical sheet type it belongs to, so the
  // result can be mapped back to the client's stable `sheetId`.
  const sheets = [];
  const sheetTypeOf = [];
  normalized.sheets.forEach((sheet, typeIndex) => {
    const outline = toRing(sheet.polygontree);
    const holes = (sheet.polygontree.children || []).map(toRing);
    const quantity = Math.max(1, sheet.quantity ?? 1);
    for (let instance = 0; instance < quantity; instance += 1) {
      sheets.push({ outline, holes });
      sheetTypeOf.push(typeIndex);
    }
  });

  const total = qty.reduce((sum, value) => sum + value, 0);
  const rotations = [rotationAngles(options.rotations ?? 4)];
  const budget = Math.max(1, Math.round(options.budget ?? 1000));
  const seed = Number.isFinite(options.seed) ? Math.round(options.seed) : 1;

  const worker = new Worker(new URL("./worker.mjs", import.meta.url), {
    workerData: {
      addonPath: options.addonPath || ADDON_INDEX,
      items,
      partIds,
      qty,
      sheets,
      sheetTypeOf,
      minSep: options.spacing ?? 0,
      rotations,
      budget,
      strategy: options.strategy,
      // Default to the Fast separation tail: on real Corel jobs the "full"
      // leftover tail dominates the wall clock (measured ~7x slower on a
      // 26-part over-subscribed job for the same layout). Callers can still
      // request a different effort explicitly.
      separationEffort: options.separationEffort ?? "fast",
      columnWeight: options.columnWeight,
      baseSeed: seed,
      total,
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

  let firstResultLogged = false;
  worker.on("message", (message) => {
    if (aborted) {
      return;
    }
    if (message.type === "result") {
      if (!firstResultLogged) {
        firstResultLogged = true;
        console.log(`[ironnest] first result: ${message.result.length} placed`);
      }
      callback({
        result: message.result,
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
      console.error(`[ironnest] engine error: ${message.message}`);
      options.onError?.(new Error(message.message));
    }
  });
  worker.on("error", (error) => {
    console.error(`[ironnest] worker error: ${error && error.message ? error.message : error}`);
    options.onError?.(error);
  });

  return abort;
}
