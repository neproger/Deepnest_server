# Vendored: OpenNest `nfp_nest` engine

This folder contains the C++ nesting engine from
[petrasvestartas/OpenNest](https://github.com/petrasvestartas/OpenNest), used as
the optional `opennest` engine of this server.

## Provenance

| | |
| --- | --- |
| Upstream | https://github.com/petrasvestartas/OpenNest |
| Commit | `be5456b840edbf8615dc7d84b2acbc43f330d772` (branch `main`) |
| Vendored paths | `src/opennest_cpp/src` -> `native/opennest/src`, `src/opennest_cpp/third_party/boost_min` -> `native/opennest/third_party/boost_min`, plus `CMakeLists.txt`, `LICENSE`, `CREDITS.md` |
| Not vendored | `bench/`, `out/`, `tools/` (research harnesses; not needed by the addon) |

The engine is a C++ re-implementation of the SVGnest/Deepnest no-fit-polygon +
genetic-algorithm method, extended with sparse/parallel evaluation, MaxRects fast
path, exact NFP, per-part rotation overrides and non-convex voids. It has a clean
C ABI in `src/capi/nfp_nest_capi.{(h,cpp)}` and is self-contained: Clipper2 and a
minimal Boost.Polygon subset are vendored in-tree.

## Integration in this repo

- `binding.gyp` target `opennest` compiles this engine plus `src/opennest_addon.cc`
  (Node-API bridge) into `build/Release/opennest.node`.
- `src/geometry/engines/opennest.mjs` adapts Canonical Geometry to the C ABI and
  maps results back to the engine payload contract.
- The native engine keeps process-global solve state (cancel/progress/live
  snapshot), so only one solve runs at a time. The bridge rejects a concurrent
  `nest()`.

## Licenses

- OpenNest: MIT (see `LICENSE`).
- Clipper2 (vendored `src/clipper2`): Boost Software License.
- Boost.Polygon subset (vendored `third_party/boost_min`): Boost Software License.
- See the repository root `LICENSES.md` and `CREDITS.md`.

## Updating

1. Replace `src/` and `third_party/boost_min/` with the new upstream revision
   (paths above) and update the commit in this file.
2. Re-check `binding.gyp` against the new `CMakeLists.txt` source list.
3. `npm run build:native` and run the tests.
