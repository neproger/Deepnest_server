import { EventEmitter } from "node:events";
import { randomUUID } from "node:crypto";
import { adaptInput, buildSheetMap, toExternalResult } from "./input.mjs";
import { nestWithRender } from "../geometry/engine.mjs";

const TERMINAL = new Set(["stopped", "completed", "failed"]);

/**
 * A long-lived nesting job.
 *
 * Lifecycle and placement quality are separate concerns:
 * `placementComplete` means the current best placement holds every part, which
 * does NOT mean the genetic search is finished. A job therefore stays `running`
 * until it is explicitly stopped, a configured time limit elapses, or the
 * engine fails. `status === "completed"` is only used for a configured normal
 * stop condition (e.g. `execution.timeLimitMs`).
 */
export class Job {
  constructor(spec, manager) {
    this.id = randomUUID();
    this.manager = manager;
    this.spec = spec;
    this.binId = spec.sheetId;
    this.sheetMap = null;

    this.status = "queued";
    this.createdAt = new Date().toISOString();
    this.startedAt = null;
    this.finishedAt = null;

    this.progress = null;
    this.result = null;
    this.rawResult = null;
    this.svgFn = null;
    this.svgAvailable = false;
    this.placementComplete = false;
    this.resultUpdatedAt = null;
    this.error = null;

    this.abort = null;
    this.timer = null;
    this.stopRequested = false;

    this.emitter = new EventEmitter();
    // SSE subscribers (and internal wiring) attach here; don't cap them.
    this.emitter.setMaxListeners(0);
  }

  get terminal() {
    return TERMINAL.has(this.status);
  }

  emit(type, data = {}) {
    this.emitter.emit("event", { type, ...data });
  }

  snapshot() {
    return {
      jobId: this.id,
      status: this.status,
      progress: this.progress,
      hasResult: this.result !== null,
      placementComplete: this.placementComplete,
      createdAt: this.createdAt,
      startedAt: this.startedAt,
      finishedAt: this.finishedAt,
      ...(this.error && { error: this.error }),
    };
  }

  async start() {
    if (this.status !== "queued") {
      return;
    }
    this.status = "running";
    this.startedAt = new Date().toISOString();
    this.emit("job.started", { jobId: this.id });

    try {
      const { geometry, renderContext, engineOptions } = await adaptInput(
        this.spec
      );
      this.svgAvailable = Boolean(renderContext);
      this.sheetMap = buildSheetMap(geometry.sheets);
      this.abort = await nestWithRender(
        geometry,
        renderContext,
        ({ data, status, svg }) => this.onResult({ data, status, svg }),
        {
          ...engineOptions,
          progressCallback: (progress) => this.onProgress(progress),
          onError: (error) => this.fail(error),
        }
      );
    } catch (error) {
      this.fail(error);
      return;
    }

    // A stop may have arrived while `nest()` was still importing.
    if (this.stopRequested || this.status !== "running") {
      await this.finish("stopped");
      return;
    }

    const limit = this.spec.execution?.timeLimitMs;
    if (limit) {
      this.timer = setTimeout(() => this.finish("completed"), limit);
    }
  }

  onProgress(progress) {
    if (this.status !== "running") {
      return;
    }
    this.progress = {
      value: typeof progress.progress === "number" ? progress.progress : 0,
      phase: progress.phase,
      index: progress.index,
      threads: progress.threads,
    };
    this.emit("engine.progress", { jobId: this.id, progress: this.progress });
  }

  onResult({ data, status, svg }) {
    if (this.status !== "running") {
      return;
    }
    this.rawResult = data;
    this.svgFn = svg;
    this.result = toExternalResult(data, status, this.sheetMap);
    this.placementComplete = !!status.complete;
    this.resultUpdatedAt = new Date().toISOString();
    this.emit("result.updated", {
      jobId: this.id,
      fitness: this.result.fitness,
      placementComplete: this.placementComplete,
      placed: status.placed,
      total: status.total,
    });
  }

  async stop() {
    if (this.status === "queued") {
      this.status = "stopped";
      this.finishedAt = new Date().toISOString();
      this.emit("job.stopped", { jobId: this.id, status: this.status });
      this.manager.onJobFinished(this);
      return;
    }
    if (this.status !== "running") {
      return; // idempotent for already-terminal jobs
    }
    this.stopRequested = true;
    await this.finish("stopped");
  }

  async finish(finalStatus) {
    if (this.status !== "running") {
      return;
    }
    clearTimeout(this.timer);
    this.timer = null;
    // Set terminal status before awaiting abort so late engine messages are ignored.
    this.status = finalStatus;
    this.finishedAt = new Date().toISOString();

    const abort = this.abort;
    this.abort = null;
    if (abort) {
      try {
        await abort();
      } catch {
        // best-effort shutdown
      }
    }

    this.emit(
      finalStatus === "completed" ? "job.completed" : "job.stopped",
      { jobId: this.id, status: finalStatus }
    );
    this.manager.onJobFinished(this);
  }

  fail(error) {
    if (this.terminal) {
      return;
    }
    clearTimeout(this.timer);
    this.timer = null;
    this.status = "failed";
    this.finishedAt = new Date().toISOString();
    this.error = {
      code: error?.code || "ENGINE_ERROR",
      message: error?.message || String(error),
    };
    const abort = this.abort;
    this.abort = null;
    if (abort) {
      Promise.resolve()
        .then(() => abort())
        .catch(() => {});
    }
    this.emit("job.failed", { jobId: this.id, error: this.error });
    this.manager.onJobFinished(this);
  }
}
