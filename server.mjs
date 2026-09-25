import busboy from "busboy";
import express from "express";
import { buffer } from "node:stream/consumers";
import { pathToFileURL } from "url";
import { nest } from "./index.node.mjs";
import { JobManager } from "./src/jobs/job-manager.mjs";
import { createJobsRouter } from "./src/api/jobs-router.mjs";

export const app = express();
export const jobs = new JobManager({ maxConcurrent: 1 });
const PORT = 8080;

app.use(express.json({ limit: "10mb" }));
app.use("/api/v1", createJobsRouter(jobs));

function parseForm(req, res, next) {
  const bb = busboy(req);
  const files = [];
  const fields = {};
  bb.on("file", async (name, file, info) => {
    info.mimeType === "image/svg+xml" &&
      files.push({ name, buffer: await buffer(file) });
  });
  bb.on("field", (name, value) => {
    fields[name] = value;
  });
  bb.on("close", () => {
    req.files = files;
    req.fields = fields;
    next();
  });
  req.pipe(bb);
}

/**
 *
 * @param {{ nest: { data: (string | { file: string; svg: string })[], config: any }}} req
 * @param {*} res
 */
async function nestSSE(req, res) {
  try {
    const abort = await nest(
      req.nest.data,
      async ({ svg, result, status }) => {
        res.write("event: response\n");
        res.write(
          `data: ${JSON.stringify({
            svg: svg(),
            data: result,
            status,
          })}\n\n`
        );
      },
      {
        bin: { width: 3000, height: 1000 },
        timeRatio: 0,
        units: "mm",
        spacing: 4,
        progressCallback: ({ progress, phase }) => {
          res.write("event: progress\n");
          res.write(
            `data: ${JSON.stringify({
              progress: Math.round(progress * 100),
              phase,
            })}\n\n`
          );
        },
        ...req.nest.config,
      }
    );

    res.setHeader("Content-Type", "text/event-stream");
    res.setHeader("Cache-Control", "no-cache");
    res.setHeader("Connection", "keep-alive");
    res.write("event: connection\n\n");
    res.once("close", abort);
    res.once("end", abort);
  } catch (error) {
    res.status(500).end(error.toString());
  }
}

app.get("/health", (req, res) => {
  res.json({ status: "ok" });
});

app.post(
  "/nest",
  parseForm,
  (req, res, next) => {
    const data = req.files.map((file) => ({
      svg: file.buffer.toString(),
      file: file.name,
    }));
    req.nest = {
      data,
      config: req.fields.config ? JSON.parse(req.fields.config) : {},
    };
    next();
  },
  nestSSE
);

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
    return res
      .status(400)
      .json({
        error: { code: "INVALID_REQUEST", message: "Malformed JSON body" },
      });
  }
  console.error(error);
  return res.status(500).json({
    error: { code: "ENGINE_ERROR", message: "Unexpected server error" },
  });
});

/**
 * Start the existing HTTP server.
 *
 * This is a thin wrapper around the current `app`, kept so integration tests
 * can boot the real server on an ephemeral port. The HTTP contract (routes,
 * request/response formats) is unchanged.
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
