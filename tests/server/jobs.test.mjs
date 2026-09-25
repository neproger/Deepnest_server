import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { start, jobs } from "../../server.mjs";

const fixtures = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "fixtures"
);

let server;
let base;
let binSvg;
let partSvg;
let partHoleSvg;

before(async () => {
  server = await start({ port: 0, host: "127.0.0.1" });
  base = `http://127.0.0.1:${server.address().port}`;
  binSvg = await readFile(path.resolve(fixtures, "bin.svg"), "utf8");
  partSvg = await readFile(path.resolve(fixtures, "part.svg"), "utf8");
  partHoleSvg = await readFile(path.resolve(fixtures, "part-hole.svg"), "utf8");
});

after(async () => {
  await jobs.stopAll().catch(() => {});
  server.closeAllConnections?.();
  await new Promise((resolve) => server.close(resolve));
});

function jobBody(parts, extra = {}) {
  return {
    input: { format: "svg", bin: { id: "sheet-1", data: binSvg }, parts },
    config: { units: "mm", spacing: 0, timeRatio: 0 },
    ...extra,
  };
}

const geoSheet = () => ({
  id: "g-sheet",
  quantity: 1,
  polygontree: {
    points: [
      { x: 0, y: 0 },
      { x: 300, y: 0 },
      { x: 300, y: 200 },
      { x: 0, y: 200 },
    ],
    children: [],
  },
});

const geoPart = (id, quantity = 1, withHole = false) => ({
  id,
  quantity,
  polygontree: {
    points: [
      { x: 0, y: 0 },
      { x: 80, y: 0 },
      { x: 80, y: 50 },
      { x: 0, y: 50 },
    ],
    children: withHole
      ? [
          {
            points: [
              { x: 20, y: 15 },
              { x: 40, y: 15 },
              { x: 40, y: 30 },
              { x: 20, y: 30 },
            ],
            children: [],
          },
        ]
      : [],
  },
});

function geometryBody(parts, extra = {}) {
  return {
    input: { format: "geometry", units: "mm", sheets: [geoSheet()], parts },
    config: { spacing: 0, timeRatio: 0 },
    ...extra,
  };
}

async function createJob(body) {
  const res = await fetch(`${base}/api/v1/jobs`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  return { status: res.status, json: await res.json().catch(() => null) };
}

async function getJson(pathname) {
  const res = await fetch(`${base}${pathname}`);
  return {
    status: res.status,
    json: await res.json().catch(() => null),
    contentType: res.headers.get("content-type"),
  };
}

async function stopJob(id) {
  return fetch(`${base}/api/v1/jobs/${id}/stop`, { method: "POST" });
}

async function deleteJob(id) {
  return fetch(`${base}/api/v1/jobs/${id}`, { method: "DELETE" });
}

async function waitFor(fn, { timeout = 20_000, interval = 40, message } = {}) {
  const started = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - started > timeout) {
      throw new Error(message || "waitFor timed out");
    }
    await new Promise((resolve) => setTimeout(resolve, interval));
  }
}

const statusOf = async (id) =>
  (await getJson(`/api/v1/jobs/${id}`)).json?.status;

test("creates a job, exposes lifecycle/result with stable ids and svg", async () => {
  const created = await createJob(
    jobBody([
      { id: "part-A", data: partSvg },
      { id: "part-B", data: partSvg },
    ])
  );
  assert.equal(created.status, 202, "POST /jobs must return 202");
  assert.ok(created.json.jobId, "jobId is required");
  assert.ok(
    ["queued", "running"].includes(created.json.status),
    "initial status must be queued or running"
  );
  const id = created.json.jobId;

  const running = await waitFor(async () => {
    const snapshot = (await getJson(`/api/v1/jobs/${id}`)).json;
    return snapshot?.status === "running" ? snapshot : null;
  }, { message: "job never reached running" });
  assert.equal(running.jobId, id);

  const result = await waitFor(async () => {
    const res = await getJson(`/api/v1/jobs/${id}/result`);
    return res.status === 200 ? res.json : null;
  }, { message: "no result while running" });

  const partIds = new Set(result.placements.map((p) => p.partId));
  assert.deepEqual([...partIds].sort(), ["part-A", "part-B"]);
  for (const placement of result.placements) {
    assert.equal(placement.sheetId, "sheet-1", "stable sheet id");
    assert.equal(typeof placement.x, "number");
    assert.equal(typeof placement.y, "number");
    assert.equal(typeof placement.rotation, "number");
    assert.ok(Number.isInteger(placement.instanceId));
    assert.ok(placement.raw && Number.isInteger(placement.raw.id));
  }

  await waitFor(
    async () => (await getJson(`/api/v1/jobs/${id}`)).json?.placementComplete,
    { message: "placementComplete never became true" }
  );

  const svg = await fetch(`${base}/api/v1/jobs/${id}/result.svg`);
  assert.equal(svg.status, 200);
  assert.match(svg.headers.get("content-type") || "", /svg/);
  assert.ok((await svg.text()).includes("<svg"));

  const stopRes = await stopJob(id);
  assert.equal(stopRes.status, 200);
  const stopped = await stopRes.json();
  assert.equal(stopped.status, "stopped");
  assert.ok(stopped.finishedAt, "finishedAt must be set on stop");

  const afterStop = await getJson(`/api/v1/jobs/${id}/result`);
  assert.equal(afterStop.status, 200, "result survives stop");
  assert.equal(afterStop.json.jobStatus, "stopped");

  assert.equal((await deleteJob(id)).status, 204);
  const gone = await getJson(`/api/v1/jobs/${id}`);
  assert.equal(gone.status, 404);
  assert.equal(gone.json.error.code, "JOB_NOT_FOUND");
});

