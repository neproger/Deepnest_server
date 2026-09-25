import express from "express";
import { pathToFileURL } from "url";
import { JobManager } from "./src/jobs/job-manager.mjs";
import { createJobsRouter } from "./src/api/jobs-router.mjs";

export const app = express();
export const jobs = new JobManager({ maxConcurrent: 1 });
const PORT = 8080;

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
export async function start({ port = PORT, host } = {}) {
  return await new Promise((resolve) => {
    const onListen = () => {
      console.log("Server listening on", `http://${host || "localhost"}:${port}`);
      resolve(server);
    };
    const server = host
      ? app.listen(port, host, onListen)
      : app.listen(port, onListen);
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
