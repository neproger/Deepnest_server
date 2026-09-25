import {
  normalizeGeometry,
  GeometryValidationError,
} from "../geometry/canonical.mjs";
import { parseGeometryInput } from "../geometry/json-adapter.mjs";
import { parseSvgInput } from "../geometry/svg-adapter.mjs";

/**
 * HTTP/Job input layer.
 *
 * Selects an input adapter (`svg` or `geometry`), validates the request, and
 * produces Canonical Geometry + engine options. The Job itself does not know
 * which input format was used.
 */

const MAX_SVG_BYTES = 10 * 1024 * 1024;
const RESERVED_CONFIG_KEYS = ["bin", "progressCallback", "timeout"];

export function httpError(status, code, message) {
  const error = new Error(message);
  error.status = status;
  error.code = code;
  return error;
}

function assertSvg(data, label) {
  if (typeof data !== "string" || data.trim() === "") {
    throw httpError(400, "INVALID_SVG", `${label} must be a non-empty SVG string`);
  }
  if (Buffer.byteLength(data) > MAX_SVG_BYTES) {
    throw httpError(400, "INVALID_SVG", `${label} exceeds the maximum SVG size`);
  }
  let document;
  try {
    document = new DOMParser().parseFromString(data, "image/svg+xml");
  } catch {
    document = null;
  }
  if (
    !document ||
    !document.documentElement ||
    document.documentElement.nodeName.indexOf("parsererror") > -1 ||
    document.documentElement.nodeName.toLowerCase() !== "svg"
  ) {
    throw httpError(400, "INVALID_SVG", `${label} is not valid SVG`);
  }
}

function validateConfig(config) {
  if (
    config !== undefined &&
    (config === null || typeof config !== "object" || Array.isArray(config))
  ) {
    throw httpError(400, "INVALID_CONFIG", "`config` must be an object");
  }
  for (const reserved of RESERVED_CONFIG_KEYS) {
    if (config && Object.prototype.hasOwnProperty.call(config, reserved)) {
      throw httpError(
        400,
        "INVALID_CONFIG",
        `config.${reserved} is reserved and cannot be set`
      );
    }
  }
  return config || {};
}

function validateExecution(execution) {
  const spec = execution === undefined ? {} : execution;
  if (spec === null || typeof spec !== "object" || Array.isArray(spec)) {
    throw httpError(400, "INVALID_CONFIG", "`execution` must be an object");
  }
  if (spec.timeLimitMs !== undefined) {
    if (!Number.isFinite(spec.timeLimitMs) || spec.timeLimitMs <= 0) {
      throw httpError(
        400,
        "INVALID_CONFIG",
        "`execution.timeLimitMs` must be a positive number"
      );
    }
  }
  return spec;
}

function validateSvgInput(input, config, execution) {
  const bin = input.bin;
  if (!bin || typeof bin !== "object" || Array.isArray(bin)) {
    throw httpError(400, "INVALID_REQUEST", "`input.bin` is required");
  }
  if (typeof bin.id !== "string" || bin.id === "") {
    throw httpError(400, "INVALID_REQUEST", "`input.bin.id` is required");
  }
  assertSvg(bin.data, "input.bin.data");

  if (!Array.isArray(input.parts) || input.parts.length === 0) {
    throw httpError(400, "INVALID_REQUEST", "`input.parts` must be a non-empty array");
  }

  const seen = new Set();
  const parts = input.parts.map((part, index) => {
    if (!part || typeof part !== "object" || Array.isArray(part)) {
      throw httpError(400, "INVALID_REQUEST", `input.parts[${index}] must be an object`);
    }
    if (typeof part.id !== "string" || part.id === "") {
      throw httpError(400, "INVALID_REQUEST", `input.parts[${index}].id is required`);
    }
    if (seen.has(part.id)) {
      throw httpError(400, "INVALID_REQUEST", `Duplicate part id: ${part.id}`);
    }
    if (/[\\/]/.test(part.id)) {
      throw httpError(
        400,
        "INVALID_REQUEST",
        `input.parts[${index}].id must not contain path separators`
      );
    }
    seen.add(part.id);
    assertSvg(part.data, `input.parts[${index}].data`);

    const quantity = part.quantity === undefined ? 1 : part.quantity;
    if (!Number.isInteger(quantity) || quantity < 1 || quantity > 1000) {
      throw httpError(
        400,
        "INVALID_REQUEST",
        `input.parts[${index}].quantity must be an integer >= 1`
      );
    }
    return { id: part.id, data: part.data, quantity };
  });

  return {
    format: "svg",
    bin: { id: bin.id, data: bin.data },
    parts,
    config,
    execution,
    sheetId: bin.id,
    units: config.units,
  };
}

function validateGeometryInput(input, config, execution) {
  if (!Array.isArray(input.sheets) || input.sheets.length === 0) {
    throw httpError(
      400,
      "INVALID_REQUEST",
      "`input.sheets` must be a non-empty array"
    );
  }
  if (!Array.isArray(input.parts) || input.parts.length === 0) {
    throw httpError(
      400,
      "INVALID_REQUEST",
      "`input.parts` must be a non-empty array"
    );
  }
  if (input.units !== undefined && typeof input.units !== "string") {
    throw httpError(400, "INVALID_REQUEST", "`input.units` must be a string");
  }

  // Public JSON DTO → internal canonical geometry (validation throws
  // GeometryValidationError, code INVALID_GEOMETRY).
  const geometry = normalizeGeometry(parseGeometryInput(input));

  // Multiple sheets are representable but identity for >1 sheet is not
  // guaranteed yet, so the HTTP contract accepts exactly one for now.
  if (geometry.sheets.length !== 1) {
    throw new GeometryValidationError(
      "exactly one sheet is currently supported by the geometry input"
    );
  }

  return {
    format: "geometry",
    geometry,
    config,
    execution,
    sheetId: geometry.sheets[0].id,
    units: geometry.units,
  };
}

