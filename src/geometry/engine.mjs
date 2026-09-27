import { nestGeometryIronnest } from "./engine-ironnest/index.mjs";

/**
 * Engine boundary.
 *
 * The ironnest Rust core (`native/ironnest-napi`, vendored source in
 * `native/vendor/ironnest`) is the ONLY nesting engine. There is no engine
 * selection and no `DEEPNEST_ENGINE` switch: the original Deepnest GA/native-NFP
 * engine and the SVGnest WASM core have been removed.
 *
 * This module must stay importable without DOM globals and must not import the
 * SVG parser, the DOM or HTTP.
 */

export const DEFAULT_ENGINE_CONFIG = {
  curveTolerance: 0.72,
  clipperScale: 10000000,
  rotations: 4,
  threads: 4,
  populationSize: 10,
  mutationRate: 10,
  placementType: "gravity",
  mergeLines: true,
  timeRatio: 0.5,
  simplify: false,
  dxfImportScale: 1,
  dxfExportScale: 72,
  endpointTolerance: 0.36,
  conversionServer: "http://convert.deepnest.io",
};

/**
 * Resolve options into the config object consumed by the SVG importer
 * (`SvgParser`/`DeepNest.importsvg`). `spacing` is expected in canonical
 * coordinate units; the SVG adapter converting SVG units happens before calling
 * the engine.
 */
export function resolveEngineConfig(options = {}) {
  const {
    timeout = 0,
    progressCallback,
    onError,
    units = "inch",
    scale = 72,
    spacing = 0,
    render,
    ...config
  } = options;
  const deepNestConfig = {
    ...DEFAULT_ENGINE_CONFIG,
    ...config,
    units,
    scale,
    spacing,
  };
  return { deepNestConfig, timeout, progressCallback, onError };
}

/**
 * Nest canonical geometry. Returns the abort function.
 *
 * @param {{ sheets: Array, parts: Array }} geometry
 * @param {(payload: object) => any} callback
 * @param {object} [options]
 */
export async function nestGeometry(geometry, callback, options = {}) {
  return nestGeometryIronnest(geometry, callback, options);
}

/**
 * SVG-path variant: `renderContext` carries adapter-owned DOM/render data
 * (kept out of canonical geometry). The ironnest engine is a pure placement
 * oracle and does not consume it, so it is accepted for interface parity only.
 */
export async function nestWithRender(geometry, renderContext, callback, options = {}) {
  return nestGeometryIronnest(geometry, callback, options);
}