test("places extra jobs in a queue and starts the next after stop", async () => {
  const a = (await createJob(jobBody([{ id: "queue-A", data: partSvg }]))).json;
  await waitFor(
    async () => (await statusOf(a.jobId)) === "running",
    { message: "A did not start" }
  );

  const b = (await createJob(jobBody([{ id: "queue-B", data: partSvg }]))).json;
  assert.equal(await statusOf(b.jobId), "queued", "B must be queued while A runs");

  await stopJob(a.jobId);
  await waitFor(async () => (await statusOf(a.jobId)) === "stopped");
  await waitFor(async () => (await statusOf(b.jobId)) === "running", {
    message: "B did not start after A stopped",
  });

  await stopJob(b.jobId);
  await waitFor(async () => (await statusOf(b.jobId)) === "stopped");
  await deleteJob(a.jobId);
  await deleteJob(b.jobId);
});

test("streams job.started and result.updated over SSE", async () => {
  const blocker = (await createJob(jobBody([{ id: "sse-blocker", data: partSvg }])))
    .json;
  await waitFor(async () => (await statusOf(blocker.jobId)) === "running");

  const queued = (await createJob(jobBody([{ id: "sse-part", data: partSvg }])))
    .json;
  assert.equal(await statusOf(queued.jobId), "queued");

  const controller = new AbortController();
  const events = [];
  const response = await fetch(`${base}/api/v1/jobs/${queued.jobId}/events`, {
    signal: controller.signal,
  });
  assert.equal(response.status, 200);
  assert.match(response.headers.get("content-type") || "", /text\/event-stream/);

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  const consume = () => {
    let index;
    while ((index = buffer.indexOf("\n\n")) !== -1) {
      const block = buffer.slice(0, index);
      buffer = buffer.slice(index + 2);
      const event = block
        .split("\n")
        .find((line) => line.startsWith("event:"));
      const data = block
        .split("\n")
        .find((line) => line.startsWith("data:"));
      if (event && data) {
        events.push({
          type: event.slice(6).trim(),
          data: JSON.parse(data.slice(5).trim()),
        });
      }
    }
  };

  // Release the queue only after the SSE client is attached.
  await stopJob(blocker.jobId);

  try {
    while (!events.some((event) => event.type === "result.updated")) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      consume();
    }
  } catch (error) {
    if (error.name !== "AbortError") throw error;
  } finally {
    controller.abort();
    reader.cancel().catch(() => {});
  }

  assert.ok(
    events.some((event) => event.type === "job.status"),
    "missing initial job.status snapshot"
  );
  assert.ok(
    events.some((event) => event.type === "job.started"),
    "missing job.started event"
  );
  assert.ok(
    events.some((event) => event.type === "engine.progress"),
    "missing engine.progress event"
  );
  assert.ok(
    events.some((event) => event.type === "result.updated"),
    "missing result.updated event"
  );

  await stopJob(queued.jobId);
  await waitFor(async () => (await statusOf(queued.jobId)) === "stopped");
  await deleteJob(blocker.jobId);
  await deleteJob(queued.jobId);
});

