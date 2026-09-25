import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { nest } from "../../index.node.mjs";

const fixtures = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "fixtures"
);

const waitFor = (predicate, timeout, message) =>
  new Promise((resolve, reject) => {
    const started = Date.now();
    const tick = () => {
      if (predicate()) return resolve();
      if (Date.now() - started > timeout) return reject(new Error(message));
      setTimeout(tick, 50);
    };
    tick();
  });

test("headless nesting produces placements with source/x/y/rotation", async () => {
  const svg = await readFile(path.resolve(fixtures, "parts.svg"), "utf8");
  const COPIES = 4;
  const TOTAL = COPIES * 2; // parts.svg contains 2 polygons
  const progress = [];
  let last = null;

  const abort = await nest(
    Array.from({ length: COPIES }, () => svg),
    (payload) => {
      last = payload;
    },
    {
      bin: { width: 200, height: 100 },
      units: "mm",
      spacing: 0,
      timeout: 60_000,
      progressCallback: (data) => progress.push(data),
    }
  );

  try {
    await waitFor(
      () => last && last.status.complete,
      60_000,
      "nesting timed out before placing every element"
    );

    assert.ok(last, "callback was never invoked");
    assert.equal(last.status.total, TOTAL, "unexpected total element count");
    assert.equal(
      last.result.length,
      last.elements.length,
      "every element should be placed"
    );
    assert.ok(progress.length > 0, "no progress reported");

    const sources = new Set();
    const ids = new Set();
    for (const placement of last.result) {
      assert.equal(typeof placement.x, "number", "x must be a number");
      assert.equal(typeof placement.y, "number", "y must be a number");
      assert.equal(typeof placement.rotation, "number", "rotation must be a number");
      assert.equal(typeof placement.source, "number", "source must be a number");
      assert.ok(
        Number.isInteger(placement.source) && placement.source >= 0,
        "source must be a non-negative integer index into the parts list"
      );
      assert.ok(Number.isInteger(placement.id), "id must be an integer");
      // source links a placement back to the imported element. Because
      // index.mjs imports the sheet first, sources are offset, so we only
      // assert that the whole input is accounted for via unique ids.
      sources.add(placement.source);
      ids.add(placement.id);
    }
    assert.equal(ids.size, TOTAL, "each nested instance must have a unique id");
    assert.ok(sources.size > 0, "placements must reference source elements");
  } finally {
    await abort();
  }
});
