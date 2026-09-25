/**
 * HTTP/Job input adapter: validates an external job request and converts it to
 * the existing `nest()` inputs, then converts engine results back to stable,
 * client-facing placements.
 *
 * The Job Manager and the HTTP layer must not know about engine internals
 * (`source`, `id`, `filename`, `sheetid`). This module owns that boundary.
 */

const MAX_SVG_BYTES = 10 * 1024 * 1024;

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

/**
 * @param {unknown} body
 * @returns {{ bin: {id: string, data: string}, parts: {id: string, data: string, quantity: number}[], config: object, execution: {timeLimitMs?: number} }}
 */
export function validateRequest(body) {
  if (!body || typeof body !== "object" || Array.isArray(body)) {
    throw httpError(400, "INVALID_REQUEST", "Request body must be a JSON object");
  }

  const { input, config, execution } = body;

  if (!input || typeof input !== "object" || Array.isArray(input)) {
    throw httpError(400, "INVALID_REQUEST", "`input` is required");
  }
  if (input.format !== "svg") {
    throw httpError(400, "INVALID_REQUEST", "`input.format` must be \"svg\"");
  }

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
    // The core adapter passes the id as a `file` name, so keep it path-safe.
    if (/[\\/]/.test(part.id)) {
      throw httpError(
        400,
        "INVALID_REQUEST",
        `input.parts[${index}].id must not contain path separators`
      );
    }
    if (seen.has(part.id)) {
      throw httpError(400, "INVALID_REQUEST", `Duplicate part id: ${part.id}`);
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

  if (
    config !== undefined &&
    (config === null || typeof config !== "object" || Array.isArray(config))
  ) {
    throw httpError(400, "INVALID_CONFIG", "`config` must be an object");
  }

  // These are owned by the Job adapter, not the engine config.
  for (const reserved of ["bin", "progressCallback", "timeout"]) {
    if (config && Object.prototype.hasOwnProperty.call(config, reserved)) {
      throw httpError(
        400,
        "INVALID_CONFIG",
        `config.${reserved} is reserved and cannot be set`
      );
    }
  }

  const executionSpec = execution === undefined ? {} : execution;
  if (
    executionSpec === null ||
    typeof executionSpec !== "object" ||
    Array.isArray(executionSpec)
  ) {
    throw httpError(400, "INVALID_CONFIG", "`execution` must be an object");
  }
  if (executionSpec.timeLimitMs !== undefined) {
    const limit = executionSpec.timeLimitMs;
    if (!Number.isFinite(limit) || limit <= 0) {
      throw httpError(
        400,
        "INVALID_CONFIG",
        "`execution.timeLimitMs` must be a positive number"
      );
    }
  }

  return {
    bin: { id: bin.id, data: bin.data },
    parts,
    config: config || {},
    execution: executionSpec,
  };
}

/**
 * Expand external parts (with quantity) into the `nest()` `svgInput` array.
 * Each part is passed as `{ file: partId, svg }` so the engine carries the
 * external id through as `filename`.
 */
export function toNestInput({ bin, parts }) {
  const svgInput = [];
  for (const part of parts) {
    for (let i = 0; i < part.quantity; i++) {
      svgInput.push({ file: part.id, svg: part.data });
    }
  }
  return { svgInput, bin: bin.data };
}

/**
 * Convert the engine result to the stable external result contract.
 *
 * `instanceId` is derived from the engine's globally unique nested-instance
 * `id`, so it stays stable across successive best-result updates for a job.
 */
export function toExternalResult(data, status, binId) {
  const flat = data.placements
    .flatMap((sheet) => sheet.sheetplacements)
    .slice()
    .sort((a, b) => a.id - b.id);

  const counters = new Map();
  const placements = flat.map((placement) => {
    const partId = placement.filename;
    const instanceId = counters.get(partId) ?? 0;
    counters.set(partId, instanceId + 1);
    return {
      partId,
      instanceId,
      sheetId: binId,
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
      },
    };
  });

  return {
    fitness: data.fitness,
    area: data.area,
    mergedLength: data.mergedLength,
    index: data.index,
    placements,
    status,
  };
}
