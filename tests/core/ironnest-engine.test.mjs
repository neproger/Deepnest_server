import test from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { nestGeometryIronnest } from "../../src/geometry/engine-ironnest/index.mjs";

const ADDON_INDEX = fileURLToPath(
  new URL("../../native/ironnest-napi/index.js", import.meta.url)
);

const skip = existsSync(ADDON_INDEX)
  ? false
  : "ironnest native addon not built (run: npm --prefix native/ironnest-napi run build)";

function square(width, height) {
  return [
    { x: 0, y: 0 },
    { x: width, y: 0 },
    { x: width, y: height },
    { x: 0, y: height },
  ];
}

function firstResult(geometry, options = {}) {
  return new Promise((resolve, reject) => {
    let settled = false;
    nestGeometryIronnest(
      geometry,
      (payload) => {
        if (settled) {
          return;
        }
        settled = true;
        Promise.resolve(payload.abort()).finally(() => resolve(payload));
      },
      {
        spacing: 0,
        rotations: 4,
        budget: 2000,
        onError: reject,
        ...options,
      }
    ).catch(reject);
  });
}

test("ironnest engine nests simple parts on a single sheet", { skip }, async () => {
  const geometry = {
    units: "mm",
    sheets: [{ id: "sheet-1", quantity: 1, polygontree: square(100, 100) }],
    parts: [
      { id: "part-a", quantity: 1, polygontree: square(20, 20) },
      { id: "part-b", quantity: 1, polygontree: square(30, 10) },
    ],
  };

  const payload = await firstResult(geometry);
  const placed = payload.data.placements.flatMap(
    (group) => group.sheetplacements
  );
  assert.equal(placed.length, 2);
  assert.deepEqual(
    placed.map((placement) => placement.filename).sort(),
    ["part-a", "part-b"]
  );
  for (const placement of placed) {
    assert.ok(Number.isFinite(placement.x));
    assert.ok(Number.isFinite(placement.y));
    assert.ok(Number.isFinite(placement.rotation));
  }
  assert.equal(payload.unplaced.length, 0);
  assert.equal(payload.status.complete, true);
});

test("ironnest engine spreads identical parts across sheet instances", { skip }, async () => {
  const geometry = {
    units: "mm",
    sheets: [{ id: "sheet-1", quantity: 4, polygontree: square(25, 25) }],
    parts: [{ id: "block", quantity: 2, polygontree: square(20, 20) }],
  };

  const payload = await firstResult(geometry, { spacing: 2 });
  const groups = payload.data.placements.filter(
    (group) => group.sheetplacements.length > 0
  );
  assert.equal(payload.unplaced.length, 0);
  // Two 20x20 parts cannot share a 25x25 sheet at spacing 2.
  assert.equal(groups.length, 2);
  assert.deepEqual(groups.map((group) => group.sheetid), [0, 1]);
});

test("ironnest engine reports over-large parts as unplaced", { skip }, async () => {
  const geometry = {
    units: "mm",
    sheets: [{ id: "sheet-1", quantity: 1, polygontree: square(50, 50) }],
    parts: [
      { id: "huge", quantity: 1, polygontree: square(80, 80) },
      { id: "small", quantity: 1, polygontree: square(10, 10) },
    ],
  };

  const payload = await firstResult(geometry);
  assert.equal(payload.status.complete, false);
  assert.equal(payload.unplaced.length, 1);
  assert.equal(payload.unplaced[0].filename, "huge");
});

test("ironnest engine is deterministic for a fixed seed", { skip }, async () => {
  const geometry = {
    units: "mm",
    sheets: [{ id: "sheet-1", quantity: 1, polygontree: square(200, 100) }],
    parts: [{ id: "block", quantity: 30, polygontree: square(26, 16) }],
  };

  const a = await firstResult(geometry, { seed: 7, budget: 800, spacing: 3 });
  const b = await firstResult(geometry, { seed: 7, budget: 800, spacing: 3 });
  assert.deepEqual(a.data.placements, b.data.placements);
});
