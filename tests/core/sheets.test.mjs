import test from "node:test";
import assert from "node:assert/strict";
import { nestGeometry } from "../../src/geometry/engine.mjs";
import { buildSheetMap, toExternalResult } from "../../src/jobs/input.mjs";

assert.equal(typeof window, "undefined");

const rect = (w, h) => {
  const tree = [
    { x: 0, y: 0 },
    { x: w, y: 0 },
    { x: w, y: h },
    { x: 0, y: h },
  ];
  tree.children = [];
  return tree;
};

function run(geometry, options = {}) {
  return new Promise((resolve, reject) => {
    let settled = false;
    let abortFn = null;
    const timer = setTimeout(() => {
      if (!settled) {
        settled = true;
        abortFn?.().catch(() => {});
        reject(new Error("nesting timed out"));
      }
    }, 30_000);
    const settle = (value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(value);
    };
    nestGeometry(
      geometry,
      async (payload) => {
        if (!payload.status.complete) return;
        try {
          await payload.abort();
        } catch {}
        settle({ payload });
      },
      { units: "mm", spacing: 0, rotations: 2, ...options, onError: (e) => settle({ error: e }) }
    )
      .then((abort) => {
        abortFn = abort;
      })
      .catch(reject);
  });
}

/** Resolve on the first placement (complete or partial) and abort. */
function runPartial(geometry, options = {}) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const timer = setTimeout(() => {
      if (!settled) {
        settled = true;
        reject(new Error("nesting timed out"));
      }
    }, 30_000);
    nestGeometry(
      geometry,
      async (payload) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        try {
          await payload.abort();
        } catch {}
        resolve({ payload });
      },
      { units: "mm", spacing: 0, rotations: 2, ...options, onError: (e) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolve({ error: e });
      } }
    ).catch(reject);
  });
}

test("multiple instances of one sheet geometry are used in order", async () => {
  const result = await run({
    sheets: [{ id: "sheet-A", quantity: 2, polygontree: rect(120, 60) }],
    parts: [{ id: "p", quantity: 2, polygontree: rect(115, 55) }],
  });

  assert.ok(result.payload, `expected success, got ${result.error?.message}`);
  assert.equal(result.payload.status.placed, 2);

  const groups = result.payload.data.placements.map((g) => ({
    sheet: g.sheet,
    sheetid: g.sheetid,
    count: g.sheetplacements.length,
  }));
  assert.equal(groups.length, 2, "two sheet instances opened");
  assert.ok(groups.every((g) => g.sheet === 0), "same sheet geometry (source 0)");
  // NOTE: raw `sheetid` collides for quantity>1 (the engine reuses one polygon
  // object), so instance identity is derived from group order below.

  const sheetMap = buildSheetMap([{ id: "sheet-A", quantity: 2 }]);
  const external = toExternalResult(
    result.payload.data,
    result.payload.status,
    sheetMap
  );
  assert.equal(external.placements.length, 2);
  assert.ok(
    external.placements.every((p) => p.sheetId === "sheet-A"),
    "stable sheetId"
  );
  assert.deepEqual(
    external.placements.map((p) => p.sheetInstanceId).sort(),
    [0, 1],
    "stable per-type sheetInstanceId"
  );
  assert.deepEqual(external.sheetsUsed, [
    { sheetId: "sheet-A", instancesUsed: 2 },
  ]);
});

test("different sheet geometries are distinguished by the engine", async () => {
  const result = await run({
    sheets: [
      { id: "sheet-small", quantity: 1, polygontree: rect(120, 60) },
      { id: "sheet-large", quantity: 1, polygontree: rect(300, 200) },
    ],
    parts: [{ id: "p", quantity: 2, polygontree: rect(115, 55) }],
  });

  assert.ok(result.payload, `expected success, got ${result.error?.message}`);
  assert.equal(result.payload.status.placed, 2);

  const groups = result.payload.data.placements
    .map((g) => ({ sheet: g.sheet, sheetid: g.sheetid }))
    .sort((a, b) => a.sheet - b.sheet);
  assert.deepEqual(groups, [
    { sheet: 0, sheetid: 0 },
    { sheet: 1, sheetid: 1 },
  ]);

  const sheetMap = buildSheetMap([
    { id: "sheet-small", quantity: 1 },
    { id: "sheet-large", quantity: 1 },
  ]);
  const external = toExternalResult(
    result.payload.data,
    result.payload.status,
    sheetMap
  );
  assert.deepEqual(
    [...new Set(external.placements.map((p) => p.sheetId))].sort(),
    ["sheet-large", "sheet-small"]
  );
  assert.deepEqual(
    external.placements.map((p) => p.sheetInstanceId),
    [0, 0],
    "each sheet type instance 0"
  );
  assert.deepEqual(
    external.sheetsUsed.map((s) => s.sheetId).sort(),
    ["sheet-large", "sheet-small"]
  );
});

test("sheet exhaustion returns a partial result with unplaced parts", async () => {
  const { payload, error } = await runPartial({
    sheets: [{ id: "sheet-A", quantity: 1, polygontree: rect(120, 60) }],
    parts: [{ id: "p", quantity: 2, polygontree: rect(115, 55) }],
  });

  assert.ok(payload, error?.message);
  assert.equal(payload.status.complete, false);
  assert.equal(payload.status.placed, 1);
  assert.equal(payload.status.unplaced, 1);
  assert.equal(payload.unplaced.length, 1);
  assert.equal(payload.data.unplaced[0].filename, "p");

  const external = toExternalResult(
    payload.data,
    payload.status,
    buildSheetMap([{ id: "sheet-A", quantity: 1 }])
  );
  assert.equal(external.placements.length, 1);
  assert.equal(external.unplaced.length, 1);
  assert.equal(external.unplaced[0].partId, "p");
  assert.equal(external.unplaced[0].instanceId, 1);
  assert.deepEqual(external.sheetsUsed, [{ sheetId: "sheet-A", instancesUsed: 1 }]);
});

test("a normal job still works after a sheet-exhaustion failure", async () => {
  const result = await run({
    sheets: [{ id: "sheet-A", quantity: 1, polygontree: rect(300, 200) }],
    parts: [{ id: "p", quantity: 1, polygontree: rect(115, 55) }],
  });
  assert.ok(result.payload, "engine recovered after a worker failure");
  assert.equal(result.payload.status.complete, true);
});
