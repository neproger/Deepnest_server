import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

// Side-effect import: installs jsdom-backed DOM globals.
import "../../index.node.mjs";

import { parseSvgInput } from "../../src/geometry/svg-adapter.mjs";
import { buildMaterialRegions } from "../../src/geometry/material.mjs";

const require = createRequire(import.meta.url);
const svgParser = require("../../main/svgparser.js");

const here = path.dirname(fileURLToPath(import.meta.url));
const fixture = path.resolve(here, "..", "ТЕСТ.svg");

test("material union splits nested frames into separate ring regions", async () => {
  const raw = readFileSync(fixture, "utf8");
  const bin =
    "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect x=\"0\" y=\"0\" width=\"100000\" height=\"100000\"/></svg>";

  const { renderContext } = await parseSvgInput(
    [{ file: "test.svg", svg: raw, rigid: true }],
    { units: "mm", scale: 25.4, bin, spacing: 0, curveTolerance: 1 }
  );

  // entries[0] is the sheet; the rest are the imported part's elements.
  const elements = renderContext.entries
    .slice(1)
    .flatMap((entry) => entry.svgelements);

  const regions = buildMaterialRegions(elements, svgParser.polygonify);

  // Two painted triangles (a green and a gray outline) merge into two rings.
  assert.equal(regions.length, 2);
  for (const region of regions) {
    assert.ok(region.length >= 3, "outer ring must have a body");
    assert.equal(region.children.length, 1, "ring must have exactly one hole");
    assert.ok(region.children[0].length >= 3);
  }

  // The smaller ring lives inside the larger ring's hole.
  const area = (pts) => {
    let a = 0;
    for (let i = 0; i < pts.length; i += 1) {
      const p = pts[i];
      const q = pts[(i + 1) % pts.length];
      a += p.x * q.y - q.x * p.y;
    }
    return Math.abs(a / 2);
  };
  const [big, small] = regions
    .slice()
    .sort((a, b) => area(b) - area(a));
  assert.ok(area(big) > area(small));
  assert.ok(area(small) > area(small.children[0]));
});

test("svg-adapter keeps a rigid nested object as one (safe) part", async () => {
  const raw = readFileSync(fixture, "utf8");
  const bin =
    "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect x=\"0\" y=\"0\" width=\"100000\" height=\"100000\"/></svg>";

  const { geometry } = await parseSvgInput(
    [{ file: "test.svg", svg: raw, rigid: true }],
    { units: "mm", scale: 25.4, bin, spacing: 0, curveTolerance: 1 }
  );

  // Several material regions -> the engine gets one convex-hull body (never
  // overlapping anything), not a multi-region part.
  assert.equal(geometry.parts.length, 1);
  const part = geometry.parts[0];
  assert.equal(part.regions, undefined);
  assert.ok(part.polygontree.length >= 3);
});
