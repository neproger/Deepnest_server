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

  const importedGroups = svgInput.map((input, index) => {
    const imported = typeof input === "object"
      ? importer.importsvg(
          path.basename(input.file),
          path.dirname(input.file),
          input.svg
        )
      : importer.importsvg(null, null, input);
    if (imported.length === 0) {
      return null;
    }
    return {
      id: typeof input === "object" ? input.file : `part-${index}`,
      imported,
      rigid: typeof input === "object" && input.rigid === true,
    };
  }).filter(Boolean);
  if (importedGroups.length === 0) {
    throw new Error("Nothing to nest");
  }
  const partGroups = importedGroups.flatMap(({ id, imported, rigid }) =>
    rigid
      ? [{ id, imported }]
      : imported.map((part, rootIndex) => ({
          id: imported.length === 1 ? id : `${id}#${rootIndex + 1}`,
          imported: [part],
        }))
  );

  const sheetId = options.sheetId ?? "sheet-0";
  const entries = [];
  const parts = [];
  const previewParts = [];

  entries.push({
    svgelements: sheetPart.svgelements,
    bounds: sheetPart.bounds,
  });

  partGroups.forEach(({ id, imported }) => {
    const tree = imported.length === 1
      ? clonePolygonTree(imported[0].polygontree)
      : convexHullTree(imported.map((part) => part.polygontree));
    parts.push({
      id,
      quantity: 1,
      polygontree: tree,
    });
    entries.push({
      svgelements: imported.flatMap((part) => part.svgelements),
      bounds: polygonBounds(tree),
    });
    previewParts.push({
      id,
      polygontrees: imported.map((part) => clonePolygonTree(part.polygontree)),
    });
  });

  return {
    geometry: {
      units,
      sheets: [
        { id: sheetId, quantity: 1, polygontree: clonePolygonTree(sheetPart.polygontree) },
      ],
      parts,
    },
    renderContext: { entries, previewParts },
  };
}

function convexHullTree(trees) {
  const points = trees.flatMap((tree) =>
    tree.map(({ x, y }) => ({ x, y }))
  );
  points.sort((a, b) => a.x - b.x || a.y - b.y);
  const unique = points.filter((point, index) =>
    index === 0 ||
    point.x !== points[index - 1].x ||
    point.y !== points[index - 1].y
  );
  if (unique.length < 3) {
    throw new Error("An SVG part must contain at least three distinct points");
  }
  const cross = (a, b, c) =>
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
  const lower = [];
  for (const point of unique) {
    while (lower.length >= 2 && cross(lower.at(-2), lower.at(-1), point) <= 0) {
      lower.pop();
    }
    lower.push(point);
  }
  const upper = [];
  for (let index = unique.length - 1; index >= 0; index -= 1) {
    const point = unique[index];
    while (upper.length >= 2 && cross(upper.at(-2), upper.at(-1), point) <= 0) {
      upper.pop();
    }
    upper.push(point);
  }
  lower.pop();
  upper.pop();
  const hull = lower.concat(upper);
  hull.children = [];
  return hull;
}

function polygonBounds(polygon) {
  const xs = polygon.map((point) => point.x);
  const ys = polygon.map((point) => point.y);
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return {
    x,
    y,
    width: Math.max(...xs) - x,
    height: Math.max(...ys) - y,
  };
}
