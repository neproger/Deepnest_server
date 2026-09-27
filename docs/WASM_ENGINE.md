# WASM nesting engine (SVGnest / polygon-packer)

Status of the Rust/WASM nesting core that is now the **default** engine.

- Engine selection lives in `src/geometry/engine.mjs` (`run()`).
- Default: `wasm`. Fall back with `DEEPNEST_ENGINE=deepnest`.
- Adapter: `src/geometry/engine-wasm/{index.mjs,worker.mjs,loader.mjs}` plus the
  vendored binary `polygon-packer.wasm` (~212 KB).
- The engine runs in a `worker_threads` worker, so a Rust panic cannot take the
  server down and `abort()` can terminate it.

## What works

- Canonical geometry in, stable `partId`, `sheetInstanceId`, `x/y/rotation`
  placements out — the same external contract as the Deepnest engine.
- Holes (explicit outer/hole hierarchy via the patched `wasm_packer_init_trees`).
- Multi-sheet and `quantity` expansion (the engine opens bins internally; each
  bin becomes a sheet instance; leftover parts are re-run in fresh instances).
- Incremental `result.updated` streaming and `engine.progress` over SSE.
- `result.parts` (preview geometry) is always present, so the Corel form draws
  previews without `result.svg`.
- Time limit (`execution.timeLimitMs`) completes a job; `stop`/queue work.

`GET /result.svg` returns `400 RESULT_FORMAT_UNAVAILABLE` under the wasm engine
(no server-side SVG renderer for this path). The Corel addon does not use it.

## Integration fixes applied

Adapter (`src/geometry/engine-wasm`):

- Fresh polygon tree per `quantity` instance (sharing one tree collided on
  `source`).
- `encodeTrees` assigns roots `0..n-1`, holes/islands after them (the engine
  assumes `nodes[source]`).
- No artificial layout offsets: the returned transform is applied directly to
  the part's canonical coordinates.
- `buildBins` maps each engine bin to a separate sheet instance.
- Final-result safety net so a job never hangs without a result.
- **Search budget**: the Rust GA evaluates ONE individual per iteration and
  advances a generation only after `populationSize` evaluations. The budget is
  therefore `generationsPerSheet * populationSize` (`generationsPerSheet` default
  100), not a raw iteration count. Previously 40 iterations meant ~2 generations.
- **Improvement test**: the engine only reports a result when it strictly beats
  its running best (`hasResult`). The driver forwards every such improvement and
  scores it as *more placed > fewer sheets > higher density (`placePercentage`) >
  tighter bounds*. Bounding-box metrics saturate at the sheet size, so density
  must be part of the test — otherwise the client freezes on the first layout.
- `data.index`/`data.fitness` are now populated (candidate counter and
  `placePercentage`) so the Corel status line shows real values.
- `engine.progress` is emitted at most every 250 ms (was flooding).

Rust patches (the engine is an alpha and needed fixes). `third_party/` is
git-ignored, so these are **not** in the repo — re-apply them on a fresh clone:

- `nesting/place_flow.rs`: reject a first placement of `[NaN, NaN]`.
- `genetic_algorithm/genetic_algorithm.rs`: `mutate` looks the node up by
  `source` (sets of roots with holes are not `0..n-1`); fixes an index panic.
- `nesting/pair_flow.rs`: hole-NFP condition used `child.size()` instead of
  `polygon_b.size()` (always false → holes never generated).
- `wasm_packer.rs` + `lib.rs`: added `wasm_packer_init_trees(config, trees, bin)`
  / `WasmPacker::init_from_trees` to pass the explicit hierarchy.
- `clipper_wrapper.rs`: made `offset_nodes`/`simplify_nodes` public.
- `packages/polygon-packer/src/wasm-nesting.ts`: added `initTrees()`.

Rebuild (from `third_party/svgnest/packages/polygon-packer-algo`):

```
$env:RUSTFLAGS="-C target-feature=+simd128"
wasm-pack build --release --no-opt --target no-modules --out-name polygon-packer --out-dir ./pkg
# then copy pkg/polygon-packer_bg.wasm -> src/geometry/engine-wasm/polygon-packer.wasm
# rebundle the loader:
npx esbuild src/wasm-nesting.ts --bundle --format=esm --platform=node --target=node20 --outfile=loader.mjs
```

## Known problems (TODO)

1. **Overlapping placements** — the biggest issue. With identical (or very
   similar) parts the NFP/placement lets several parts occupy the *same*
   coordinates. Evidence: 26 parts of 100×100 in a 1000×500 sheet reports
   `bins=1` (impossible: capacity is 25) with duplicated `x,y` such as `600,100`
   twice and `600,200` three times. Because the engine believes a 1-bin overlap
   is optimal, it stops improving — this is consistent with "the preview stays
   on the first layout". Likely in NFP generation / bottom-left candidate
   selection (`nesting/nfp_store.rs`, `nesting/place_flow.rs`,
   `nesting/place_content.rs`); needs a comparison against the original
   Deepnest/SVGnest placement.
2. **NFP cache key rotation width** — `generate_nfp_cache_key`
   (`nesting/polygon_node.rs`) packs the rotation index into 4 bits while the
   config allows up to 31 rotations, so indices ≥16 can collide and reuse the
   wrong NFP. Not hit at the default `rotations=4`, but wrong for high values.
3. **Coarse objective** — improvements are only reported when the engine's own
   best is beaten; density keeps changing but bin count rarely drops, so
   improvements are sparse on large sheets (mitigated by the scoring above).
4. Full server contract parity is not reached: `result.svg` (see above) and the
   SSE-driven server tests that assume the Deepnest-only behaviours.

## Reproducing the overlap

```
node <temp>/probe-ga2.mjs 26 100 5   # N, part size, iterations
# iter 0: placed=26 bins=1 ...  bin 0 (26): ... 600,100 ... 600,100 ... 600,200 ...
```
