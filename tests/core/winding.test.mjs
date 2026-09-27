import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { nestGeometry } from "../../src/geometry/engine.mjs";

const require = createRequire(import.meta.url);
const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  ".."
);
const ClipperLib = require(path.resolve(repoRoot, "main", "util", "clipper.js"));

// Prove this test runs without DOM.
assert.equal(typeof window, "undefined");

const signedArea = (poly) => {
  let area = 0;
  for (let i = 0; i < poly.length; i++) {
    const p = poly[i];
    const q = poly[(i + 1) % poly.length];
    area += p.x * q.y - q.x * p.y;
  }
  return area / 2;
};

const ring = (points, ccw) => (ccw ? points : points.slice().reverse());

const square = (size, ccw) =>
  ring(
    [
      { x: 0, y: 0 },
      { x: size, y: 0 },
      { x: size, y: size },
      { x: 0, y: size },
    ],
    ccw
  );

const rect = (points, ccw) => ring(points, ccw);

function runCanonical(geometry, options = {}) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(value);
    };
    const timer = setTimeout(() => {
      if (!settled) {
        settled = true;
        reject(new Error("nesting timed out"));
      }
    }, 30_000);
    nestGeometry(
      geometry,
      async (payload) => {
        if (settled || !payload.status.complete) return;
        settled = true;
        clearTimeout(timer);
        try {
          await payload.abort();
        } catch {}
        resolve(payload);
      },
      { units: "mm", spacing: 0, ...options }
    ).catch(reject);
  });
}

test("winding: signed area convention (CCW positive)", () => {
  assert.ok(signedArea(square(20, true)) > 0, "CCW is positive");
  assert.ok(signedArea(square(20, false)) < 0, "CW is negative");
});

test("winding: JS Clipper MinkowskiSum is orientation-agnostic", () => {
  const run = (outerCcw) => {
    const A = square(20, outerCcw).map((p) => ({ X: p.x, Y: p.y }));
    const B = square(3, true)
      .map((p) => ({ X: -p.x, Y: -p.y }));
    ClipperLib.JS.ScaleUpPath(A, 10000000);
    ClipperLib.JS.ScaleUpPath(B, 10000000);
    const solution = ClipperLib.Clipper.MinkowskiSum(A, B, true);
    return solution.map((poly) =>
      poly.map((p) => ({ x: p.X / 10000000, y: p.Y / 10000000 }))
    );
  };
  const describe = (solution) =>
    solution.map((p) => ({
      length: p.length,
      area: Math.round(signedArea(p) * 1000) / 1000,
    }));
  assert.deepEqual(describe(run(true)), describe(run(false)));
});

test("winding: engine nests all outer/hole combinations", async () => {
  for (const outerCcw of [true, false]) {
    for (const holeCcw of [true, false]) {
      const sheet = square(60, true);
      sheet.children = [];
      const part = rect(
        [
          { x: 0, y: 0 },
          { x: 40, y: 0 },
          { x: 40, y: 30 },
          { x: 0, y: 30 },
        ],
        outerCcw
      );
      part.children = [
        rect(
          [
            { x: 15, y: 10 },
            { x: 25, y: 10 },
            { x: 25, y: 20 },
            { x: 15, y: 20 },
          ],
          holeCcw
        ),
      ];
      const payload = await runCanonical({
        sheets: [{ id: "sheet", polygontree: sheet }],
        parts: [{ id: "P", polygontree: part }],
      });
      assert.equal(
        payload.status.complete,
        true,
        `outer=${outerCcw ? "CCW" : "CW"} hole=${holeCcw ? "CCW" : "CW"}`
      );
      assert.equal(payload.result.length, 1);
    }
  }
});

test("winding: sheet orientation does not affect nesting", async () => {
  for (const sheetCcw of [true, false]) {
    const sheet = square(60, sheetCcw);
    sheet.children = [];
    const part = square(40, true);
    part.children = [];
    const payload = await runCanonical({
      sheets: [{ id: "sheet", polygontree: sheet }],
      parts: [{ id: "P", polygontree: part }],
    });
    assert.equal(payload.status.placed, 1);
    assert.equal(payload.status.complete, true);
  }
});
