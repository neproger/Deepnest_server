import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const ClipperLib = require("../../main/util/clipper.js");

// Clipper works on integers; 1000 keeps sub-mm precision for the canonical (mm) space.
const SCALE = 1000;

const POLYGON_ELEMENTS = new Set([
  "path",
  "polygon",
  "polyline",
  "rect",
  "circle",
  "ellipse",
]);

function toClipper(points) {
  return points.map((point) => ({
    X: Math.round(point.x * SCALE),
    Y: Math.round(point.y * SCALE),
  }));
}

function signedArea(path) {
  return ClipperLib.Clipper.Area(path) / (SCALE * SCALE);
}

function pointInPath(point, path) {
  return (
    ClipperLib.Clipper.PointInPolygon(
      { X: Math.round(point.x * SCALE), Y: Math.round(point.y * SCALE) },
      path
    ) !== 0
  );
}

function isFilled(element) {
  const fill = (element.getAttribute("fill") || "").trim().toLowerCase();
  if (fill === "none" || fill === "transparent") {
    return false;
  }
  const opacity = (element.getAttribute("fill-opacity") || "").trim();
  if (opacity !== "" && Number(opacity) === 0) {
    return false;
  }
  return true;
}

function fillRuleOf(element) {
  return (element.getAttribute("fill-rule") || "nonzero").trim().toLowerCase();
}

function unionGroup(paths, fillRule) {
  const clipper = new ClipperLib.Clipper();
  clipper.AddPaths(paths, ClipperLib.PolyType.ptSubject, true);
  const fill =
    fillRule === "evenodd"
      ? ClipperLib.PolyFillType.pftEvenOdd
      : ClipperLib.PolyFillType.pftNonZero;
  const solution = new ClipperLib.Paths();
  clipper.Execute(ClipperLib.ClipType.ctUnion, solution, fill, fill);
  return solution;
}

/**
 * Turn an SVG's polygon elements into the part's *material*: the union of every
 * filled region (respecting each element's fill-rule and grouping compound paths),
 * then split into regions ({ points, children }).
 *
 * Filled = body, unfilled = hole. Nested/overlapping art collapses correctly
 * (e.g. concentric rings become several regions), so the engine never needs
 * containment trees deeper than one hole level.
 *
 * @param {Array} elements cleaned DOM polygon elements (with fill/data-deepnest-group)
 * @param {(element: object) => Array} polygonify svgparser.polygonify
 * @returns {Array<{points: Array, children: Array}>}
 */
export function buildMaterialRegions(elements, polygonify) {
  const groups = new Map();

  for (const element of elements) {
    if (!"tagName" in element || !POLYGON_ELEMENTS.has(element.tagName)) {
      continue;
    }
    if (!isFilled(element)) {
      continue;
    }
    const poly = polygonify(element);
    if (!poly || poly.length < 3) {
      continue;
    }
    const groupId =
      element.getAttribute("data-deepnest-group") ||
      `element-${groups.size}`;
    if (!groups.has(groupId)) {
      groups.set(groupId, { fillRule: fillRuleOf(element), paths: [] });
    }
    groups.get(groupId).paths.push(toClipper(poly));
  }

  const allPaths = new ClipperLib.Paths();
  for (const group of groups.values()) {
    const filled = unionGroup(group.paths, group.fillRule);
    for (const path of filled) {
      allPaths.push(path);
    }
  }
  if (allPaths.length === 0) {
    return [];
  }

  const union = new ClipperLib.Clipper();
  union.AddPaths(allPaths, ClipperLib.PolyType.ptSubject, true);
  const material = new ClipperLib.Paths();
  union.Execute(
    ClipperLib.ClipType.ctUnion,
    material,
    ClipperLib.PolyFillType.pftNonZero,
    ClipperLib.PolyFillType.pftNonZero
  );

  const outers = [];
  const holes = [];
  for (const path of material) {
    if (path.length < 3) {
      continue;
    }
    const area = signedArea(path);
    if (area >= 0) {
      outers.push({ path, area, children: [] });
    } else {
      holes.push({ path, area });
    }
  }

  const toPoints = (path) =>
    path.map((p) => ({ x: p.X / SCALE, y: p.Y / SCALE }));

  // Region 0 must be the LARGEST body: the engine uses the primary outer for the
  // sheet inner-fit and as the placement reference. Clipper's output order is not
  // guaranteed, so sort by descending area.
  outers.sort((a, b) => Math.abs(b.area) - Math.abs(a.area));

  for (const hole of holes) {
    // Attach the hole to the smallest outer that contains it.
    let best = null;
    for (const outer of outers) {
      if (Math.abs(hole.area) >= Math.abs(outer.area)) {
        continue;
      }
      if (pointInPath({ x: hole.path[0].X / SCALE, y: hole.path[0].Y / SCALE }, outer.path)) {
        if (!best || Math.abs(outer.area) < Math.abs(best.area)) {
          best = outer;
        }
      }
    }
    if (best) {
      const holeTree = toPoints(hole.path);
      holeTree.children = [];
      best.children.push(holeTree);
    }
  }

  return outers.map((outer) => {
    const tree = toPoints(outer.path);
    tree.children = outer.children;
    return tree;
  });
}
