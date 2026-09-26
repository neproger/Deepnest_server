// Node runtime bootstrap: installs jsdom-backed DOM globals used by the SVG
// input adapter and the SVG renderer. Geometry-only jobs do not need DOM, but
// the server supports both.
import "./index.node.mjs";

import express from "express";
import { pathToFileURL } from "url";
import { JobManager } from "./src/jobs/job-manager.mjs";
import { createJobsRouter } from "./src/api/jobs-router.mjs";

export const app = express();
export const jobs = new JobManager({ maxConcurrent: 1 });
export const DEFAULT_HOST = "127.0.0.1";
export const DEFAULT_PORT = 8080;

function environmentPort(value) {
  const port = Number(value);
  return Number.isInteger(port) && port >= 0 && port <= 65535
    ? port
    : DEFAULT_PORT;
}

const PORT = environmentPort(process.env.DEEPNEST_PORT);
const HOST = process.env.DEEPNEST_HOST || DEFAULT_HOST;

// The server is intended to be consumed by local CAD and browser clients.
// Keep the network listener local by default, while allowing browser clients
// served from any origin to call the public API and subscribe to SSE.
app.use((req, res, next) => {
  res.setHeader("Access-Control-Allow-Origin", "*");
  res.setHeader("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS");
  res.setHeader(
    "Access-Control-Allow-Headers",
    req.get("Access-Control-Request-Headers") || "Content-Type"
  );
  res.setHeader("Access-Control-Max-Age", "86400");
  if (req.method === "OPTIONS") {
    return res.sendStatus(204);
  }
  return next();
});

app.use(express.json({ limit: "10mb" }));

app.get("/health", (req, res) => {
  res.json({ status: "ok" });
});

app.use("/api/v1", createJobsRouter(jobs));

// Stable JSON error format for the whole HTTP surface.
app.use((error, req, res, next) => {
  if (res.headersSent) {
    return next(error);
  }
  if (error?.status && error?.code) {
    return res
      .status(error.status)
      .json({ error: { code: error.code, message: error.message } });
  }
  if (error instanceof SyntaxError && "body" in error) {
    return res.status(400).json({
      error: { code: "INVALID_REQUEST", message: "Malformed JSON body" },
    });
  }
  console.error(error);
  return res.status(500).json({
    error: { code: "ENGINE_ERROR", message: "Unexpected server error" },
  });
});

/**
 * Start the HTTP server.
 *
 * @param {{ port?: number, host?: string }} [options]
 * @returns {Promise<import("http").Server>}
 */
export async function start({ port = PORT, host = HOST } = {}) {
  return await new Promise((resolve) => {
    const onListen = () => {
      console.log("Server listening on", `http://${host}:${port}`);
      resolve(server);
    };
    const server = app.listen(port, host, onListen);
  });
}

const isMain =
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href;

if (isMain) {
  start().then((server) => {
    process.once("SIGINT", () => {
      server.close();
      process.exit();
    });
  });
}