/**
 * Validate a `POST /api/v1/jobs` body. Returns a format-specific spec.
 */
export function validateRequest(body) {
  if (!body || typeof body !== "object" || Array.isArray(body)) {
    throw httpError(400, "INVALID_REQUEST", "Request body must be a JSON object");
  }
  const { input } = body;
  if (!input || typeof input !== "object" || Array.isArray(input)) {
    throw httpError(400, "INVALID_REQUEST", "`input` is required");
  }
  if (input.format !== "svg" && input.format !== "geometry") {
    throw httpError(
      400,
      "INVALID_REQUEST",
      '`input.format` must be "svg" or "geometry"'
    );
  }

  const config = validateConfig(body.config);
  const execution = validateExecution(body.execution);

  return input.format === "svg"
    ? validateSvgInput(input, config, execution)
    : validateGeometryInput(input, config, execution);
}

/** Expand external SVG parts (with quantity) into the `nest()` input array. */
export function toNestInput(spec) {
  const svgInput = [];
  for (const part of spec.parts) {
    for (let i = 0; i < part.quantity; i++) {
      svgInput.push({ file: part.id, svg: part.data });
    }
  }
  return { svgInput, bin: spec.bin.data };
}

function geometryEngineOptions(spec) {
  const units = spec.units ?? spec.config.units ?? "inch";
  const scale = spec.config.scale ?? 72;
  const spacing = spec.config.spacing ?? 0;
  return { ...spec.config, units, scale, spacing };
}

function svgEngineOptions(spec) {
  const units = spec.config.units ?? "inch";
  const scale = spec.config.scale ?? 72;
  const spacing = spec.config.spacing ?? 0;
  const ratio = units === "mm" ? 1 / 25.4 : 1;
  return { ...spec.config, units, scale, spacing: spacing * ratio * scale };
}

/**
 * Select and run the input adapter.
 *
 * @returns {Promise<{ geometry: object, renderContext: object|null, engineOptions: object }>}
 */
export async function adaptInput(spec) {
  if (spec.format === "geometry") {
    return {
      geometry: spec.geometry,
      renderContext: null,
      engineOptions: geometryEngineOptions(spec),
    };
  }

  const { svgInput, bin } = toNestInput(spec);
  const units = spec.config.units ?? "inch";
  const scale = spec.config.scale ?? 72;
  const { geometry, renderContext } = await parseSvgInput(svgInput, {
    ...spec.config,
    bin,
    units,
    scale,
    sheetId: spec.sheetId,
  });
  // Renderer is SVG-specific and loaded only on this path.
  const { nestingToSVG } = await import("../../main/nestingToSVG.mjs");
  renderContext.render = nestingToSVG;

  return { geometry, renderContext, engineOptions: svgEngineOptions(spec) };
}

/**
 * Map internal sheet identity to stable external identity.
 *
 * NOTE: the engine reuses the same polygon object for `quantity > 1` copies of
 * one sheet geometry and overwrites its `id`, so the raw `sheetid` is not a
 * reliable instance counter. We therefore derive `sheetInstanceId` from the
 * order in which the engine opens sheet instances (per sheet type).
 */
export function buildSheetMap(sheets) {
  return sheets.map((sheet) => ({ id: sheet.id, quantity: sheet.quantity }));
}

/**
 * Convert the engine result to the stable external result contract.
 *
 * `instanceId`/`sheetInstanceId` are derived from engine order, so they stay
 * stable across successive best-result updates.
 */
export function toExternalResult(data, status, sheetMap) {
  const counters = new Map();
  const sheetInstanceCounters = new Map();
  const placements = [];

  for (const group of data.placements) {
    const sheet = sheetMap ? sheetMap[group.sheet] : undefined;
    const sheetId = sheet ? sheet.id : String(group.sheet);
    const sheetInstanceId = sheetInstanceCounters.get(group.sheet) ?? 0;
    sheetInstanceCounters.set(group.sheet, sheetInstanceId + 1);

    for (const placement of group.sheetplacements
      .slice()
      .sort((a, b) => a.id - b.id)) {
      const partId = placement.filename;
      const instanceId = counters.get(partId) ?? 0;
      counters.set(partId, instanceId + 1);
      placements.push({
        partId,
        instanceId,
        sheetId,
        sheetInstanceId,
        x: placement.x,
        y: placement.y,
        rotation: placement.rotation,
        ...(placement.mergedLength !== undefined && {
          mergedLength: placement.mergedLength,
        }),
        ...(placement.mergedSegments !== undefined && {
          mergedSegments: placement.mergedSegments,
        }),
        raw: {
          id: placement.id,
          source: placement.source,
          filename: placement.filename,
          sheet: group.sheet,
          sheetid: group.sheetid,
        },
      });
    }
  }

  return {
    fitness: data.fitness,
    area: data.area,
    mergedLength: data.mergedLength,
    index: data.index,
    placements,
    status,
  };
}
