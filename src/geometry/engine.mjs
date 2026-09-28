import { pathToFileURL } from "node:url";
import {
  registerEngine,
  hasEngine,
  resolveEngine,
} from "./engine-registry.mjs";

/**
 * Canonical engine boundary.
 *
 * This module is the stable entry the rest of the application uses
 * (`nestGeometry` / `nestWithRender`). It does not implement an engine itself:
 * it selects a registered engine and delegates. The built-in original Deepnest
 * engine is registered as `"deepnest"` and is the default.
 *
 * Engine selection (in order):
 *   - `options.engine`
 *   - `DEEPNEST_ENGINE`
 *   - `"deepnest"`
 *
 * To install a custom engine set `DEEPNEST_ENGINE=myengine` and
 * `DEEPNEST_ENGINE_MODULE=/abs/path/to/my.mjs` (see `engine-registry.mjs`), or
 * call `registerEngine()` before the first job.
 *
 * This module must stay importable without DOM globals. It intentionally does
 * not import `main/nestingToSVG.mjs` (the SVG renderer); rendering is injected
 * by the caller via `renderContext`.
 */

export { registerEngine, listEngines } from "./engine-registry.mjs";
export { DEFAULT_ENGINE_CONFIG, resolveEngineConfig } from "./engines/deepnest.mjs";

export const DEFAULT_ENGINE = "deepnest";

registerEngine(DEFAULT_ENGINE, () => import("./engines/deepnest.mjs"));

async function resolveSelectedEngine(options) {
  const name = options.engine || process.env.DEEPNEST_ENGINE || DEFAULT_ENGINE;
  const modulePath = process.env.DEEPNEST_ENGINE_MODULE;
  if (!hasEngine(name) && modulePath) {
    registerEngine(name, () =>
      import(pathToFileURL(modulePath).href)
    );
  }
  return resolveEngine(name);
}

async function run(geometry, renderContext, callback, options = {}) {
  const nest = await resolveSelectedEngine(options);
  return nest(geometry, renderContext, callback, options);
}

/**
 * Nest canonical geometry with the selected engine. Returns the abort function.
 *
 * @param {{sheets: Array, parts: Array}} geometry
 * @param {(payload: object) => any} callback
 * @param {object} [options]
 */
export async function nestGeometry(geometry, callback, options = {}) {
  return run(geometry, null, callback, options);
}

/**
 * Internal variant used by the SVG path: `renderContext` carries adapter-owned
 * DOM/render data (kept out of canonical geometry).
 *
 * @param {{sheets: Array, parts: Array}} geometry
 * @param {object|null} renderContext
 * @param {(payload: object) => any} callback
 * @param {object} [options]
 */
export async function nestWithRender(geometry, renderContext, callback, options = {}) {
  return run(geometry, renderContext, callback, options);
}
