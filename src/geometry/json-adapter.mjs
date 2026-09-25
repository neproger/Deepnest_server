import { GeometryValidationError } from "./canonical.mjs";

/**
 * Public JSON Geometry DTO → internal Canonical Geometry.
 *
 * The public HTTP contract must be plain, serialisable JSON. Internal canonical
 * geometry represents a polygon as a JS Array with array properties
 * (`polygon.children`), which JSON cannot express naturally. So the transport
 * representation is:
 *
 *   PolygonDTO { points: PointDTO[], children?: PolygonDTO[] }
 *   PointDTO   { x: number, y: number }
 *
 * and this adapter converts it to the internal canonical polygon tree. The two
 * representations are intentionally kept separate.
 */

function fail(message) {
  throw new GeometryValidationError(message);
}

function toPolygon(dto, where) {
  if (!dto || typeof dto !== "object" || Array.isArray(dto)) {
    fail(`${where} must be an object with a points array`);
  }
  if (!Array.isArray(dto.points)) {
    fail(`${where}.points must be an array`);
  }
  const polygon = dto.points.map((point, index) => {
    if (!point || typeof point !== "object" || Array.isArray(point)) {
      fail(`${where}.points[${index}] must be an { x, y } point`);
    }
    if (!Number.isFinite(point.x) || !Number.isFinite(point.y)) {
      fail(`${where}.points[${index}] must have finite numeric x and y`);
    }
    return { x: point.x, y: point.y };
  });
  if (polygon.length < 3) {
    fail(`${where}.points must contain at least 3 points`);
  }
  if (dto.children !== undefined) {
    if (!Array.isArray(dto.children)) {
      fail(`${where}.children must be an array`);
    }
    polygon.children = dto.children.map((child, index) =>
      toPolygon(child, `${where}.children[${index}]`)
    );
  }
  return polygon;
}

function toEntry(entry, where) {
  if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
    fail(`${where} must be an object`);
  }
  return {
    id: entry.id,
    quantity: entry.quantity,
    mode: entry.mode,
    polygontree: toPolygon(entry.polygontree, `${where}.polygontree`),
  };
}

/**
 * @returns canonical geometry `{ units, sheets, parts }` (not yet normalised).
 */
export function parseGeometryInput(input) {
  return {
    units: input.units,
    sheets: (input.sheets || []).map((sheet, index) =>
      toEntry(sheet, `input.sheets[${index}]`)
    ),
    parts: (input.parts || []).map((part, index) =>
      toEntry(part, `input.parts[${index}]`)
    ),
  };
}
