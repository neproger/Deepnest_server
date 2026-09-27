// Worker-thread driver for the SVGnest Rust/WASM nesting core.
//
// The vendored `polygon-packer.wasm` is an alpha-stage engine and can emit a
// Rust `unreachable` panic on some inputs. Running it in a worker keeps a panic
// from taking down the server and lets `abort()` terminate it.
//
// We use the patched `wasm_packer_init_trees` entry so the engine receives the
// explicit part/hole hierarchy from canonical geometry instead of inferring it
// by global containment (which would merge unrelated overlapping parts). The
// engine opens as many bins as needed internally (one `placementCount` group
// per bin, mapped to a sheet instance); the outer loop re-runs any parts that
// still did not fit in a fresh instance.
//
// Known upstream limitation: for some rotation combinations of identical
// geometry the NFP can allow overlapping placements, which the GA may prefer.
import { parentPort, workerData } from "node:worker_threads";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import WasmNesting from "./loader.mjs";

globalThis.self ??= globalThis;
globalThis.window ??= globalThis;

const {
  config,
  bin, // number[]
  parts, // [{ id, tree: { ring: number[], children: [...] } }]
  maxSheets = 1,
  maxGenerationsPerSheet = 40,
  chunkSize = 512,
} = workerData;

let stopRequested = false;
parentPort.on("message", (message) => {
  if (message && message.type === "stop") {
    stopRequested = true;
  }
});

const total = parts.length;
let nextInstanceId = 0;

// Serialize explicit polygon trees into the `PolygonNode` format expected by
// `wasm_packer_init_trees`: [rootCount] then, per node, source+1, rotation,
// point count, coordinates, child count, children (all counts as u32 bits in
// f32 slots, little-endian).
//
// The engine assumes a node's `source` is also its index into the top-level
// node list (`self.nodes[source]`), which only holds if root sources are
// exactly 0..roots-1. Assign roots first, then number holes/islands after them.
function encodeTrees(roots) {
  const sourceToPart = new Map();
  roots.forEach((root, partIndex) => {
    root.source = partIndex;
    sourceToPart.set(partIndex, partIndex);
  });
  let nextSource = roots.length;
  const assignChildren = (node) => {
    node.children.forEach((child) => {
      child.source = nextSource;
      nextSource += 1;
      assignChildren(child);
    });
  };
  roots.forEach(assignChildren);

  const floatCount = (node) =>
    3 +
    node.ring.length +
    1 +
    node.children.reduce((sum, child) => sum + floatCount(child), 0);

  const totalFloats = 1 + roots.reduce((sum, node) => sum + floatCount(node), 0);
  const buffer = new ArrayBuffer(totalFloats * Float32Array.BYTES_PER_ELEMENT);
  const floats = new Float32Array(buffer);
  const view = new DataView(buffer);
  let index = 0;

  const writeU32 = (value) => {
    view.setUint32(index * Float32Array.BYTES_PER_ELEMENT, value >>> 0, true);
    index += 1;
  };
  const writeF32 = (value) => {
    floats[index] = value;
    index += 1;
  };
  const writeNode = (node) => {
    writeU32(node.source + 1);
    writeF32(0); // rotation
    writeU32(node.ring.length / 2);
    for (const value of node.ring) {
      writeF32(value);
    }
    writeU32(node.children.length);
    for (const child of node.children) {
      writeNode(child);
    }
  };

  writeU32(roots.length);
  roots.forEach(writeNode);

  return { data: floats, sourceToPart };
}

