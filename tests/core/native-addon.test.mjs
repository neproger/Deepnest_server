import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  ".."
);

// The Node build of the native Minkowski addon lives here. It is produced by
// `npm install` (node-gyp / gypfile) and must not be confused with the Electron
// build in `minkowski/`.
const addonPath = path.resolve(repoRoot, "build", "Release", "addon.node");
const { calculateNFP } = require(addonPath);

test("native Minkowski addon loads and computes an NFP", () => {
  const A = [
    { x: 0, y: 0 },
    { x: 10, y: 0 },
    { x: 10, y: 10 },
    { x: 0, y: 10 },
  ];
  A.children = [];
  const B = [
    { x: 0, y: 0 },
    { x: 5, y: 0 },
    { x: 5, y: 5 },
    { x: 0, y: 5 },
  ];
  B.children = [];

  const nfp = calculateNFP({ A, B });

  assert.ok(Array.isArray(nfp), "NFP must be an array of polygons");
  assert.ok(nfp.length > 0, "NFP must contain at least one polygon");
  assert.ok(nfp[0].length >= 3, "NFP polygon must contain points");
  for (const point of nfp[0]) {
    assert.equal(typeof point.x, "number", "point.x must be a number");
    assert.equal(typeof point.y, "number", "point.y must be a number");
  }
});
