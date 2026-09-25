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