// Read the placements of one engine run, keeping one group per bin (the engine
// opens a new bin whenever a part no longer fits in the current one).
function buildBins(wrapper, remaining, sourceToPart, baseId) {
  const bins = [];
  const placedParts = new Set();
  let id = baseId;

  for (let binIndex = 0; binIndex < wrapper.placementCount; binIndex += 1) {
    wrapper.bindPlacement(binIndex);
    const sheetplacements = [];
    for (let index = 0; index < wrapper.size; index += 1) {
      const source = wrapper.bindData(index);
      const partIndex = sourceToPart.get(source);
      if (partIndex === undefined || placedParts.has(partIndex)) {
        continue;
      }
      placedParts.add(partIndex);
      sheetplacements.push({
        id,
        source,
        filename: remaining[partIndex].id,
        x: wrapper.x,
        y: wrapper.y,
        rotation: wrapper.rotation,
      });
      id += 1;
    }
    bins.push(sheetplacements);
  }

  const unplaced = remaining.filter((_, partIndex) => !placedParts.has(partIndex));
  return {
    bins,
    placedParts,
    unplaced,
    count: id - baseId,
    boundsSpan: wrapper.boundsWidth + wrapper.boundsHeight,
    // Average density across the bins. Bounds saturate at the sheet size once a
    // sheet fills, so density is what keeps changing as the GA packs better;
    // it must be part of the improvement test or the client stops updating.
    placePercentage: wrapper.placePercentage ?? 0,
  };
}

function makePayload(completedSheets, current, sheetIndex, index = 0, fitness = 0) {
  const placements = [...completedSheets];
  if (current) {
    current.bins.forEach((sheetplacements, binIndex) => {
      placements.push({
        sheet: 0,
        sheetid: sheetIndex + binIndex,
        sheetplacements,
      });
    });
  }

  const unplacedParts = current ? current.unplaced : [];
  const placed = total - unplacedParts.length;
  return {
    type: "result",
    better: true,
    data: {
      placements,
      unplaced: unplacedParts.map((part, index) => ({
        id: total + index,
        filename: part.id,
        rotation: 0,
      })),
      mergedLength: 0,
      index,
      fitness,
    },
    status: {
      better: true,
      complete: unplacedParts.length === 0,
      placed,
      total,
      unplaced: unplacedParts.length,
    },
  };
}

