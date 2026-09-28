import test from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { nestGeometry } from "../../src/geometry/engine.mjs";

const ADDON_PATH = fileURLToPath(
  new URL("../../build/Release/opennest.node", import.meta.url)
);

const skip = existsSync(ADDON_PATH)
  ? false
  : "opennest addon not built (run: npm install / npm run build:native)";

function square(width, height) {
  return [
    { x: 0, y: 0 },
    { x: width, y: 0 },
    { x: width, y: height },
    { x: 0, y: height },
  ];
}

function ring(width, height, thickness) {
  const outer = square(width, height);
  outer.children = [
    square(width - 2 * thickness, height - 2 * thickness).map((point) => ({
      x: point.x + thickness,
      y: point.y + thickness,
    })),
  ];
  return outer;
}

function firstResult(geometry, options = {}) {
  return new Promise((resolve, reject) => {
    let settled = false;
    nestGeometry(
      geometry,
      (payload) => {
        if (settled) {
          return;
        }
        settled = true;
        Promise.resolve(payload.abort()).finally(() => resolve(payload));
      },
      { engine: "opennest", spacing: 0, rotations: 4, onError: reject, ...options }
    ).catch(reject);
  });
}

test("opennest nests simple parts on a single sheet", { skip }, async () => {
  const geometry = {
    units: "mm",
    sheets: [{ id: "sheet-1", quantity: 1, polygontree: square(100, 100) }],
    parts: [
      { id: "part-a", quantity: 1, polygontree: square(20, 20) },
      { id: "part-b", quantity: 1, polygontree: square(30, 10) },
    ],
  };

  const payload = await firstResult(geometry, { generations: 3 });
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
  assert.equal(payload.status.complete, true);
  assert.equal(payload.unplaced.length, 0);
});

test("opennest reports over-large parts as unplaced", { skip }, async () => {
  const geometry = {
    units: "mm",
    sheets: [{ id: "sheet-1", quantity: 1, polygontree: square(50, 50) }],
    parts: [
      { id: "huge", quantity: 1, polygontree: square(80, 80) },
      { id: "small", quantity: 1, polygontree: square(10, 10) },
    ],
  };

  const payload = await firstResult(geometry, { generations: 3 });
  assert.equal(payload.status.complete, false);
  assert.equal(payload.unplaced.length, 1);
  assert.equal(payload.unplaced[0].filename, "huge");
});

test("opennest handles a sheet with a hole (void)", { skip }, async () => {
  const sheet = square(100, 100);
  sheet.children = [square(40, 40).map((point) => ({ x: point.x + 30, y: point.y + 30 }))];
  const geometry = {
    units: "mm",
    sheets: [{ id: "sheet-1", quantity: 1, polygontree: sheet }],
    parts: [{ id: "peg", quantity: 1, polygontree: square(10, 10) }],
  };

  const payload = await firstResult(geometry, { generations: 3 });
  assert.equal(payload.status.complete, true);
  assert.equal(payload.unplaced.length, 0);
});

test("opennest nests a part inside another part's hole (islands)", { skip }, async () => {
  // The ring's 150x150 hole is the ONLY free space for the 140x140 part.
  // rotations=1 + tryAllRotations=false used to silently disable hole nesting because the
  // generation-parallel NFP pre-warm cached outer NFPs without their hole pockets.
  const geometry = {
    units: "mm",
    sheets: [{ id: "sheet-1", quantity: 1, polygontree: square(260, 260) }],
    parts: [
      { id: "ring", quantity: 1, polygontree: ring(200, 200, 25) },
      { id: "inner", quantity: 1, polygontree: square(140, 140) },
    ],
  };

  const payload = await firstResult(geometry, {
    rotations: 1,
    tryAllRotations: false,
    generations: 5,
  });
  const placed = payload.data.placements.flatMap(
    (group) => group.sheetplacements
  );
  assert.equal(payload.unplaced.length, 0);
  const inner = placed.find((placement) => placement.filename === "inner");
  assert.ok(inner, "the inner part should be placed");
  // Inside the ring hole -> away from the sheet origin; otherwise it cannot fit at all.
  assert.ok(
    inner.x > 10 && inner.y > 10,
    `expected an island placement, got (${inner.x}, ${inner.y})`
  );
});