test("sequential jobs neither leak listeners nor contaminate results", async () => {
  const warnings = [];
  const onWarning = (warning) => warnings.push(warning);
  process.on("warning", onWarning);
  try {
    for (const partId of ["iso-A", "iso-B", "iso-C"]) {
      const created = (await createJob(jobBody([{ id: partId, data: partSvg }])))
        .json;
      const id = created.jobId;
      await waitFor(async () => (await statusOf(id)) === "running");

      const result = await waitFor(async () => {
        const res = await getJson(`/api/v1/jobs/${id}/result`);
        return res.status === 200 ? res.json : null;
      }, { message: `no result for ${partId}` });
      assert.ok(
        result.placements.every((placement) => placement.partId === partId),
        `cross-job contamination for ${partId}`
      );

      await stopJob(id);
      await waitFor(async () => (await statusOf(id)) === "stopped");
      await deleteJob(id);
    }

    await new Promise((resolve) => setTimeout(resolve, 50));
    assert.equal(
      warnings.filter((w) => w.name === "MaxListenersExceededWarning").length,
      0,
      "MaxListenersExceededWarning must not be emitted"
    );
  } finally {
    process.off("warning", onWarning);
  }
});

test("returns stable JSON errors for invalid input and unknown jobs", async () => {
  const badFormat = await createJob({ input: { format: "dxf" }, config: {} });
  assert.equal(badFormat.status, 400);
  assert.equal(badFormat.json.error.code, "INVALID_REQUEST");

  const noParts = await createJob({
    input: { format: "svg", bin: { id: "s", data: binSvg }, parts: [] },
    config: {},
  });
  assert.equal(noParts.status, 400);
  assert.equal(noParts.json.error.code, "INVALID_REQUEST");

  const badSvg = await createJob({
    input: {
      format: "svg",
      bin: { id: "s", data: binSvg },
      parts: [{ id: "p", data: 123 }],
    },
    config: {},
  });
  assert.equal(badSvg.status, 400);
  assert.equal(badSvg.json.error.code, "INVALID_SVG");

  const badConfig = await createJob(
    jobBody([{ id: "p", data: partSvg }], { config: "nope" })
  );
  assert.equal(badConfig.status, 400);
  assert.equal(badConfig.json.error.code, "INVALID_CONFIG");

  const missing = await getJson("/api/v1/jobs/does-not-exist");
  assert.equal(missing.status, 404);
  assert.equal(missing.json.error.code, "JOB_NOT_FOUND");

  const malformedRes = await fetch(`${base}/api/v1/jobs`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: "{not json",
  });
  assert.equal(malformedRes.status, 400);
  assert.equal((await malformedRes.json()).error.code, "INVALID_REQUEST");
});

test("TIME limit moves the job to completed with a saved result", async () => {
  const created = (
    await createJob(
      jobBody([{ id: "timed", data: partSvg }], {
        execution: { timeLimitMs: 1500 },
      })
    )
  ).json;
  const id = created.jobId;

  const completed = await waitFor(
    async () => {
      const snapshot = (await getJson(`/api/v1/jobs/${id}`)).json;
      return snapshot?.status === "completed" ? snapshot : null;
    },
    { timeout: 15_000, message: "job did not complete via time limit" }
  );
  assert.ok(completed.finishedAt);

  const result = await getJson(`/api/v1/jobs/${id}/result`);
  assert.equal(result.status, 200, "best result must be saved on completion");

  await deleteJob(id);
});

test("refuses to delete a running job until it is stopped", async () => {
  const created = (await createJob(jobBody([{ id: "del-running", data: partSvg }])))
    .json;
  const id = created.jobId;
  await waitFor(async () => (await statusOf(id)) === "running");

  const refused = await deleteJob(id);
  assert.equal(refused.status, 409);
  assert.equal((await refused.json()).error.code, "INVALID_JOB_STATE");

  await stopJob(id);
  await stopJob(id); // idempotent
  await waitFor(async () => (await statusOf(id)) === "stopped");
  assert.equal((await deleteJob(id)).status, 204);
});

