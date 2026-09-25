import { Job } from "./job.mjs";
import { httpError } from "./input.mjs";

/**
 * Application layer between HTTP and the nesting engine.
 *
 * The HTTP handler must never touch Deepnest workers directly: it creates and
 * queries jobs through this manager.
 *
 * `maxConcurrentJobs` is deliberately 1 for now because the engine's worker /
 * listener lifecycle for truly concurrent jobs is not proven. Extra jobs are
 * queued rather than rejected, and the next one starts automatically when the
 * running job finishes.
 */
export class JobManager {
  constructor({ maxConcurrent = 1 } = {}) {
    this.maxConcurrent = maxConcurrent;
    this.jobs = new Map();
    this.queue = [];
    this.running = new Set();
  }

  create(spec) {
    const job = new Job(spec, this);
    this.jobs.set(job.id, job);
    this.queue.push(job);
    queueMicrotask(() => this.drain());
    return job;
  }

  drain() {
    while (this.running.size < this.maxConcurrent && this.queue.length > 0) {
      const job = this.queue.shift();
      if (job.status !== "queued") {
        continue;
      }
      this.running.add(job);
      job.start().catch((error) => job.fail(error));
    }
  }

  onJobFinished(job) {
    this.running.delete(job);
    const index = this.queue.indexOf(job);
    if (index >= 0) {
      this.queue.splice(index, 1);
    }
    queueMicrotask(() => this.drain());
  }

  get(id) {
    return this.jobs.get(id) || null;
  }

  require(id) {
    const job = this.get(id);
    if (!job) {
      throw httpError(404, "JOB_NOT_FOUND", "Job not found");
    }
    return job;
  }

  async stop(id) {
    const job = this.require(id);
    await job.stop();
    return job;
  }

  delete(id) {
    const job = this.require(id);
    if (!job.terminal) {
      throw httpError(
        409,
        "INVALID_JOB_STATE",
        "Stop the job before deleting it"
      );
    }
    this.jobs.delete(id);
    return true;
  }

  async stopAll() {
    await Promise.all([...this.jobs.values()].map((job) => job.stop()));
  }
}
