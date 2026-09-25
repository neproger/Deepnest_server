import { nestingToSVG } from "./main/nestingToSVG.mjs";
import { parseSvgInput } from "./src/geometry/svg-adapter.mjs";
import { nestGeometry, nestWithRender } from "./src/geometry/engine.mjs";

/**
 * Nest SVG input. Thin composer: SVG → Canonical Geometry → engine.
 *
 * The public behaviour (progress/result/abort semantics, callback shape) is
 * unchanged; internally the geometry now crosses an explicit canonical
 * boundary before reaching the engine.
 *
 * @param {(string | { file: string, svg: string })[]} svgInput
 * @param {(payload: object) => any} callback
 * @param {import(".").NestingOptions} options
 * @returns {Promise<() => Promise<void>>}
 */
export async function nest(svgInput, callback, options = {}) {
  const { units = "inch", scale = 72, spacing = 0 } = options;
  // SVG units → canonical coordinate units (the engine treats `spacing` as
  // already canonical).
  const ratio = units === "mm" ? 1 / 25.4 : 1;

  const { geometry, renderContext } = await parseSvgInput(svgInput, options);
  renderContext.render = nestingToSVG;

  return nestWithRender(geometry, renderContext, callback, {
    ...options,
    spacing: spacing * ratio * scale,
  });
}

export { nestGeometry };