test("passes engine config through (rotations=1 disables rotation)", async () => {
  const created = (
    await createJob(
      jobBody([{ id: "cfg", data: partSvg }], {
        config: {
          units: "mm",
          spacing: 0,
          timeRatio: 0,
          rotations: 1,
          placementType: "box",
          populationSize: 4,
          mutationRate: 1,
          curveTolerance: 0.5,
        },
      })
    )
  ).json;
  const id = created.jobId;

  const result = await waitFor(async () => {
    const res = await getJson(`/api/v1/jobs/${id}/result`);
    return res.status === 200 ? res.json : null;
  }, { message: "no result for custom config" });

  assert.ok(result.placements.length > 0);
  assert.ok(
    result.placements.every((placement) => placement.rotation === 0),
    "rotations=1 must produce rotation 0 placements"
  );

  await stopJob(id);
  await waitFor(async () => (await statusOf(id)) === "stopped");
  await deleteJob(id);
});

test("expands part quantity into distinct instances", async () => {
  const created = (
    await createJob(jobBody([{ id: "copy", data: partSvg, quantity: 3 }]))
  ).json;
  const id = created.jobId;

  const result = await waitFor(async () => {
    const res = await getJson(`/api/v1/jobs/${id}/result`);
    return res.status === 200 && res.json.placements.length === 3
      ? res.json
      : null;
  }, { message: "quantity was not expanded to 3 instances" });

  assert.deepEqual(
    result.placements.map((placement) => placement.partId),
    ["copy", "copy", "copy"]
  );
  assert.deepEqual(
    result.placements.map((placement) => placement.instanceId).sort(),
    [0, 1, 2]
  );

  await stopJob(id);
  await waitFor(async () => (await statusOf(id)) === "stopped");
  await deleteJob(id);
});

test("nests parts that contain holes", async () => {
  const created = (await createJob(jobBody([{ id: "holey", data: partHoleSvg }])))
    .json;
  const id = created.jobId;

  const result = await waitFor(async () => {
    const res = await getJson(`/api/v1/jobs/${id}/result`);
    return res.status === 200 ? res.json : null;
  }, { message: "no result for part with a hole" });

  assert.equal(result.placements.length, 1);
  assert.equal(result.placements[0].partId, "holey");

  await stopJob(id);
  await waitFor(async () => (await statusOf(id)) === "stopped");
  await deleteJob(id);
});

test("rejects reserved engine fields leaked through config", async () => {
  const withBin = await createJob(
    jobBody([{ id: "p", data: partSvg }], {
      config: { units: "mm", spacing: 0, bin: { width: 1, height: 1 } },
    })
  );
  assert.equal(withBin.status, 400);
  assert.equal(withBin.json.error.code, "INVALID_CONFIG");

  const withTimeout = await createJob(
    jobBody([{ id: "p", data: partSvg }], {
      config: { units: "mm", spacing: 0, timeout: 10 },
    })
  );
  assert.equal(withTimeout.status, 400);
  assert.equal(withTimeout.json.error.code, "INVALID_CONFIG");
});

test("geometry input: stable ids, quantity, holes and structured result", async () => {
  const created = await createJob(
    geometryBody([geoPart("g-A", 2, true), geoPart("g-B")])
  );
  assert.equal(created.status, 202, "POST /jobs geometry must return 202");
  const id = created.json.jobId;

  const result = await waitFor(async () => {
    const res = await getJson(`/api/v1/jobs/${id}/result`);
    return res.status === 200 ? res.json : null;
  }, { message: "geometry job produced no result while running" });

  assert.equal(result.placements.length, 3);
  assert.deepEqual(
    result.placements.map((p) => p.partId).sort(),
    ["g-A", "g-A", "g-B"]
  );
  assert.ok(
    result.placements.every((p) => p.sheetId === "g-sheet"),
    "sheet id must round-trip unchanged"
  );
  for (const placement of result.placements) {
    assert.equal(typeof placement.x, "number");
    assert.equal(typeof placement.y, "number");
    assert.equal(typeof placement.rotation, "number");
    assert.ok(Number.isInteger(placement.instanceId));
    assert.equal(placement.sheetInstanceId, 0);
  }

  await waitFor(
    async () => (await getJson(`/api/v1/jobs/${id}`)).json?.placementComplete,
    { message: "geometry placement never completed" }
  );

  // Geometry jobs have no source SVG, so result.svg is unavailable.
  const svg = await getJson(`/api/v1/jobs/${id}/result.svg`);
  assert.equal(svg.status, 400);
  assert.equal(svg.json.error.code, "RESULT_FORMAT_UNAVAILABLE");

  await stopJob(id);
  await waitFor(async () => (await statusOf(id)) === "stopped");
  await deleteJob(id);
});

