/**
 * Pluggable nesting engine registry.
 *
 * The geometry/Job layers never hard-code an engine: they ask this registry for
 * the implementation selected by `DEEPNEST_ENGINE` (or `options.engine`). The
 * built-in Deepnest engine is registered as `"deepnest"`; another engine can be
 * installed without touching the rest of the code.
 *
 * An engine is a module exporting a `nest(geometry, renderContext, callback,
 * options)` function. It implements the exact callback/abort contract documented
 * on `nestWithRender` in `./engine.mjs` and is fully responsible for running the
 * placement algorithm.
 *
 * Two ways to install a custom engine:
 *
 *   1. Programmatic: `registerEngine("myengine", () => import("./my.mjs"))`
 *      before the first job is created.
 *   2. External module path: set `DEEPNEST_ENGINE=myengine` and
 *      `DEEPNEST_ENGINE_MODULE=/abs/path/to/my.mjs`; the module's default export
 *      (or named `nest`) is used.
 */

const engines = new Map();

function normalizeName(name) {
  if (typeof name !== "string" || name.trim() === "") {
    const error = new Error("Nesting engine name must be a non-empty string");
    error.code = "UNKNOWN_ENGINE";
    throw error;
  }
  return name.trim().toLowerCase();
}

/**
 * Register an engine under `name`.
 *
 * @param {string} name
 * @param {Function | { nest: Function }} loader a loader returning the engine
 *   module, or the engine object itself
 * @returns {string} the normalized engine name
 */
export function registerEngine(name, loader) {
  const key = normalizeName(name);
  if (
    typeof loader !== "function" &&
    !(loader && typeof loader.nest === "function")
  ) {
    throw new TypeError(
      `Engine "${name}" must be a loader function or an object with a nest() function`
    );
  }
  engines.set(key, loader);
  return key;
}

/** Whether an engine is registered under `name`. */
export function hasEngine(name) {
  return engines.has(normalizeName(name));
}

/** Names of all registered engines, sorted. */
export function listEngines() {
  return [...engines.keys()].sort();
}

/**
 * Resolve the `nest` function of a registered engine.
 *
 * @param {string} name
 * @returns {Promise<Function>} `nest(geometry, renderContext, callback, options)`
 */
export async function resolveEngine(name) {
  const key = normalizeName(name);
  const loader = engines.get(key);
  if (!loader) {
    const error = new Error(
      `Unknown nesting engine "${name}". Registered engines: ` +
        `${listEngines().join(", ") || "(none)"}. ` +
        "Set DEEPNEST_ENGINE_MODULE to load an external engine."
    );
    error.code = "UNKNOWN_ENGINE";
    throw error;
  }

  const module = typeof loader === "function" ? await loader() : loader;
  const nest =
    module?.nest ??
    module?.default ??
    (typeof module === "function" ? module : undefined);
  if (typeof nest !== "function") {
    const error = new Error(
      `Nesting engine "${name}" does not export a nest() function`
    );
    error.code = "UNKNOWN_ENGINE";
    throw error;
  }
  return nest;
}
