import test from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import {
  registerEngine,
  listEngines,
  nestGeometry,
} from "../../src/geometry/engine.mjs";

const geometry = {
  units: "mm",
  sheets: [
    {
      id: "sheet-1",
      quantity: 1,
      polygontree: [
        { x: 0, y: 0 },
        { x: 100, y: 0 },
        { x: 100, y: 100 },
        { x: 0, y: 100 },
      ],
    },
  ],
  parts: [
    {
      id: "part-a",
      quantity: 1,
      polygontree: [
        { x: 0, y: 0 },
        { x: 10, y: 0 },
        { x: 10, y: 10 },
        { x: 0, y: 10 },
      ],
    },
  ],
};

function fakeEngine(onCall) {
  return {
    nest: async (geom, renderContext, callback, options) => {
      onCall?.({ geom, renderContext, options });
      callback({
        result: [],
        data: { placements: [], index: 0, fitness: 0 },
        status: { better: true, complete: true, placed: 0, total: 0 },
      });
      return async () => {};
    },
  };
}

test("registers the built-in opennest engine", () => {
  assert.ok(listEngines().includes("opennest"));
});

test("delegates to an engine selected with options.engine", async () => {
  let seen = null;
  registerEngine("fake-engine", fakeEngine((call) => (seen = call)));
  assert.ok(listEngines().includes("fake-engine"));

  await nestGeometry(geometry, () => {}, { engine: "fake-engine", spacing: 0 });

  assert.ok(seen, "registered engine was not called");
  assert.equal(seen.renderContext, null);
  assert.equal(seen.options.spacing, 0);
});

test("selects the engine from DEEPNEST_ENGINE", async () => {
  let called = false;
  registerEngine("env-engine", fakeEngine(() => (called = true)));
  const previous = process.env.DEEPNEST_ENGINE;
  process.env.DEEPNEST_ENGINE = "env-engine";
  try {
    await nestGeometry(geometry, () => {});
    assert.ok(called, "DEEPNEST_ENGINE engine was not called");
  } finally {
    if (previous === undefined) {
      delete process.env.DEEPNEST_ENGINE;
    } else {
      process.env.DEEPNEST_ENGINE = previous;
    }
  }
});

test("loads an engine from DEEPNEST_ENGINE_MODULE", async () => {
  const modulePath = fileURLToPath(
    new URL("../fixtures/external-engine.mjs", import.meta.url)
  );
  const previous = process.env.DEEPNEST_ENGINE_MODULE;
  process.env.DEEPNEST_ENGINE_MODULE = modulePath;
  try {
    let called = false;
    await nestGeometry(
      geometry,
      () => (called = true),
      { engine: "external-fixture" }
    );
    assert.ok(called, "external engine module was not loaded/called");
  } finally {
    if (previous === undefined) {
      delete process.env.DEEPNEST_ENGINE_MODULE;
    } else {
      process.env.DEEPNEST_ENGINE_MODULE = previous;
    }
  }
});

test("rejects an unknown engine with UNKNOWN_ENGINE", async () => {
  await assert.rejects(
    () => nestGeometry(geometry, () => {}, { engine: "does-not-exist" }),
    (error) => error.code === "UNKNOWN_ENGINE"
  );
});
