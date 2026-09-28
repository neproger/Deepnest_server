import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

// svgparser pulls in a browser SVGPathSeg polyfill that touches `window`; load it
// lazily so this module stays importable without DOM globals until parsing runs.
let SvgParser = null;
function svgParser() {
  if (!SvgParser) {
    SvgParser = require("./svgparser");
  }
  return SvgParser;
}

const ClipperLib = require("./util/clipper");
const { GeometryUtil } = require("./util/geometryutil");

/**
 * SVG importer (parser only — no nesting engine).
 *
 * Extracted from the original Deepnest `importsvg`/`getParts`: turns an SVG
 * string into parts `{ polygontree, svgelements, bounds, area, quantity, filename }`
 * where `polygontree` is a plain polygon tree (array of `{x,y}` with `children`
 * holes). Used by the SVG → canonical-geometry adapter.
 *
 * @param {object} configuration engine config (uses `scale`, `curveTolerance`, `clipperScale`)
 * @returns {{ importsvg: (filename: string|null, dirpath: string|null, svg: string, scalingFactor?: number, dxfFlag?: boolean) => Array }}
 */
export function createImporter(configuration = {}) {
  const config = {
    clipperScale: 10000000,
    curveTolerance: 0.3,
    scale: 72,
    ...configuration,
  };

  function svgToClipper(polygon, scale) {
    const clip = [];
    for (let i = 0; i < polygon.length; i++) {
      clip.push({ X: polygon[i].x, Y: polygon[i].y });
    }
    ClipperLib.JS.ScaleUpPath(clip, scale || config.clipperScale);
    return clip;
  }

  function clipperToSvg(polygon) {
    const normal = [];
    for (let i = 0; i < polygon.length; i++) {
      normal.push({
        x: polygon[i].X / config.clipperScale,
        y: polygon[i].Y / config.clipperScale,
      });
    }
    return normal;
  }

  function pointInPolygon(point, polygon) {
    // scaling is deliberately coarse to filter out points that lie *on* the polygon
    const p = svgToClipper(polygon, 1000);
    const pt = new ClipperLib.IntPoint(1000 * point.x, 1000 * point.y);
    return ClipperLib.Clipper.PointInPolygon(pt, p) > 0;
  }

  // returns a less complex polygon that satisfies the curve tolerance
  function cleanPolygon(polygon) {
    const p = svgToClipper(polygon);
    // remove self-intersections and find the biggest polygon that's left
    const simple = ClipperLib.Clipper.SimplifyPolygon(
      p,
      ClipperLib.PolyFillType.pftNonZero
    );

    if (!simple || simple.length === 0) {
      return null;
    }

    let biggest = simple[0];
    let biggestarea = Math.abs(ClipperLib.Clipper.Area(biggest));
    for (let i = 1; i < simple.length; i++) {
      const area = Math.abs(ClipperLib.Clipper.Area(simple[i]));
      if (area > biggestarea) {
        biggest = simple[i];
        biggestarea = area;
      }
    }

    // clean up singularities, coincident points and edges
    const clean = ClipperLib.Clipper.CleanPolygon(
      biggest,
      0.01 * config.curveTolerance * config.clipperScale
    );

    if (!clean || clean.length === 0) {
      return null;
    }

    const cleaned = clipperToSvg(clean);

    // remove duplicate endpoints
    const start = cleaned[0];
    const end = cleaned[cleaned.length - 1];
    if (
      start === end ||
      (GeometryUtil.almostEqual(start.x, end.x) &&
        GeometryUtil.almostEqual(start.y, end.y))
    ) {
      cleaned.pop();
    }

    return cleaned;
  }

  // assuming no intersections, return a tree where odd leaves are parts and even ones are holes
  function getParts(paths, filename) {
    let i, j;
    const polygons = [];

    const numChildren = paths.length;
    for (i = 0; i < numChildren; i++) {
      if (svgParser().polygonElements.indexOf(paths[i].tagName) < 0) {
        continue;
      }

      // don't use open paths
      if (!svgParser().isClosed(paths[i], 2 * config.curveTolerance)) {
        continue;
      }

      let poly = svgParser().polygonify(paths[i]);
      poly = cleanPolygon(poly);

      if (
        poly &&
        poly.length > 2 &&
        Math.abs(GeometryUtil.polygonArea(poly)) >
          config.curveTolerance * config.curveTolerance
      ) {
        poly.source = i;
        polygons.push(poly);
      } else {
        console.warn("Excluding poly", poly, paths[i]);
      }
    }

    // turn the list into a tree
    // root level nodes of the tree are parts
    toTree(polygons);

    function toTree(list, idstart) {
      function svgToClipperLocal(polygon) {
        const clip = [];
        for (let i = 0; i < polygon.length; i++) {
          clip.push({ X: polygon[i].x, Y: polygon[i].y });
        }
        ClipperLib.JS.ScaleUpPath(clip, config.clipperScale);
        return clip;
      }
      function pointInClipperPolygon(point, polygon) {
        const pt = new ClipperLib.IntPoint(
          config.clipperScale * point.x,
          config.clipperScale * point.y
        );
        return ClipperLib.Clipper.PointInPolygon(pt, polygon) > 0;
      }
      const parents = [];
      let i, j, k;

      // assign a unique id to each leaf
      let id = idstart || 0;

      for (i = 0; i < list.length; i++) {
        const p = list[i];
        let ischild = false;
        for (j = 0; j < list.length; j++) {
          if (j === i) {
            continue;
          }
          if (p.length < 2) {
            continue;
          }
          let inside = 0;
          const fullinside = Math.min(10, p.length);

          // sample about 10 points
          const clipper_polygon = svgToClipperLocal(list[j]);

          for (k = 0; k < fullinside; k++) {
            if (pointInClipperPolygon(p[k], clipper_polygon) === true) {
              inside++;
            }
          }

          if (inside > 0.5 * fullinside) {
            if (!list[j].children) {
              list[j].children = [];
            }
            list[j].children.push(p);
            p.parent = list[j];
            ischild = true;
            break;
          }
        }

        if (!ischild) {
          parents.push(p);
        }
      }

      for (i = 0; i < list.length; i++) {
        if (parents.indexOf(list[i]) < 0) {
          list.splice(i, 1);
          i--;
        }
      }

      for (i = 0; i < parents.length; i++) {
        parents[i].id = id;
        id++;
      }

      for (i = 0; i < parents.length; i++) {
        if (parents[i].children) {
          id = toTree(parents[i].children, id);
        }
      }

      return id;
    }

    function findElementById(id, tree) {
      if (id === tree.source) {
        return true;
      }
      if (tree.children && tree.children.length > 0) {
        for (let i = 0; i < tree.children.length; i++) {
          if (findElementById(id, tree.children[i])) {
            return true;
          }
        }
      }
      return false;
    }

    // construct part objects with metadata
    const parts = [];
    const svgelements = Array.prototype.slice.call(paths);
    const openelements = svgelements.slice();

    for (i = 0; i < polygons.length; i++) {
      const part = {};
      part.polygontree = polygons[i];
      part.svgelements = [];

      const bounds = GeometryUtil.getPolygonBounds(part.polygontree);
      part.bounds = bounds;
      part.area = bounds.width * bounds.height;
      part.quantity = 1;
      part.filename = filename;

      if (part.filename === "BACKGROUND.svg") {
        part.sheet = true;
      }

      // load root element
      part.svgelements.push(svgelements[part.polygontree.source]);
      let index = openelements.indexOf(svgelements[part.polygontree.source]);
      if (index > -1) {
        openelements.splice(index, 1);
      }

      // load all elements that lie within the outer polygon
      for (j = 0; j < svgelements.length; j++) {
        if (j !== part.polygontree.source && findElementById(j, part.polygontree)) {
          part.svgelements.push(svgelements[j]);
          index = openelements.indexOf(svgelements[j]);
          if (index > -1) {
            openelements.splice(index, 1);
          }
        }
      }

      parts.push(part);
    }

    // include open segments that lie within the part boundaries
    for (i = 0; i < parts.length; i++) {
      const part = parts[i];
      for (j = 0; j < openelements.length; j++) {
        const el = openelements[j];
        if (el.tagName === "line") {
          const x1 = Number(el.getAttribute("x1"));
          const x2 = Number(el.getAttribute("x2"));
          const y1 = Number(el.getAttribute("y1"));
          const y2 = Number(el.getAttribute("y2"));
          const start = { x: x1, y: y1 };
          const end = { x: x2, y: y2 };
          const mid = { x: (start.x + end.x) / 2, y: (start.y + end.y) / 2 };

          if (
            pointInPolygon(start, part.polygontree) === true ||
            pointInPolygon(end, part.polygontree) === true ||
            pointInPolygon(mid, part.polygontree) === true
          ) {
            part.svgelements.push(el);
            openelements.splice(j, 1);
            j--;
          }
        } else if (el.tagName === "image") {
          const x = Number(el.getAttribute("x"));
          const y = Number(el.getAttribute("y"));
          const width = Number(el.getAttribute("width"));
          const height = Number(el.getAttribute("height"));

          const mid = { x: x + width / 2, y: y + height / 2 };

          const transformString = el.getAttribute("transform");
          if (transformString) {
            const transform = svgParser().transformParse(transformString);
            if (transform) {
              const transformed = transform.calc(mid.x, mid.y);
              mid.x = transformed[0];
              mid.y = transformed[1];
            }
          }
          // just test midpoint for images
          if (pointInPolygon(mid, part.polygontree) === true) {
            part.svgelements.push(el);
            openelements.splice(j, 1);
            j--;
          }
        } else if (el.tagName === "path" || el.tagName === "polyline") {
          let p;
          if (el.tagName === "path") {
            p = svgParser().polygonifyPath(el);
          } else {
            p = [];
            for (let k = 0; k < el.points.length; k++) {
              p.push({ x: el.points[k].x, y: el.points[k].y });
            }
          }

          if (p.length < 2) {
            continue;
          }

          let found = false;
          let next = p[1];
          for (let k = 0; k < p.length; k++) {
            if (pointInPolygon(p[k], part.polygontree) === true) {
              found = true;
              break;
            }

            if (k >= p.length - 1) {
              next = p[0];
            } else {
              next = p[k + 1];
            }

            // also test for midpoints in case of single line edge case
            const mid = { x: (p[k].x + next.x) / 2, y: (p[k].y + next.y) / 2 };
            if (pointInPolygon(mid, part.polygontree) === true) {
              found = true;
              break;
            }
          }
          if (found) {
            part.svgelements.push(el);
            openelements.splice(j, 1);
            j--;
          }
        }
      }
    }

    return parts;
  }

  function importsvg(filename, dirpath, svgstring, scalingFactor, dxfFlag) {
    // config.scale is the default scale, and may not be applied;
    // scalingFactor is an absolute scaling applied regardless of the SVG.
    let svg = svgParser().load(dirpath, svgstring, config.scale, scalingFactor);
    svg = svgParser().clean(dxfFlag);
    return getParts(svg.children, filename);
  }

  return { importsvg, getParts };
}
