# Nesting engine — ironnest (Rust)

Status of the nesting engine. ironnest is the **only** engine: the original
Deepnest GA / native-NFP engine and the SVGnest WASM core have been removed.

## What it is

[`ironnest`](https://github.com/TexasCoding/ironnest) is a deterministic,
embeddable 2D true-shape nesting engine: a fork-and-extend of
[`jagua-rs`](https://github.com/JeroenGar/jagua-rs) (collision detection) pushed
to `f64` and made cross-platform reproducible, with its own bin-packing
optimizer on top. It is a pure **placement oracle**:

```text
in:  items (polygon outlines) + sheets (outline + keep-out holes)
     + min separation + allowed rotations + seed + sample budget
out: (item, x, y, rotation) placements  and  the unplaced instances
```

There is no incremental/progress API and no mid-solve cancellation inside the
engine. The same inputs always produce the same placements.

## Layout (in this repo)

```text
native/vendor/ironnest/   vendored crate source (MPL-2.0), pinned to a commit
native/ironnest-napi/     Node-API bridge (napi-rs) -> the `.node` addon
src/geometry/engine-ironnest/
    index.mjs             adapter: canonical geometry -> request -> callback payload
    worker.mjs            worker-thread driver (progressive results)
src/geometry/engine.mjs   thin engine boundary (nestGeometry / nestWithRender)
```

The vendored source is `native/vendor/ironnest` (crates `geo`, `cde`,
`optimizer`, `ironnest`; the PyO3 `py` crate is unused by us). It is tracked in
the repo because ironnest is `publish = false` on crates.io.

## Build

```sh
npm run engine:build
```

This runs `npm install` + `napi build --platform --release` inside
`native/ironnest-napi`, which compiles the vendored Rust crates and emits:

```text
native/ironnest-napi/ironnest_napi.<platform>.node
native/ironnest-napi/index.js
```

Build artifacts are git-ignored; only the crate source is committed. The addon
is built per platform (win-x64 / macos-arm64 / linux-x64) — there are no
committed prebuilds yet.

The bridge exposes two JSON-in/JSON-out functions, `nest` and `nestMulti`; the
whole Node-API surface is JSON on purpose, so the ABI stays tiny and the
expensive solve runs in-process with no serialization boundary around it.

## Engine boundary / contract

`nestGeometry(geometry, callback, options)` and `nestWithRender(...)` in
`src/geometry/engine.mjs` delegate to the adapter. The adapter:

- normalizes canonical geometry and expands every sheet **type** × `quantity`
  into the flat, ordered list of physical sheet instances that `nestMulti`
  consumes (`sheetTypeOf` maps an instance back to its client `sheetId`);
- sends **one outer ring per part**: ironnest does not model holes inside a
  part. A part hole is therefore not represented for placement. This is
  conservative (the part is treated as solid) and does not affect the client,
  which re-applies the returned transform to its own geometry. Sheet holes
  (children of the sheet polygon) are forwarded as keep-out zones;
- maps `(item, x, y, rotation)` back to the engine payload contract consumed by
  `src/jobs/input.mjs#toExternalResult` (`partId`, `instanceId`, `sheetId`,
  `sheetInstanceId`).

Placements use the same `translate(x,y) rotate(rotation)` model the client
expects.

## Job semantics (progress + abort)

ironnest returns a complete solve per call, so the adapter runs it in a
`worker_threads` worker and drives a **progressive** loop:

1. iteration 0 uses `separation_effort: "off"` — a construction-only preview
   that returns in a fraction of the time (see below);
2. later iterations run the configured effort (default `"fast"`) from the
   canonical seed and are forwarded only when they improve on the best
   (more placed → fewer sheets → higher density).

`abort()` posts a stop and terminates the worker, so `POST /jobs/:id/stop`
still interrupts an in-flight solve.

## Config passthrough

`config` is forwarded to the engine. Recognized keys:

| key | meaning |
|---|---|
| `spacing` | minimum separation (`min_sep`), in canonical units |
| `rotations` | a count (`4` → `0/90/180/270`) or an explicit angle list |
| `budget` | samples per item placement (default `1000`) |
| `strategy` | `"sampling"` (default) or `"nfp"` |
| `separationEffort` | `"full"`, `"fast"` (default), `"max"`, `"off"` |
| `columnWeight` | NFP LBF horizontal weight |

The rest of `config` (`populationSize`, `mutationRate`, `placementType`,
`mergeLines`, `timeRatio`, …) is accepted for compatibility but ignored by this
engine. `mergeLines` is **not** implemented by ironnest.

## Vendored patch: `SeparationEffort::Off`

The separation ("leftover packing") tail dominates wall-clock on
over-subscribed jobs. Measured on a real Corel job (26 parts on 1000×500 mm at
20 mm spacing): `full` ≈ 47 s, `fast` ≈ 7 s, and a construction-only preview
≈ 1.5 s — for the same 26/26 placement.

To enable the preview, `native/vendor/ironnest` carries a small MPL-2.0
modification adding `SeparationEffort::Off` (skips `run_separation`), wired
through `native/ironnest-napi`. This is the only local modification to the
vendored source; see `crates/optimizer/src/lib.rs` and
`crates/optimizer/src/sep/mod.rs`.

## Known limitations

- **Part holes** are not modelled (see above). "Nest into another part's hole"
  requires container holes / a second pass.
- `GET /result.svg` returns `400 RESULT_FORMAT_UNAVAILABLE`: there is no
  server-side SVG renderer for this engine. Clients draw previews from
  `result.parts`; the Corel addon does not use `result.svg`.
- The engine is single-threaded (deterministic by design); concurrency is the
  Job queue's.
- `separation_effort` defaults to `"fast"` for server responsiveness; pass
  `"full"` in `config` for maximum density at higher cost.

## Licensing

ironnest and its jagua-rs lineage are **MPL-2.0** (file-scoped copyleft). The
vendored source is unmodified except for the documented `Off` patch; keep the
per-file MPL headers and publish modifications to those files when
redistributing.

## Upgrading the engine

1. Replace `native/vendor/ironnest/{crates,Cargo.toml,Cargo.lock}` with the new
   revision, re-apply the `Off` patch (search for `SeparationEffort::Off`), and
   note the new commit here.
2. `npm run engine:build`.
3. `npm run test:core` (the `ironnest-engine` test covers single/multi sheet,
   unplaced, determinism).
