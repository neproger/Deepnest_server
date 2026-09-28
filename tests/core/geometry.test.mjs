import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Side-effect import: installs jsdom-backed DOM globals (DOMParser, window, ...).
import "../../index.node.mjs";

import { parseSvgInput } from "../../src/geometry/svg-adapter.mjs";
import { nestGeometry } from "../../src/geometry/engine.mjs";

const { DeepNest } = await import("../../main/deepnest.js");

const fixtures = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "fixtures"
);

const config = {
  clipperScale: 10000000,
  curveTolerance: 0.72,
  spacing: 0,
  rotations: 4,
  populationSize: 4,
  mutationRate: 10,
  threads: 1,
  placementType: "gravity",
  mergeLines: true,
  timeRatio: 0.5,
  scale: 72,
  simplify: false,
  units: "mm",
};

const pointKeys = (poly) => {
  const keys = new Set();
  for (const point of poly) for (const key of Object.keys(point)) keys.add(key);
  return [...keys].sort();
};

const arrayProps = (poly) =>
  Object.keys(poly)
    .filter((key) => Number.isNaN(Number(key)))
    .sort();

function captureFirstPayload(deepNest) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error("no background-start payload")),
      5000
    );
    const handler = ({ detail }) => {
      clearTimeout(timer);
      deepNest.eventEmitter.removeEventListener("background-start", handler);
      resolve(detail);
    };
    deepNest.eventEmitter.addEventListener("background-start", handler);
    deepNest.start();
  });
}

test("SVG parsing produces a plain polygon tree with a hole", async () => {
  const svg = await readFile(path.resolve(fixtures, "part-hole.svg"), "utf8");
  const deepNest = new DeepNest(new EventTarget(), config);
  const [part] = deepNest.importsvg(null, null, svg);

  assert.equal(part.polygontree.length, 4, "outer polygon has 4 points");
  assert.ok(
    Array.isArray(part.polygontree.children),
    "hole must be represented as children"
  );
  assert.equal(part.polygontree.children.length, 1, "one hole");
  assert.deepEqual(pointKeys(part.polygontree), ["x", "y"]);
  assert.deepEqual(pointKeys(part.polygontree.children[0]), ["x", "y"]);

  // Import-level metadata (not needed by nesting mathematics itself).
  assert.ok(Array.isArray(part.svgelements), "part keeps DOM elements");
  assert.ok(part.svgelements.length >= 1, "DOM elements retained for rendering");
  assert.ok(part.bounds && typeof part.bounds.width === "number");
  assert.equal(part.quantity, 1);
});

test("one SVG input follows original Deepnest root-to-part behavior", async () => {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg">
    <rect x="0" y="0" width="10" height="10"/>
    <rect x="30" y="0" width="10" height="10"/>
  </svg>`;
  const { geometry } = await parseSvgInput(
    [{ file: "group-1", svg }],
    {
      bin: { width: 100, height: 100 },
      units: "mm",
      scale: 25.4,
    }
  );

  assert.equal(geometry.parts.length, 2);
  assert.deepEqual(geometry.parts.map((part) => part.id), [
    "group-1#1",
    "group-1#2",
  ]);
  assert.deepEqual(
    geometry.parts[0].polygontree.map(({ x, y }) => ({ x, y })),
    [
      { x: 10, y: 10 },
      { x: 0, y: 10 },
      { x: 0, y: 0 },
      { x: 10, y: 0 },
    ]
  );
  assert.deepEqual(
    geometry.parts[1].polygontree.map(({ x, y }) => ({ x, y })),
    [
      { x: 40, y: 10 },
      { x: 30, y: 10 },
      { x: 30, y: 0 },
      { x: 40, y: 0 },
    ]
  );
});

test("rigid SVG input uses one conservative hull and retains preview roots", async () => {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg">
    <rect x="0" y="0" width="10" height="10"/>
    <rect x="30" y="0" width="10" height="10"/>
  </svg>`;
  const { geometry, renderContext } = await parseSvgInput(
    [{ file: "corel-object", svg, rigid: true }],
    { bin: { width: 100, height: 100 }, units: "mm", scale: 25.4 }
  );

  assert.equal(geometry.parts.length, 1);
  assert.equal(geometry.parts[0].id, "corel-object");
  assert.equal(geometry.parts[0].polygontree.length, 4);
  assert.equal(renderContext.previewParts.length, 1);
  assert.equal(renderContext.previewParts[0].polygontrees.length, 2);
});

test("worker payload geometry is DOM-free with parallel metadata arrays", async () => {
  const binSvg = await readFile(path.resolve(fixtures, "bin.svg"), "utf8");
  const holeSvg = await readFile(path.resolve(fixtures, "part-hole.svg"), "utf8");

  const deepNest = new DeepNest(new EventTarget(), config);
  const [sheet] = deepNest.importsvg(null, null, binSvg);
  sheet.sheet = true;
  deepNest.importsvg(null, null, holeSvg);

  const payload = await captureFirstPayload(deepNest);
  deepNest.stop();

  const placement = payload.individual.placement;
  assert.equal(placement.length, 1, "one nestable part");
  assert.equal(payload.sheets.length, 1, "one sheet");
  assert.deepEqual(payload.sheetids, [0]);
  assert.deepEqual(payload.sheetsources, [0]);

  const tree = placement[0];
  assert.ok(!("svgelements" in tree), "no DOM leaks into worker payload");
  assert.ok(Array.isArray(tree.children), "hole survives into payload");
  assert.deepEqual(pointKeys(tree), ["exact", "x", "y"]);
  assert.deepEqual(arrayProps(tree), ["children", "filename", "id", "source"]);
  assert.deepEqual(payload.ids, [tree.id]);
  assert.deepEqual(payload.sources, [tree.source]);
  assert.deepEqual(payload.filenames, [tree.filename]);
  assert.equal(payload.individual.rotation.length, 1);
});

