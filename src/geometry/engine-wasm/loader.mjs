// Vendored JS glue for the SVGnest Rust/WASM nesting core.
//
// Source: https://github.com/yuriilychak/SVGnest (MIT)
//   packages/polygon-packer/src/wasm-nesting.ts (+ helpers, placement-wrapper)
// Local fork additions/fixes to the upstream source before bundling:
//   - wasm-nesting.ts: `initTrees()` calling the patched
//     `wasm_packer_init_trees` export;
//   - packages/polygon-packer-algo: `WasmPacker::init_from_trees`,
//     `wasm_packer_init_trees` in lib.rs, and `pub` on
//     `clipper_wrapper::{offset_nodes, simplify_nodes}`;
//   - place_flow.rs: reject a first-placement `[NaN, NaN]` instead of marking
//     an unplaceable part as placed;
//   - genetic_algorithm.rs: look up the node by `source` (root sources are not
//     0..n-1 once holes/islands exist);
//   - pair_flow.rs: use `polygon_b.size()` for the hole-NFP condition (it
//     compared `child` against itself and never generated hole NFPs).
// Rebuild the wasm (from packages/polygon-packer-algo):
//   wasm-pack build --release --no-opt --target no-modules \
//     --out-name polygon-packer --out-dir ./pkg
// Regenerate this file from packages/polygon-packer:
//   npx esbuild src/wasm-nesting.ts --bundle --format=esm --platform=node \
//     --target=node20 --outfile=loader.mjs
// The raw .wasm is loaded manually by this glue (no browser fetch).
//
// third_party/svgnest/packages/polygon-packer/src/helpers.ts
function deserializeSourceItemsInternal(view, offset, count) {
  const children = [];
  let currentOffset = offset;
  for (let i = 0; i < count; ++i) {
    const source = view.getUint16(currentOffset, true);
    currentOffset += Uint16Array.BYTES_PER_ELEMENT;
    const childrenCount = view.getUint16(currentOffset, true);
    currentOffset += Uint16Array.BYTES_PER_ELEMENT;
    const childrenResult = deserializeSourceItemsInternal(view, currentOffset, childrenCount);
    children.push({
      source,
      children: childrenResult.children
    });
    currentOffset = childrenResult.nextOffset;
  }
  return { children, nextOffset: currentOffset };
}
function deserializeSourceItems(data) {
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const count = view.getUint16(0, true);
  const result = deserializeSourceItemsInternal(view, Uint16Array.BYTES_PER_ELEMENT, count);
  return result.children;
}
function flattenTree(nodes, hole, result = { sources: [], holes: [] }) {
  const nodeCount = nodes.length;
  let node;
  let children;
  for (let i = 0; i < nodeCount; ++i) {
    node = nodes[i];
    if (hole) {
      result.holes.push(node.source);
    }
    children = node.children;
    result.sources.push(node.source);
    if (children && children.length > 0) {
      flattenTree(children, !hole, result);
    }
  }
  return result;
}
function getByteOffset(array, index) {
  return (array.byteOffset >>> 0) + index * Float32Array.BYTES_PER_ELEMENT;
}
function readUint32FromF32(array, index) {
  const byteOffset = getByteOffset(array, index);
  const view = new DataView(array.buffer);
  return view.getUint32(byteOffset, true);
}
function joinFloat32Arrays(arrays) {
  const totalFloats = arrays.reduce((sum, a) => sum + a.length, 0);
  const totalElements = totalFloats + arrays.length;
  const buffer = new ArrayBuffer(totalElements * Float32Array.BYTES_PER_ELEMENT);
  arrays.reduce((acc, a) => {
    acc.dv.setUint32(acc.byteOffset * Uint32Array.BYTES_PER_ELEMENT, a.length >>> 0, true);
    acc.byteOffset += 1;
    acc.f32view.set(a, acc.byteOffset);
    acc.byteOffset += a.length;
    return acc;
  }, { dv: new DataView(buffer), f32view: new Float32Array(buffer), byteOffset: 0 });
  return new Float32Array(buffer);
}
function mergeFloat32Arrays(arrays) {
  const total = arrays.reduce((sum, arr) => sum + arr.byteLength / Float32Array.BYTES_PER_ELEMENT, 0);
  return arrays.reduce((acc, arr) => {
    const view = new Float32Array(arr);
    acc.result.set(view, acc.offset);
    acc.offset += view.length;
    return acc;
  }, { result: new Float32Array(total), offset: 0 }).result;
}
function splitFloat32Arrays(flat) {
  const result = [];
  const view = new DataView(flat.buffer, flat.byteOffset, flat.byteLength);
  let byteOffset = 0;
  while (byteOffset < flat.byteLength) {
    const size = view.getUint32(byteOffset, true);
    byteOffset += Uint32Array.BYTES_PER_ELEMENT;
    const floatByteOffset = flat.byteOffset + byteOffset;
    const arr = new Float32Array(flat.buffer, floatByteOffset, size);
    result.push(arr.slice());
    byteOffset += size * Float32Array.BYTES_PER_ELEMENT;
  }
  return result;
}

