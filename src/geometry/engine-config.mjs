/**
 * Engine configuration defaults and option resolution.
 *
 * These used to live in the Deepnest engine adapter; they are now engine-neutral
 * (the SVG importer and the engine list both consume them). `spacing` is in
 * canonical coordinate units — the SVG adapter converts SVG units before the
 * engine is called.
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
 * Split engine options into the pass-through config plus the adapter-owned
 * fields (`timeout`, `progressCallback`, `onError`).
 */
export function resolveEngineConfig(options = {}) {
  const {
    timeout = 0,
    progressCallback,
    onError,
    engine,
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