test("engine accepts hand-built polygon trees without the SVG parser", async () => {
  const sheetTree = [
    { x: 0, y: 0 },
    { x: 300, y: 0 },
    { x: 300, y: 200 },
    { x: 0, y: 200 },
  ];
  sheetTree.children = [];

  const partTree = [
    { x: 0, y: 0 },
    { x: 80, y: 0 },
    { x: 80, y: 50 },
    { x: 0, y: 50 },
  ];
  partTree.children = [
    [
      { x: 20, y: 15 },
      { x: 40, y: 15 },
      { x: 40, y: 30 },
      { x: 20, y: 30 },
    ],
  ];

  const deepNest = new DeepNest(new EventTarget(), config);
  deepNest.parts.push({ polygontree: sheetTree, quantity: 1, sheet: true, filename: null });
  deepNest.parts.push({ polygontree: partTree, quantity: 1, filename: "synthetic" });

  const payload = await captureFirstPayload(deepNest);
  deepNest.stop();

  assert.equal(payload.individual.placement.length, 1);
  assert.equal(payload.sheets.length, 1);
  const tree = payload.individual.placement[0];
  assert.equal(tree.length, 4);
  assert.equal(tree.children.length, 1, "synthetic hole preserved");
  assert.deepEqual(pointKeys(tree), ["exact", "x", "y"]);
  assert.equal(tree.filename, "synthetic");
});

test("SVG input flows through canonical geometry into the engine", async () => {
  const binSvg = await readFile(path.resolve(fixtures, "bin.svg"), "utf8");
  const partSvg = await readFile(path.resolve(fixtures, "part.svg"), "utf8");

  const { geometry } = await parseSvgInput([{ file: "part-A", svg: partSvg }], {
    bin: binSvg,
    units: "mm",
    scale: 72,
    spacing: 0,
  });

  // Canonical geometry is DOM-free and carries only identity + polygon tree.
  assert.equal(geometry.sheets.length, 1);
  assert.equal(geometry.parts.length, 1);
  assert.equal(geometry.parts[0].id, "part-A");
  assert.equal(geometry.parts[0].quantity, 1);
  assert.ok(!("svgelements" in geometry.parts[0]), "no DOM in canonical geometry");
  assert.deepEqual(Object.keys(geometry.parts[0]).sort(), [
    "id",
    "polygontree",
    "quantity",
  ]);

  const payload = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("parity nesting timed out")), 30_000);
    nestGeometry(
      geometry,
      async (result) => {
        if (!result.status.complete) return;
        clearTimeout(timer);
        await result.abort().catch(() => {});
        resolve(result);
      },
      { units: "mm", scale: 72, spacing: 0 }
    ).catch(reject);
  });

  assert.equal(payload.status.total, 1);
  assert.equal(payload.result.length, 1);
  assert.equal(payload.result[0].filename, "part-A");
  assert.equal(typeof payload.result[0].x, "number");
  assert.equal(typeof payload.result[0].rotation, "number");
});

test("a Corel-style <rect> part at the origin is not dropped", async () => {
  // Corel exports a rectangle shape as a bare <rect> with no x/y and a
  // viewBox whose ratio differs from the mm width, so the parser applies a
  // scale transform. Regression: a legacy "drop the OnShape background rect at
  // 0/0" heuristic used to silently discard exactly this legitimate part.
  const binSvg =
    '<svg xmlns="http://www.w3.org/2000/svg" width="1220mm" height="2430mm" ' +
    'viewBox="0 0 1220 2430"><rect x="0" y="0" width="1220" height="2430"/></svg>';
  const partSvg =
    '<svg xmlns="http://www.w3.org/2000/svg" width="1200mm" height="90mm" ' +
    'viewBox="0 0 136.2397 10.218"><rect width="136.2397" height="10.218"/></svg>';

  const { geometry } = await parseSvgInput(
    [{ file: "part-1", svg: partSvg, rigid: true }],
    { bin: binSvg, units: "mm", scale: 25.4, curveTolerance: 1 }
  );

  assert.equal(geometry.parts.length, 1, "the rectangle part must be kept");
  const xs = geometry.parts[0].polygontree.map((point) => point.x);
  const ys = geometry.parts[0].polygontree.map((point) => point.y);
  const width = Math.max(...xs) - Math.min(...xs);
  const height = Math.max(...ys) - Math.min(...ys);
  assert.ok(Math.abs(width - 1200) < 1, `width should be ~1200, got ${width}`);
  assert.ok(Math.abs(height - 90) < 1, `height should be ~90, got ${height}`);
});
