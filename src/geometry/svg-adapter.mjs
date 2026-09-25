import path from "path";
import { DeepNest } from "../../main/deepnest.js";
import { clonePolygonTree } from "./canonical.mjs";
import { resolveEngineConfig } from "./engine.mjs";

/**
 * SVG → Canonical Geometry adapter.
 *
 * Converts SVG input into canonical geometry (plain polygon trees) plus a
 * separate adapter-owned `renderContext` (DOM elements/bounds) that never
 * enters canonical geometry. `main/svgparser.js` and jsdom are used unchanged.
 *
 * `nest()` composes this adapter with the engine; HTTP/Job code does not need
 * to know the details.
 *
 * @param {(string | { file: string, svg: string })[]} svgInput
 * @param {object} options same options accepted by `nest()`
 * @returns {{ geometry: object, renderContext: { entries: object[] } }}
 */
export async function parseSvgInput(svgInput, options = {}) {
  const { units = "inch", scale = 72 } = options;
  const ratio = units === "mm" ? 1 / 25.4 : 1;
  const { deepNestConfig } = resolveEngineConfig(options);

  // Throwaway importer: svgparser needs a DeepNest instance for config/scale.
  const importer = new DeepNest(new EventTarget(), deepNestConfig);

  const bin = options.bin;
  const binSvg =
    typeof bin === "object"
      ? `<svg xmlns="http://www.w3.org/2000/svg"><rect x="0" y="0" width="${
          bin.width * ratio * scale
        }" height="${bin.height * ratio * scale}" class="sheet"/></svg>`
      : /<svg[\s>]/i.test(bin)
        ? bin
        : `<svg xmlns="http://www.w3.org/2000/svg">${bin}</svg>`;

  const [sheetPart] = importer.importsvg(null, null, binSvg);
  if (!sheetPart) {
    throw new Error("Nothing to nest");
  }
  sheetPart.sheet = true;

  const elements = svgInput
    .map((input) =>
      typeof input === "object"
        ? importer.importsvg(
            path.basename(input.file),
            path.dirname(input.file),
            input.svg
          )
        : importer.importsvg(null, null, input)
    )
    .flat();
  if (elements.length === 0) {
    throw new Error("Nothing to nest");
  }

  const sheetId = options.sheetId ?? "sheet-0";
  const entries = [];
  const parts = [];

  entries.push({
    svgelements: sheetPart.svgelements,
    bounds: sheetPart.bounds,
  });

  elements.forEach((part, index) => {
    const id = part.filename === null ? `part-${index}` : part.filename;
    parts.push({
      id,
      quantity: 1,
      polygontree: clonePolygonTree(part.polygontree),
    });
    entries.push({ svgelements: part.svgelements, bounds: part.bounds });
  });

  return {
    geometry: {
      units,
      sheets: [
        { id: sheetId, quantity: 1, polygontree: clonePolygonTree(sheetPart.polygontree) },
      ],
      parts,
    },
    renderContext: { entries },
  };
}
