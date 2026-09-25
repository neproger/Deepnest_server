import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { start } from "../../server.mjs";

const fixtures = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "fixtures"
);

function parseSseBlock(block) {
  let event = "message";
  const data = [];
  for (const line of block.split("\n")) {
    if (line.startsWith("event:")) event = line.slice(6).trim();
    else if (line.startsWith("data:")) data.push(line.slice(5).trim());
  }
  return data.length > 0 ? { event, data: data.join("\n") } : null;
}

async function postNest({ port, files, config, signal, onEvent }) {
  const form = new FormData();
  form.append("config", JSON.stringify(config));
  for (const [name, svg] of files) {
    form.append(name, new Blob([svg], { type: "image/svg+xml" }), name);
  }

  const response = await fetch(`http://127.0.0.1:${port}/nest`, {
    method: "POST",
    body: form,
    signal,
  });
  assert.equal(response.status, 200, "unexpected HTTP status");
  assert.match(
    response.headers.get("content-type") || "",
    /text\/event-stream/,
    "server did not answer with SSE"
  );

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      let index;
      while ((index = buffer.indexOf("\n\n")) !== -1) {
        const block = buffer.slice(0, index);
        buffer = buffer.slice(index + 2);
        const parsed = parseSseBlock(block);
        if (parsed) onEvent(parsed);
      }
    }
  } catch (error) {
    if (error.name !== "AbortError") throw error;
  } finally {
    reader.cancel().catch(() => {});
  }
}

test(
  "existing HTTP server performs a real nesting job over SSE",
  { timeout: 60_000 },
  async (t) => {
    const server = await start({ port: 0, host: "127.0.0.1", prefetchFonts: false });
    const { port } = server.address();

    t.after(
      () =>
        new Promise((resolve) => {
          server.closeAllConnections?.();
          server.close(resolve);
        })
    );

    const svg = await readFile(path.resolve(fixtures, "parts.svg"), "utf8");
    const controller = new AbortController();
    const progress = [];
    let complete = null;

    await postNest({
      port,
      files: [
        ["part-a.svg", svg],
        ["part-b.svg", svg],
        ["part-c.svg", svg],
      ],
      config: {
        units: "mm",
        spacing: 0,
        timeRatio: 0,
        bin: { width: 200, height: 100 },
      },
      signal: controller.signal,
      onEvent: (event) => {
        if (event.event === "progress") {
          progress.push(JSON.parse(event.data));
        } else if (event.event === "response") {
          const payload = JSON.parse(event.data);
          if (payload.status?.complete) {
            complete = payload;
            controller.abort();
          }
        }
      },
    });

    assert.ok(complete, "never received a completed placement over SSE");
    assert.equal(complete.status.placed, complete.status.total);
    assert.ok(Array.isArray(complete.data), "response data must be an array");
    assert.ok(complete.data.length > 0, "no placements returned");
    assert.equal(typeof complete.svg, "string", "server should include svg result");
    assert.ok(complete.svg.includes("<svg"), "svg result must be serialized SVG");
    assert.ok(progress.length > 0, "no progress events reported");

    for (const placement of complete.data) {
      assert.equal(typeof placement.x, "number", "x must be a number");
      assert.equal(typeof placement.y, "number", "y must be a number");
      assert.equal(typeof placement.rotation, "number", "rotation must be a number");
      assert.equal(typeof placement.source, "number", "source must be a number");
      assert.ok(Number.isInteger(placement.id), "id must be an integer");
    }
  }
);
