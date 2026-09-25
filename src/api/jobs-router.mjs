import express from "express";
import { validateRequest, httpError } from "../jobs/input.mjs";

function sendError(res, error) {
  const status = error?.status || 500;
  const code =
    error?.code || (status === 500 ? "ENGINE_ERROR" : "INVALID_REQUEST");
  res.status(status).json({
    error: {
      code,
      message: error?.message || "Unexpected error",
    },
  });
}

/**
 * HTTP transport for the Job API. Contains no nesting logic: it validates the
 * request, delegates to the JobManager, and translates job events to SSE.
 */
export function createJobsRouter(manager) {
  const router = express.Router();

  router.post("/jobs", (req, res) => {
    try {
      const spec = validateRequest(req.body);
      const job = manager.create(spec);
      res.status(202).json({ jobId: job.id, status: job.status });
    } catch (error) {
      sendError(res, error);
    }
  });

  router.get("/jobs/:id", (req, res) => {
    try {
      res.json(manager.require(req.params.id).snapshot());
    } catch (error) {
      sendError(res, error);
    }
  });

  router.get("/jobs/:id/result", (req, res) => {
    try {
      const job = manager.require(req.params.id);
      if (!job.result) {
        throw httpError(409, "INVALID_JOB_STATE", "No placement available yet");
      }
      res.json({
        jobId: job.id,
        jobStatus: job.status,
        placementComplete: job.placementComplete,
        updatedAt: job.resultUpdatedAt,
        ...job.result,
      });
    } catch (error) {
      sendError(res, error);
    }
  });

  router.get("/jobs/:id/result.svg", (req, res) => {
    try {
      const job = manager.require(req.params.id);
      if (!job.svgFn) {
        throw httpError(409, "INVALID_JOB_STATE", "No placement available yet");
      }
      res.type("image/svg+xml").send(job.svgFn());
    } catch (error) {
      sendError(res, error);
    }
  });

  router.get("/jobs/:id/events", (req, res) => {
    let job;
    try {
      job = manager.require(req.params.id);
    } catch (error) {
      sendError(res, error);
      return;
    }

    res.setHeader("Content-Type", "text/event-stream");
    res.setHeader("Cache-Control", "no-cache");
    res.setHeader("Connection", "keep-alive");
    res.flushHeaders?.();

    const send = ({ type, ...data }) => {
      res.write(`event: ${type}\n`);
      res.write(`data: ${JSON.stringify(data)}\n\n`);
    };

    send({ type: "job.status", ...job.snapshot() });
    job.emitter.on("event", send);
    const cleanup = () => job.emitter.off("event", send);
    res.once("close", cleanup);
    res.once("end", cleanup);
  });

  router.post("/jobs/:id/stop", async (req, res) => {
    try {
      const job = await manager.stop(req.params.id);
      res.json(job.snapshot());
    } catch (error) {
      sendError(res, error);
    }
  });

  router.delete("/jobs/:id", (req, res) => {
    try {
      manager.delete(req.params.id);
      res.status(204).end();
    } catch (error) {
      sendError(res, error);
    }
  });

  return router;
}
