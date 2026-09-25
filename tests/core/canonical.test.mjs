import test from "node:test";
import assert from "node:assert/strict";
import { nestGeometry } from "../../src/geometry/engine.mjs";
import {
  normalizeGeometry,
  clonePolygonTree,
  GeometryValidationError,
} from "../../src/geometry/canonical.mjs";

// Proof that the canonical engine path is importable and runnable without DOM.
assert.equal(
  typeof window,
  "undefined",
  "this test must run without DOM globals (no index.node.mjs import)"
);

const bin = () => {
  const tree = [
    { x: 0, y: 0 },
    { x: 300, y: 0 },
    { x: 300, y: 200 },
    { x: 0, y: 200 },
  ];
  tree.children = [];
  return tree;
};

const part = () => {
  const tree = [
    { x: 0, y: 0 },
    { x: 80, y: 0 },
    { x: 80, y: 50 },
    { x: 0, y: 50 },
  ];
  tree.children = [];
  return tree;
};

const partWithHole = () => {
  const tree = part();
  tree.children = [
    [
      { x: 20, y: 15 },
      { x: 40, y: 15 },
      { x: 40, y: 30 },
      { x: 20, y: 30 },
    ],
  ];
  return tree;
};

function runCanonical(geometry, options = { units: "mm", spacing: 0 }) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const timer = setTimeout(() => {
      if (!settled) {
        settled = true;
        reject(new Error("canonical nesting timed out"));
      }
    }, 30_000);
    nestGeometry(
      geometry,
      async (payload) => {
        if (settled || !payload.status.complete) {
          return;
        }
        settled = true;
        clearTimeout(timer);
        const result = payload;
        try {
          await payload.abort();
        } catch {}
        resolve(result);
      },
      options
    ).catch(reject);
  });
}

test("nests canonical geometry without SVG or DOM", async () => {
  const payload = await runCanonical({
    sheets: [{ id: "sheet-1", polygontree: bin() }],
    parts: [{ id: "part-A", polygontree: part() }],
  });

  assert.equal(payload.status.total, 1);
  assert.equal(payload.status.placed, 1);
  assert.equal(payload.result.length, 1);
  assert.equal(payload.result[0].filename, "part-A");
  assert.equal(typeof payload.result[0].x, "number");
  assert.equal(typeof payload.result[0].y, "number");
  assert.equal(typeof payload.result[0].rotation, "number");
  assert.equal(payload.svg, undefined, "no render context in canonical mode");
});

test("preserves holes and clones geometry (does not mutate caller input)", async () => {
  const geometry = {
    sheets: [{ id: "sheet-1", polygontree: bin() }],
    parts: [{ id: "holey", polygontree: partWithHole() }],
  };
  const before = JSON.stringify(geometry);

  const payload = await runCanonical(geometry);

  assert.equal(payload.result.length, 1);
  assert.equal(payload.result[0].filename, "holey");
  assert.equal(
    JSON.stringify(geometry),
    before,
    "engine must not mutate canonical input (clone before nesting)"
  );
});

test("expands canonical quantity into instances", async () => {
  const payload = await runCanonical({
    sheets: [{ id: "sheet-1", polygontree: bin() }],
    parts: [{ id: "copy", quantity: 3, polygontree: part() }],
  });

  assert.equal(payload.status.total, 3);
  assert.equal(payload.result.length, 3);
  assert.ok(payload.result.every((p) => p.filename === "copy"));
  assert.deepEqual(
    payload.result.map((p) => p.id).sort(),
    [0, 1, 2]
  );
});

test("accepts `bin` as an alias for a single sheet", () => {
  const geometry = normalizeGeometry({
    bin: { id: "sheet-1", polygontree: bin() },
    parts: [{ id: "part-A", polygontree: part() }],
  });
  assert.equal(geometry.sheets.length, 1);
  assert.equal(geometry.sheets[0].id, "sheet-1");
  assert.equal(geometry.parts[0].quantity, 1, "quantity defaults to 1");
});

test("rejects invalid canonical geometry", () => {
  const bad = [
    { parts: [{ id: "p", polygontree: part() }] }, // no sheet
    { sheets: [{ id: "s", polygontree: bin() }] }, // no parts
    { sheets: [{ id: "", polygontree: bin() }], parts: [{ id: "p", polygontree: part() }] },
    { sheets: [{ id: "s", polygontree: bin() }], parts: [{ id: "p", polygontree: [] }] },
    {
      sheets: [{ id: "s", polygontree: bin() }],
      parts: [{ id: "p", polygontree: [{ x: 0, y: 0 }, { x: NaN, y: 1 }, { x: 2, y: 2 }] }],
    },
    {
      sheets: [{ id: "s", polygontree: bin() }],
      parts: [{ id: "p", quantity: 0, polygontree: part() }],
    },
  ];

  for (const geometry of bad) {
    assert.throws(
      () => normalizeGeometry(geometry),
      (error) => error instanceof GeometryValidationError && error.code === "INVALID_GEOMETRY",
      `expected invalid geometry to be rejected: ${JSON.stringify(geometry)}`
    );
  }
});

test("nestGeometry rejects invalid canonical geometry", async () => {
  await assert.rejects(
    () =>
      nestGeometry(
        { sheets: [{ id: "s", polygontree: bin() }], parts: [] },
        () => {},
        { units: "mm", spacing: 0 }
      ),
    (error) => error instanceof GeometryValidationError
  );
});

test("clonePolygonTree is a deep, metadata-free copy", () => {
  const tree = part();
  tree.source = 7;
  tree.children = [
    [
      { x: 0, y: 0, exact: true },
      { x: 1, y: 0 },
      { x: 1, y: 1 },
    ],
  ];
  const clone = clonePolygonTree(tree);

  assert.notEqual(clone, tree);
  assert.notEqual(clone.children[0], tree.children[0]);
  assert.equal(clone.length, tree.length);
  assert.deepEqual(clone[0], { x: tree[0].x, y: tree[0].y });
  assert.equal(clone.source, undefined, "engine metadata is not copied");
  assert.equal(clone.children.length, 1);
  assert.equal(clone.children[0].length, 3);
  assert.deepEqual(clone.children[0][0], { x: 0, y: 0 });
  assert.equal(clone.children[0][0].exact, undefined, "point metadata is not copied");
});