// third_party/svgnest/packages/polygon-packer/src/placement-wrapper.ts
var PlacementWrapper = class {
  #buffer;
  #view;
  #placement;
  #memSeg;
  #offset;
  #size;
  #pointData;
  #pointOffset;
  #placementCount;
  #angleSplit;
  #sources;
  constructor(buffer) {
    this.#buffer = buffer;
    this.#view = new DataView(this.#buffer);
    this.#memSeg = this.placementsData;
    this.#angleSplit = this.angleSplit;
    this.#placementCount = this.#memSeg[1];
    this.#memSeg = this.#memSeg;
    this.#sources = this.sources;
    this.#placement = 0;
    this.#offset = 0;
    this.#size = 0;
    this.#pointData = 0;
    this.#pointOffset = 0;
  }
  /**
   * Binds to a specific placement in the results to access its data.
   * 
   * Must be called before accessing placement-specific properties like
   * {@link offset}, {@link size}, or {@link bindData}.
   * 
   * @param index - Zero-based placement index (0 to {@link placementCount} - 1)
   */
  bindPlacement(index) {
    this.#placement = readUint32FromF32(this.#memSeg, 2 + index);
    this.#offset = this.#placement >>> 16;
    this.#size = this.#placement & (1 << 16) - 1;
  }
  /**
   * Binds to a specific polygon within the current placement.
   * 
   * After calling, properties like {@link id}, {@link rotation}, {@link x}, {@link y},
   * and {@link flattnedChildren} reflect data for this specific polygon.
   * 
   * @param index - Zero-based polygon index within the current placement (0 to {@link size} - 1)
   * @returns The source ID of the bound polygon
   */
  bindData(index) {
    this.#pointData = readUint32FromF32(this.#memSeg, this.#offset + index);
    this.#pointOffset = this.#offset + this.#size + (index << 1);
    return this.#sources[this.id].source;
  }
  /**
   * Gets the flattened children (holes) of the currently bound polygon.
   * 
   * Returns a flattened representation of nested polygon children,
   * useful for rendering holes within a parent polygon.
   * 
   * @returns Flattened children data, or `null` if the polygon has no children
   */
  get flattnedChildren() {
    const source = this.#sources[this.id];
    return source.children.length ? flattenTree(source.children, true) : null;
  }
  /**
   * Gets the total number of placements in the result.
   * @returns Number of placements
   */
  get placementCount() {
    return this.#placementCount;
  }
  /**
   * Gets the offset of the currently bound placement in the data array.
   * @returns Placement offset
   * @internal
   */
  get offset() {
    return this.#offset;
  }
  /**
   * Gets the number of polygons in the currently bound placement.
   * @returns Number of polygons in current placement
   */
  get size() {
    return this.#size;
  }
  /**
   * Gets the polygon ID of the currently bound polygon.
   * @returns Polygon ID
   */
  get id() {
    return this.#pointData >>> 16;
  }
  /**
   * Gets the rotation angle in degrees of the currently bound polygon.
   * @returns Rotation angle (0-360 degrees)
   */
  get rotation() {
    return Math.round((this.#pointData & (1 << 16) - 1) * 360 / this.#angleSplit);
  }
  /**
   * Gets the X coordinate of the currently bound polygon.
   * @returns X position
   */
  get x() {
    return this.#memSeg[this.#pointOffset];
  }
  /**
   * Gets the Y coordinate of the currently bound polygon.
   * @returns Y position
   */
  get y() {
    return this.#memSeg[this.#pointOffset + 1];
  }
  /**
   * Gets the percentage of polygons successfully placed (0-1).
   * @returns Placement success rate
   */
  get placePercentage() {
    return this.#view.getFloat32(0, true);
  }
  /**
   * Gets the number of parts that were successfully placed.
   * @returns Number of placed parts
   */
  get numPlacedParts() {
    return this.#view.getUint16(4, true);
  }
  /**
   * Gets the total number of parts that were attempted to be placed.
   * @returns Total number of parts
   */
  get numParts() {
    return this.#view.getUint16(6, true);
  }
  /**
   * Gets the angle quantization value used for rotation calculations.
   * @returns Angle split value
   * @internal
   */
  get angleSplit() {
    return this.#view.getUint8(8);
  }
  /**
   * Checks if the packing algorithm produced a valid result.
   * @returns `true` if results are available, `false` otherwise
   */
  get hasResult() {
    return this.#view.getUint8(9) === 1;
  }
  /**
   * Gets the X coordinate of the bounding box for all placed polygons.
   * @returns Bounding box X position
   */
  get boundsX() {
    return this.#view.getFloat32(10, true);
  }
  /**
   * Gets the Y coordinate of the bounding box for all placed polygons.
   * @returns Bounding box Y position
   */
  get boundsY() {
    return this.#view.getFloat32(14, true);
  }
  /**
   * Gets the width of the bounding box for all placed polygons.
   * @returns Bounding box width
   */
  get boundsWidth() {
    return this.#view.getFloat32(18, true);
  }
  /**
   * Gets the height of the bounding box for all placed polygons.
   * @returns Bounding box height
   */
  get boundsHeight() {
    return this.#view.getFloat32(22, true);
  }
  /**
   * Gets the hierarchical source polygon tree.
   * 
   * Deserializes the polygon hierarchy from the binary buffer,
   * including parent-child relationships for holes.
   * 
   * @returns Array of source items with nested children
   */
  get sources() {
    const sourcesSize = this.#view.getUint32(26, true);
    if (sourcesSize === 0) {
      return [];
    }
    const sourcesData = new Uint8Array(this.#buffer, 34, sourcesSize);
    return deserializeSourceItems(sourcesData);
  }
  /**
   * Gets the raw placements data as a Float32Array.
   * 
   * Contains encoded placement information including positions,
   * rotations, and polygon IDs.
   * 
   * @returns Float32Array view of placements data
   * @internal
   */
  get placementsData() {
    const sourcesSize = this.#view.getUint32(26, true);
    const placementsDataSize = this.#view.getUint32(30, true);
    if (placementsDataSize === 0) {
      return new Float32Array(0);
    }
    const placementsOffset = 34 + sourcesSize;
    return new Float32Array(this.#buffer, placementsOffset, placementsDataSize / Float32Array.BYTES_PER_ELEMENT);
  }
};

// third_party/svgnest/packages/polygon-packer/src/wasm-nesting.ts
var MEM_SEG_TYPES = [Uint8Array, Uint16Array, Float32Array];
var WasmNesting = class {
  #wasm;
  #heap;
  #heapNext;
  #textDecoder;
  #vecLen;
  #memSegs;
  #isInitialized = false;
  constructor() {
    this.#heap = new Array(128).fill(void 0);
    this.#heap.push(void 0, null, true, false);
    this.#heapNext = this.#heap.length;
    this.#textDecoder = typeof TextDecoder !== "undefined" ? new TextDecoder("utf-8", { ignoreBOM: true, fatal: true }) : {
      decode: () => {
        throw Error("TextDecoder not available");
      }
    };
    this.#memSegs = new Array(3).fill(null);
    this.#vecLen = 0;
  }
  async initBuffer(bytes) {
    const imports = this.#getImports();
    const module2 = await WebAssembly.compile(bytes);
    const instance = await WebAssembly.instantiate(module2, imports);
    this.#wasm = instance.exports;
    this.#memSegs.fill(null);
    this.#isInitialized = true;
  }
  setBits(source, value, index, bit_count) {
    const ret = this.#wasm.set_bits_u32(source, value, index, bit_count);
    return ret >>> 0;
  }
  calculate(buffer) {
    const ptr0 = this.#passMem(buffer, this.#wasm.__wbindgen_export_1);
    const ret = this.#wasm.calculate_chunk_wasm(ptr0, this.#vecLen);
    return this.#takeObject(ret);
  }
  init(configuration, polygons) {
    const polygon_data = joinFloat32Arrays(polygons);
    const config = this.#serializeConfig(configuration);
    const ptr0 = this.#passMem(polygon_data, this.#wasm.__wbindgen_export_1);
    const len0 = this.#vecLen;
    this.#wasm.wasm_packer_init(config, ptr0, len0);
  }
  initTrees(configuration, treeData, bin) {
    const config = this.#serializeConfig(configuration);
    const treePtr = this.#passMem(treeData, this.#wasm.__wbindgen_export_1);
    const treeLen = this.#vecLen;
    const binPtr = this.#passMem(bin, this.#wasm.__wbindgen_export_1);
    const binLen = this.#vecLen;
    this.#wasm.wasm_packer_init_trees(config, treePtr, treeLen, binPtr, binLen);
  }
  nest() {
    const ret = this.#wasm.wasm_nest();
    const result = this.#takeObject(ret);
    return new PlacementWrapper(result.buffer);
  }
  getPairs(chunkSize) {
    const ret = this.#wasm.wasm_packer_get_pairs(chunkSize >>> 0);
    const result = this.#takeObject(ret);
    return splitFloat32Arrays(result);
  }
  getPlacementData(generatedNfp) {
    const generated_nfp = mergeFloat32Arrays(generatedNfp);
    const ptr0 = this.#passMem(generated_nfp, this.#wasm.__wbindgen_export_1);
    const len0 = this.#vecLen;
    const ret = this.#wasm.wasm_packer_get_placement_data(ptr0, len0);
    return this.#takeObject(ret);
  }
  getPlacementResult(placements) {
    const placements_f32 = mergeFloat32Arrays(placements);
    const ptr0 = this.#passMem(placements_f32, this.#wasm.__wbindgen_export_1);
    const len0 = this.#vecLen;
    const ret = this.#wasm.wasm_packer_get_placement_result(ptr0, len0);
    const result = this.#takeObject(ret);
    return new PlacementWrapper(result.buffer);
  }
  stop() {
    this.#wasm.wasm_packer_stop();
  }
  get isInitialized() {
    return this.#isInitialized;
  }
  #addHeapObject(obj) {
    if (this.#heapNext === this.#heap.length) {
      this.#heap.push(this.#heap.length + 1);
    }
    const idx = this.#heapNext;
    this.#heapNext = this.#heap[idx];
    this.#heap[idx] = obj;
    return idx;
  }
  #handleError(f, args) {
    try {
      return f.apply(this, args);
    } catch (e) {
      this.#wasm.__wbindgen_export_0(this.#addHeapObject(e));
    }
  }
  #getMem(index) {
    if (index > 2) {
      throw new Error("Unsupported memory segment index");
    }
    if (this.#memSegs[index] === null || this.#memSegs[index].byteLength === 0) {
      const ArrayType = MEM_SEG_TYPES[index];
      this.#memSegs[index] = new ArrayType(this.#wasm.memory.buffer);
    }
    return this.#memSegs[index];
  }
  #passMem(arg, malloc) {
    const bytes = arg.BYTES_PER_ELEMENT;
    const ptr = malloc(arg.length * bytes, bytes) >>> 0;
    const memory = this.#getMem(bytes >> 1);
    memory.set(arg, ptr / bytes);
    this.#vecLen = arg.length;
    return ptr;
  }
  #getString(ptr, len) {
    ptr = ptr >>> 0;
    return this.#textDecoder.decode(this.#getMem(0).subarray(ptr, ptr + len));
  }
  #checkObject(idx, type) {
    return typeof this.#getObject(idx) === type;
  }
  #getObject(idx) {
    return this.#heap[idx];
  }
  #dropObject(idx) {
    if (idx < 132) {
      return;
    }
    this.#heap[idx] = this.#heapNext;
    this.#heapNext = idx;
  }
  #takeObject(idx) {
    const ret = this.#getObject(idx);
    this.#dropObject(idx);
    return ret;
  }
  #isLikeNone(x) {
    return x === void 0 || x === null;
  }
  #addGlobalObject(data) {
    const formattedData = typeof data === "undefined" ? null : data;
    return this.#isLikeNone(formattedData) ? 0 : this.#addHeapObject(formattedData);
  }
  #getImports() {
    return {
      wbg: {
        __wbg_buffer_609cc3eee51ed158: (arg0) => {
          const ret = this.#getObject(arg0).buffer;
          return this.#addHeapObject(ret);
        },
        __wbg_call_672a4d21634d4a24: (...args) => this.#handleError((arg0, arg1) => {
          const ret = this.#getObject(arg0).call(this.#getObject(arg1));
          return this.#addHeapObject(ret);
        }, args),
        __wbg_call_7cccdd69e0791ae2: (...args) => {
          return this.#handleError((arg0, arg1, arg2) => {
            const ret = this.#getObject(arg0).call(this.#getObject(arg1), this.#getObject(arg2));
            return this.#addHeapObject(ret);
          }, args);
        },
        __wbg_crypto_574e78ad8b13b65f: (arg0) => {
          const ret = this.#getObject(arg0).crypto;
          return this.#addHeapObject(ret);
        },
        __wbg_getRandomValues_b8f5dbd5f3995a9e: (...args) => this.#handleError((arg0, arg1) => {
          this.#getObject(arg0).getRandomValues(this.#getObject(arg1));
        }, args),
        __wbg_length_3b4f022188ae8db6: (arg0) => this.#getObject(arg0).length,
        __wbg_length_a446193dc22c12f8: (arg0) => this.#getObject(arg0).length,
        __wbg_msCrypto_a61aeb35a24c1329: (arg0) => {
          const ret = this.#getObject(arg0).msCrypto;
          return this.#addHeapObject(ret);
        },
        __wbg_new_a12002a7f91c75be: (arg0) => {
          const ret = new Uint8Array(this.#getObject(arg0));
          return this.#addHeapObject(ret);
        },
        __wbg_newnoargs_105ed471475aaf50: (arg0, arg1) => {
          const ret = new Function(this.#getString(arg0, arg1));
          return this.#addHeapObject(ret);
        },
        __wbg_newwithbyteoffsetandlength_d97e637ebe145a9a: (arg0, arg1, arg2) => {
          const ret = new Uint8Array(this.#getObject(arg0), arg1 >>> 0, arg2 >>> 0);
          return this.#addHeapObject(ret);
        },
        __wbg_newwithbyteoffsetandlength_e6b7e69acd4c7354: (arg0, arg1, arg2) => {
          const ret = new Float32Array(this.#getObject(arg0), arg1 >>> 0, arg2 >>> 0);
          return this.#addHeapObject(ret);
        },
        __wbg_newwithlength_5a5efe313cfd59f1: (arg0) => this.#addHeapObject(new Float32Array(arg0 >>> 0)),
        __wbg_newwithlength_a381634e90c276d4: (arg0) => this.#addHeapObject(new Uint8Array(arg0 >>> 0)),
        __wbg_node_905d3e251edff8a2: (arg0) => {
          const ret = this.#getObject(arg0).node;
          return this.#addHeapObject(ret);
        },
        __wbg_process_dc0fbacc7c1c06f7: (arg0) => {
          const ret = this.#getObject(arg0).process;
          return this.#addHeapObject(ret);
        },
        __wbg_randomFillSync_ac0988aba3254290: (...args) => this.#handleError((arg0, arg1) => {
          this.#getObject(arg0).randomFillSync(this.#takeObject(arg1));
        }, args),
        __wbg_require_60cc747a6bc5215a: (...args) => {
          return this.#handleError(() => {
            const ret = module.require;
            return this.#addHeapObject(ret);
          }, args);
        },
        __wbg_set_10bad9bee0e9c58b: (arg0, arg1, arg2) => {
          this.#getObject(arg0).set(this.#getObject(arg1), arg2 >>> 0);
        },
        __wbg_set_65595bdd868b3009: (arg0, arg1, arg2) => {
          this.#getObject(arg0).set(this.#getObject(arg1), arg2 >>> 0);
        },
        __wbg_static_accessor_GLOBAL_88a902d13a557d07: () => this.#addGlobalObject(global),
        __wbg_static_accessor_GLOBAL_THIS_56578be7e9f832b0: () => this.#addGlobalObject(globalThis),
        __wbg_static_accessor_SELF_37c5d418e4bf5819: () => this.#addGlobalObject(self),
        __wbg_static_accessor_WINDOW_5de37043a91a9c40: () => this.#addGlobalObject(window),
        __wbg_subarray_aa9065fa9dc5df96: (arg0, arg1, arg2) => {
          const ret = this.#getObject(arg0).subarray(arg1 >>> 0, arg2 >>> 0);
          return this.#addHeapObject(ret);
        },
        __wbg_versions_c01dfd4722a88165: (arg0) => {
          const ret = this.#getObject(arg0).versions;
          return this.#addHeapObject(ret);
        },
        __wbindgen_is_function: (arg0) => this.#checkObject(arg0, "function"),
        __wbindgen_is_object: (arg0) => this.#checkObject(arg0, "object") && this.#getObject(arg0) !== null,
        __wbindgen_is_string: (arg0) => this.#checkObject(arg0, "string"),
        __wbindgen_is_undefined: (arg0) => this.#checkObject(arg0, "undefined"),
        __wbindgen_memory: () => this.#addHeapObject(this.#wasm.memory),
        __wbindgen_object_clone_ref: (arg0) => {
          const ret = this.#getObject(arg0);
          return this.#addHeapObject(ret);
        },
        __wbindgen_object_drop_ref: (arg0) => {
          this.#takeObject(arg0);
        },
        __wbindgen_string_new: (arg0, arg1) => {
          const ret = this.#getString(arg0, arg1);
          return this.#addHeapObject(ret);
        },
        __wbindgen_throw: (arg0, arg1) => {
          throw new Error(this.#getString(arg0, arg1));
        }
      }
    };
  }
  #serializeConfig(config) {
    let result = 0;
    result = this.setBits(result, config.curveTolerance * 10, 0, 4);
    result = this.setBits(result, config.spacing, 4, 5);
    result = this.setBits(result, config.rotations, 9, 5);
    result = this.setBits(result, config.populationSize, 14, 7);
    result = this.setBits(result, config.mutationRate, 21, 7);
    result = this.setBits(result, Number(config.useHoles), 28, 1);
    return result;
  }
};
export {
  WasmNesting as default
};