async function main() {
  const wasmBytes = readFileSync(
    fileURLToPath(new URL("./polygon-packer.wasm", import.meta.url))
  );
  const wasm = new WasmNesting();
  await wasm.initBuffer(
    wasmBytes.buffer.slice(
      wasmBytes.byteOffset,
      wasmBytes.byteOffset + wasmBytes.byteLength
    )
  );

  parentPort.postMessage({
    type: "progress",
    data: { progress: 0, phase: "placement", index: 0, threads: 1 },
  });

  const binFloat = Float32Array.from(bin);
  let bestScore = -Infinity;
  let posted = false;
  let lastProgressAt = 0;
  let candidateIndex = 0;

  // Keep solving until the job is stopped (or a time limit aborts the worker),
  // exactly like the original Deepnest engine. Each round solves the whole job
  // from scratch (the GA is randomized); we only forward a round when it beats
  // the best solution seen so far, so the client's layout refines over time and
  // never regresses.
  try {
    do {
      const completedSheets = [];
      let remaining = parts.slice();
      let sheetIndex = 0;
      let nextInstanceId = 0;
      let firstRun = true;
      let boundsSum = 0;

      while (remaining.length > 0 && sheetIndex < maxSheets && !stopRequested) {
        const { data, sourceToPart } = encodeTrees(
          remaining.map((part) => part.tree)
        );

        if (!firstRun) {
          wasm.stop();
        }
        firstRun = false;
        wasm.initTrees(config, data, binFloat);

        let bestSheet = null;
        let generation = 0;

        // Stream every improvement as a result instead of waiting for the sheet
        // to finish: clients (and the Corel preview) then see the layout refine
        // over time. We search the full generation budget even after everything
        // fits, so a complete layout can still get tighter/simpler.
        while (generation < maxGenerationsPerSheet && !stopRequested) {
          const pairs = wasm.getPairs(chunkSize);
          const generated = pairs.map((pair) => wasm.calculate(pair));
          const placementData = wasm.getPlacementData(
            generated.map((chunk) => chunk.buffer)
          );
          const placements = [wasm.calculate(placementData)];
          const wrapper = wasm.getPlacementResult(
            placements.map((chunk) => chunk.buffer)
          );

          // `hasResult` is only ever true for a strictly better placement
          // according to the engine itself, so every true here is a real
          // improvement. We keep the best for this sheet and forward it when it
          // beats everything seen so far (ordered: more placed, then fewer
          // sheets, then denser, then tighter bounds).
          if (wrapper.hasResult) {
            bestSheet = buildBins(
              wrapper,
              remaining,
              sourceToPart,
              nextInstanceId
            );
            const placed = total - bestSheet.unplaced.length;
            const sheetsNow = completedSheets.length + bestSheet.bins.length;
            const solution =
              placed * 1e12 -
              sheetsNow * 1e8 +
              bestSheet.placePercentage * 1e6 -
              (boundsSum + bestSheet.boundsSpan);
            if (solution > bestScore) {
              bestScore = solution;
              posted = true;
              candidateIndex += 1;
              parentPort.postMessage(
                makePayload(
                  completedSheets,
                  { bins: bestSheet.bins, unplaced: bestSheet.unplaced },
                  sheetIndex,
                  candidateIndex,
                  bestSheet.placePercentage
                )
              );
            }
          }

          generation += 1;
          // Throttle by time: the search loops until stopped, so per-generation
          // events would flood the SSE channel for no benefit.
          const now = Date.now();
          if (now - lastProgressAt >= 250) {
            lastProgressAt = now;
            parentPort.postMessage({
              type: "progress",
              data: {
                progress: Number.isFinite(maxGenerationsPerSheet)
                  ? Math.min(0.99, generation / maxGenerationsPerSheet)
                  : 0,
                phase: "placement",
                index: generation,
                threads: 1,
              },
            });
          }
          if (generation % 8 === 0) {
            await new Promise((resolve) => setImmediate(resolve));
          }
        }

        if (!bestSheet) {
          break;
        }

        bestSheet.bins.forEach((sheetplacements, binIndex) => {
          completedSheets.push({
            sheet: 0,
            sheetid: sheetIndex + binIndex,
            sheetplacements,
          });
        });
        nextInstanceId += bestSheet.count;
        sheetIndex += bestSheet.bins.length;
        boundsSum += bestSheet.boundsSpan;
        remaining = bestSheet.unplaced;

        const placedCount = total - remaining.length;
        const solution =
          placedCount * 1e12 -
          completedSheets.length * 1e8 +
          bestSheet.placePercentage * 1e6 -
          boundsSum;
        if (solution > bestScore && placedCount > 0) {
          bestScore = solution;
          posted = true;
          candidateIndex += 1;
          parentPort.postMessage(
            makePayload(
              completedSheets,
              { bins: [], unplaced: remaining },
              sheetIndex,
              candidateIndex,
              bestSheet.placePercentage
            )
          );
        }
      }

      if (!posted) {
        break;
      }
      // Yield between rounds so a stop message can be processed.
      await new Promise((resolve) => setImmediate(resolve));
    } while (!stopRequested);

    // If the engine never produced a placement, still emit one result so the
    // Job gets a callback and does not stay "running" forever.
    if (!posted) {
      parentPort.postMessage({
        type: "result",
        better: true,
        data: {
          placements: [],
          unplaced: parts.map((part, index) => ({
            id: total + index,
            filename: part.id,
            rotation: 0,
          })),
          mergedLength: 0,
          index: 0,
        },
        status: {
          better: true,
          complete: false,
          placed: 0,
          total,
          unplaced: total,
        },
      });
    }

    wasm.stop();
    parentPort.postMessage({ type: "done" });
  } catch (error) {
    parentPort.postMessage({
      type: "error",
      message: error && error.message ? error.message : String(error),
      stack: error && error.stack ? error.stack : undefined,
    });
  }
}

main();