test("geometry input: streams the same SSE lifecycle", async () => {
  const blocker = (await createJob(geometryBody([geoPart("g-blocker")]))).json;
  await waitFor(async () => (await statusOf(blocker.jobId)) === "running");

  const queued = (await createJob(geometryBody([geoPart("g-sse")]))).json;
  assert.equal(await statusOf(queued.jobId), "queued");

  const controller = new AbortController();
  const events = [];
  const response = await fetch(`${base}/api/v1/jobs/${queued.jobId}/events`, {
    signal: controller.signal,
  });
  assert.equal(response.status, 200);
  assert.match(response.headers.get("content-type") || "", /text\/event-stream/);

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  const consume = () => {
    let index;
    while ((index = buffer.indexOf("\n\n")) !== -1) {
      const block = buffer.slice(0, index);
      buffer = buffer.slice(index + 2);
      const event = block.split("\n").find((line) => line.startsWith("event:"));
      const data = block.split("\n").find((line) => line.startsWith("data:"));
      if (event && data) {
        events.push({
          type: event.slice(6).trim(),
          data: JSON.parse(data.slice(5).trim()),
        });
      }
    }
  };

  await stopJob(blocker.jobId);
  try {
    while (!events.some((event) => event.type === "result.updated")) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      consume();
    }
  } catch (error) {
    if (error.name !== "AbortError") throw error;
  } finally {
    controller.abort();
    reader.cancel().catch(() => {});
  }

  assert.ok(events.some((event) => event.type === "job.started"));
  assert.ok(events.some((event) => event.type === "engine.progress"));
  assert.ok(events.some((event) => event.type === "result.updated"));

  await stopJob(queued.jobId);
  await waitFor(async () => (await statusOf(queued.jobId)) === "stopped");
  await deleteJob(blocker.jobId);
  await deleteJob(queued.jobId);
});

test("geometry input: rejects invalid geometry and structure", async () => {
  const emptyPoints = await createJob(
    geometryBody([
      {
        id: "p",
        polygontree: { points: [], children: [] },
      },
    ])
  );
  assert.equal(emptyPoints.status, 400);
  assert.equal(emptyPoints.json.error.code, "INVALID_GEOMETRY");

  const nanPoint = await createJob(
    geometryBody([
      {
        id: "p",
        polygontree: {
          points: [
            { x: 0, y: 0 },
            { x: Number.NaN, y: 1 },
            { x: 2, y: 2 },
          ],
          children: [],
        },
      },
    ])
  );
  assert.equal(nanPoint.status, 400);
  assert.equal(nanPoint.json.error.code, "INVALID_GEOMETRY");

  const badQuantity = await createJob(
    geometryBody([
      {
        id: "p",
        quantity: 0,
        polygontree: { points: geoPart("x").polygontree.points, children: [] },
      },
    ])
  );
  assert.equal(badQuantity.status, 400);
  assert.equal(badQuantity.json.error.code, "INVALID_GEOMETRY");

  const missingId = await createJob(
    geometryBody([
      {
        polygontree: { points: geoPart("x").polygontree.points, children: [] },
      },
    ])
  );
  assert.equal(missingId.status, 400);
  assert.equal(missingId.json.error.code, "INVALID_GEOMETRY");

  const badChildren = await createJob(
    geometryBody([
      {
        id: "p",
        polygontree: {
          points: geoPart("x").polygontree.points,
          children: "nope",
        },
      },
    ])
  );
  assert.equal(badChildren.status, 400);
  assert.equal(badChildren.json.error.code, "INVALID_GEOMETRY");

  const noSheets = await createJob({
    input: { format: "geometry", units: "mm", sheets: [], parts: [geoPart("p")] },
    config: { spacing: 0 },
  });
  assert.equal(noSheets.status, 400);
  assert.equal(noSheets.json.error.code, "INVALID_REQUEST");

  const noParts = await createJob({
    input: { format: "geometry", units: "mm", sheets: [geoSheet()], parts: [] },
    config: { spacing: 0 },
  });
  assert.equal(noParts.status, 400);
  assert.equal(noParts.json.error.code, "INVALID_REQUEST");

  const multiSheet = await createJob({
    input: {
      format: "geometry",
      units: "mm",
      sheets: [geoSheet(), geoSheet()],
      parts: [geoPart("p")],
    },
    config: { spacing: 0 },
  });
  assert.equal(multiSheet.status, 400);
  assert.equal(multiSheet.json.error.code, "INVALID_GEOMETRY");
});

