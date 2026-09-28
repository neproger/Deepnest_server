/**
 * Canonical Geometry — the engine-facing geometry boundary.
 *
 * This module defines, validates and normalises the plain polygon-tree model
 * that the nesting engine actually consumes. It is intentionally polygonal
 * (no lines/arcs/beziers) and has no dependency on SVG, DOM or HTTP.
 *
 * Canonical shape:
 *
 *   {
 *     units?: string,                 // metadata only, never used by the math
 *     sheets: [                       // at least one; `bin` is accepted as alias
 *       { id, polygontree, quantity? }
 *     ],
 *     parts: [
 *       { id, polygontree, quantity? }
 *     ]
 *   }
 *
 * A polygon tree is an array of `{ x, y }` points with an optional nested
 * `children` array. Runtime/engine state (`source`, `id`, `rotation`, `exact`,
 * `parent`, GA/NFP state) is NOT part of canonical geometry: the engine builds
 * it. See docs/GEOMETRY_PIPELINE.md.
 *
 * Units: canonical coordinates are an arbitrary but consistent unit. `spacing`,
 * `curveTolerance` and sheet dimensions are expressed in that same unit.
 * `units` is descriptive metadata only.
 */

export const CANONICAL_VERSION = 1;

export class GeometryValidationError extends Error {
  constructor(message) {
    super(message);
    this.name = "GeometryValidationError";
    this.code = "INVALID_GEOMETRY";
    this.status = 400;
  }
}

function fail(message) {
  throw new GeometryValidationError(message);
}

function assertId(id, where) {
  if (typeof id !== "string" || id === "") {
    fail(`${where} must be a non-empty string id`);
  }
}

function assertQuantity(quantity, where) {
  if (quantity === undefined) {
    return 1;
  }
  if (!Number.isInteger(quantity) || quantity < 1) {
    fail(`${where}.quantity must be an integer >= 1`);
  }
  return quantity;
}

/**
 * Validate a polygon tree (array of points with nested `children`).
 * `minPoints` guards degenerate polygons; canonical input is assumed to be
 * already prepared (no self-intersection repair, snapping or healing).
 */
export function validatePolygonTree(polygon, where, minPoints = 3) {
  if (!Array.isArray(polygon)) {
    fail(`${where} must be an array of points`);
  }
  if (polygon.length < minPoints) {
    fail(`${where} must contain at least ${minPoints} points`);
  }
  for (let i = 0; i < polygon.length; i++) {
    const point = polygon[i];
    if (!point || typeof point !== "object") {
      fail(`${where}[${i}] must be an { x, y } point`);
    }
    if (!Number.isFinite(point.x) || !Number.isFinite(point.y)) {
      fail(`${where}[${i}] must have finite numeric x and y`);
    }
  }
  if (polygon.children !== undefined) {
    if (!Array.isArray(polygon.children)) {
      fail(`${where}.children must be an array`);
    }
    polygon.children.forEach((child, i) =>
      validatePolygonTree(child, `${where}.children[${i}]`)
    );
  }
  return polygon;
}

function normalizeEntry(entry, where) {
  if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
    fail(`${where} must be an object`);
  }
  assertId(entry.id, where);
  validatePolygonTree(entry.polygontree, `${where}.polygontree`);
  const quantity = assertQuantity(entry.quantity, where);
  return { id: entry.id, quantity, polygontree: entry.polygontree };
}

/**
 * Validate and normalise canonical geometry. Accepts `bin` as an alias for a
 * single-entry `sheets` array. Throws `GeometryValidationError` on bad input.
 */
export function normalizeGeometry(geometry) {
  if (!geometry || typeof geometry !== "object" || Array.isArray(geometry)) {
    fail("geometry must be an object");
  }

  const rawSheets =
    geometry.sheets !== undefined
      ? geometry.sheets
      : geometry.bin !== undefined
        ? [geometry.bin]
        : undefined;

  if (!Array.isArray(rawSheets) || rawSheets.length === 0) {
    fail("geometry.sheets (or geometry.bin) must provide at least one sheet");
  }
  if (!Array.isArray(geometry.parts) || geometry.parts.length === 0) {
    fail("geometry.parts must be a non-empty array");
  }

  const sheets = rawSheets.map((sheet, i) =>
    normalizeEntry(sheet, `geometry.sheets[${i}]`)
  );
  const parts = geometry.parts.map((part, i) =>
    normalizeEntry(part, `geometry.parts[${i}]`)
  );

  const units = geometry.units;
  if (units !== undefined && typeof units !== "string") {
    fail("geometry.units must be a string when present");
  }

  return { units, sheets, parts };
}

/**
 * Deep-clone a polygon tree into a plain canonical tree. Used before handing
 * geometry to the engine, which mutates polygons in place (spacing offsets).
 */
export function clonePolygonTree(polygon) {
  const clone = [];
  for (let i = 0; i < polygon.length; i++) {
    const point = polygon[i];
    clone.push({ x: point.x, y: point.y });
  }
  if (Array.isArray(polygon.children) && polygon.children.length > 0) {
    clone.children = polygon.children.map(clonePolygonTree);
  }
  return clone;
}
