import test from "node:test";
import assert from "node:assert/strict";
import { nestGeometryWasm } from "../../src/geometry/engine-wasm/index.mjs";

function square(width, height) {
  return [
    { x: 0, y: 0 },
    { x: width, y: 0 },
    { x: width, y: height },
    { x: 0, y: height },
  ];
}

function frame(width, height, holeX, holeY, holeW, holeH) {
  const outer = square(width, height);
  outer.children = [
    [
      { x: holeX, y: holeY },
      { x: holeX + holeW, y: holeY },
      { x: holeX + holeW, y: holeY + holeH },
      { x: holeX, y: holeY + holeH },
    ],
  ];
  return outer;
}

function runToComplete(geometry, options = {}) {
  return new Promise((resolve, reject) => {
    let abort = null;
    let completed = false;
    const release = () => {
      if (completed && abort) {
        const fn = abort;
        abort = null;
        fn().catch(() => {});
      }
    };
    nestGeometryWasm(
      geometry,
      (result) => {
        if (result.status.complete && !completed) {
          completed = true;
          resolve(result);
          release();
        }
      },
      {
        curveTolerance: 0.3,
        spacing: 2,
        rotations: 4,
        populationSize: 10,
        mutationRate: 10,
        maxGenerationsPerSheet: 50,
        onError: reject,
        ...options,
      }
    ).then((fn) => {
      abort = fn;
      release();
    });
  });
}

test("wasm engine nests simple parts on a single sheet", async () => {
  const geometry = {
    units: "mm",
    sheets: [{ id: "sheet-1", quantity: 1, polygontree: square(100, 100) }],
    parts: [
      { id: "part-a", quantity: 1, polygontree: square(20, 20) },
      { id: "part-b", quantity: 1, polygontree: square(30, 10) },
    ],
  };

  const payload = await runToComplete(geometry);
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
});

test("wasm engine spreads identical parts across sheet instances", async () => {
  const geometry = {
    units: "mm",
    sheets: [{ id: "sheet-1", quantity: 4, polygontree: square(25, 25) }],
    parts: [{ id: "block", quantity: 2, polygontree: square(20, 20) }],
  };

  const payload = await runToComplete(geometry, { spacing: 2 });
  const groups = payload.data.placements.filter(
    (group) => group.sheetplacements.length > 0
  );
  assert.equal(payload.unplaced.length, 0);
  // Two 20x20 parts cannot share a 25x25 sheet at spacing 2.
  assert.equal(groups.length, 2);
  assert.deepEqual(
    groups.map((group) => group.sheetid).sort((a, b) => a - b),
    [0, 1]
  );
});

test("wasm engine nests a part with a hole", async () => {
  const geometry = {
    units: "mm",
    sheets: [{ id: "sheet-1", quantity: 1, polygontree: square(100, 100) }],
    parts: [
      { id: "frame", quantity: 1, polygontree: frame(40, 40, 15, 15, 10, 10) },
    ],
  };

  const payload = await runToComplete(geometry);
  const placed = payload.data.placements.flatMap(
    (group) => group.sheetplacements
  );
  assert.equal(placed.length, 1);
  assert.equal(placed[0].filename, "frame");
  assert.ok(Number.isFinite(placed[0].x));
  assert.ok(Number.isFinite(placed[0].y));
});