test("geometry input: insufficient sheets yield a partial result with unplaced parts", async () => {
  const tight = {
    id: "tight",
    quantity: 1,
    polygontree: {
      points: [
        { x: 0, y: 0 },
        { x: 120, y: 0 },
        { x: 120, y: 60 },
        { x: 0, y: 60 },
      ],
      children: [],
    },
  };
  const block = {
    id: "big",
    quantity: 2,
    polygontree: {
      points: [
        { x: 0, y: 0 },
        { x: 115, y: 0 },
        { x: 115, y: 55 },
        { x: 0, y: 55 },
      ],
      children: [],
    },
  };

  const created = await createJob({
    input: { format: "geometry", units: "mm", sheets: [tight], parts: [block] },
    config: { spacing: 0, timeRatio: 0 },
  });
  assert.equal(created.status, 202);
  const id = created.json.jobId;

  const result = await waitFor(async () => {
    const res = await getJson(`/api/v1/jobs/${id}/result`);
    return res.status === 200 ? res.json : null;
  }, { message: "no partial result" });

  assert.equal(result.placementComplete, false);
  assert.equal(result.placements.length, 1);
  assert.equal(result.unplaced.length, 1);
  assert.equal(result.unplaced[0].partId, "big");
  assert.deepEqual(result.sheetsUsed, [{ sheetId: "tight", instancesUsed: 1 }]);

  // Partial result is a valid calculation, not an engine error: server alive.
  assert.equal((await fetch(`${base}/health`)).status, 200);

  await stopJob(id);
  await waitFor(async () => (await statusOf(id)) === "stopped");
  await deleteJob(id);
});

test("geometry input: finite sheet quantity > 1 is used up to the limit", async () => {
  const sheet = {
    id: "sheet-A",
    quantity: 2,
    polygontree: {
      points: [
        { x: 0, y: 0 },
        { x: 120, y: 0 },
        { x: 120, y: 60 },
        { x: 0, y: 60 },
      ],
      children: [],
    },
  };
  const block = {
    id: "big",
    quantity: 2,
    polygontree: {
      points: [
        { x: 0, y: 0 },
        { x: 115, y: 0 },
        { x: 115, y: 55 },
        { x: 0, y: 55 },
      ],
      children: [],
    },
  };

  const created = await createJob({
    input: { format: "geometry", units: "mm", sheets: [sheet], parts: [block] },
    config: { spacing: 0, timeRatio: 0 },
  });
  const id = created.json.jobId;

  const result = await waitFor(async () => {
    const res = await getJson(`/api/v1/jobs/${id}/result`);
    return res.status === 200 && res.json.placementComplete ? res.json : null;
  }, { message: "finite multi-instance sheets did not complete" });

  assert.equal(result.placements.length, 2);
  assert.deepEqual(result.unplaced, []);
  assert.deepEqual(result.sheetsUsed, [{ sheetId: "sheet-A", instancesUsed: 2 }]);
  assert.deepEqual(
    result.placements.map((p) => p.sheetInstanceId).sort(),
    [0, 1]
  );

  await stopJob(id);
  await waitFor(async () => (await statusOf(id)) === "stopped");
  await deleteJob(id);
});

test("geometry input: auto sheet mode expands to as many instances as needed", async () => {
  const sheet = {
    id: "auto-sheet",
    mode: "auto",
    polygontree: {
      points: [
        { x: 0, y: 0 },
        { x: 300, y: 0 },
        { x: 300, y: 200 },
        { x: 0, y: 200 },
      ],
      children: [],
    },
  };
  const big = {
    id: "big",
    quantity: 2,
    polygontree: {
      points: [
        { x: 0, y: 0 },
        { x: 280, y: 0 },
        { x: 280, y: 180 },
        { x: 0, y: 180 },
      ],
      children: [],
    },
  };

  const created = await createJob({
    input: { format: "geometry", units: "mm", sheets: [sheet], parts: [big] },
    config: { spacing: 0, timeRatio: 0 },
  });
  const id = created.json.jobId;

  const result = await waitFor(async () => {
    const res = await getJson(`/api/v1/jobs/${id}/result`);
    return res.status === 200 && res.json.placementComplete ? res.json : null;
  }, { message: "auto sheet job did not complete" });

  assert.equal(result.placements.length, 2);
  assert.deepEqual(result.sheetsUsed, [
    { sheetId: "auto-sheet", instancesUsed: 2 },
  ]);

  await stopJob(id);
  await waitFor(async () => (await statusOf(id)) === "stopped");
  await deleteJob(id);
});
