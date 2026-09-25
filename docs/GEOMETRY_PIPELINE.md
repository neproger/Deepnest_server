# Geometry Pipeline

> Investigation of the **current** geometry model, from an SVG string to the
> objects consumed by the nesting algorithm. This documents what actually
> exists today. It is not a target architecture and does not propose changing
> the pipeline. Findings come from reading `main/svgparser.js`,
> `main/deepnest.js`, `main/background.js`, `main/processPair.mjs` and from a
> runtime probe (see "Runtime evidence" below).

## Current Flow

```text
SVG string
  │
  ├─ index.node.mjs            installs jsdom globals (DOMParser, XMLSerializer, window, document)
  │
  ├─ index.mjs nest()          importsvg(bin) → sheet part, then importsvg(parts...)
  │      │
  │      └─ main/svgparser.js  load → clean (transform/flatten/filter/splitPath/mergeLines)
  │             │
  │             └─ main/deepnest.js getParts()
  │                    polygonify()   curves → point arrays
  │                    cleanPolygon() clipper simplify
  │                    toTree()       containment → parent/children (holes)
  │                    svgelements[]  attach source DOM nodes
  │
  ├─ deepNest.parts[]          { polygontree, svgelements, bounds, area, quantity, filename, sheet? }
  │
  └─ deepNest.start()
         cloneTree()            plain copy: points {x,y,exact} + children
         offsetTree()           apply spacing via clipper offset + simplify
         launchWorkers()        GA seed `adam`, attach id/source/filename
                │
                └─ worker.postMessage(payload)   ← plain data, no DOM
                       │
                       └─ main/background.js processMessage() → placeParts()
                              → main/processPairs.node.mjs → processPair.mjs (clipper Minkowski)
                              → native addon calculateNFP (holes / inside)
```

## SVG Parsing

`SvgParser.load` (`main/svgparser.js:92`) parses with `new DOMParser()` and
normalises scale using the root `width`/`viewBox`, appending a `scale(...)`
transform so the root `scale` option (default `72` = units/inch, set in
`index.mjs`) is honoured. `config.curveTolerance` is pushed into the parser via
`SvgParser.config({ tolerance, endpointTolerance })` from `DeepNest.config`.

`SvgParser.cleanInput` (`main/svgparser.js:204`):
- `applyTransform` — bakes SVG transforms into coordinates; replaces transformed
  `rect`/`ellipse` with `polygon`/`path`.
- `flatten` — moves all shapes to top level.
- `filter(allowedElements)` — keeps only `svg,circle,ellipse,path,polygon,polyline,rect,image,line`.
- `splitPath` / `splitLines` / `mergeLines` — split compound paths and merge open
  segments into closed ones.

Everything here is DOM-based and uses `SVGPathElement`, `pathSegList`,
`createElementNS`, `querySelectorAll`, etc. (vendored `util/pathsegpolyfill.js`).

## Polygon Representation

A **polygon is a plain JavaScript array of points**, with extra array
properties (arrays are objects, so custom props are legal). A point is:

```js
{ x: number, y: number, exact?: boolean }
```

A polygon tree item additionally carries:

```js
polygon.source    // number — index (into an SVG's children, then into deepNest.parts)
polygon.children  // Array<polygon> — holes (see Topology)
polygon.id        // number — tree node id (getParts), later overwritten by GA instance id
polygon.parent    // polygon — set on holes by toTree, dropped by cloneTree
polygon.filename  // string|null — attached during launchWorkers
```

`exact` is **not** present after import; it is added later by
`DeepNest.simplifyPolygon` (`main/deepnest.js:236`) and consumed by
`mergedLength` (`main/background.js:315`). It marks points that came from
coplanar merges so line-merging can match segments.

## Part Representation

`deepNest.parts[i]` (created in `getParts`, `main/deepnest.js:767`):

| field | type | created | read by | category |
|---|---|---|---|---|
| `polygontree` | polygon tree | `getParts` | `start`→`cloneTree`, renderer | **A geometry** |
| `svgelements` | DOM `Element[]` | `getParts` | only `nestingToSVG` | C SVG/import metadata |
| `bounds` | `{x,y,width,height}` | `getParts` | only `nestingToSVG` (sheet bounds) | C SVG/import metadata |
| `area` | `bounds.width*height` | `getParts` | **read nowhere** (dead field) | D dead |
| `quantity` | number (always `1` here) | `getParts` | `start`/`launchWorkers` loops | B nesting metadata |
| `filename` | string\|null | `getParts` | carried to placement; triggers `sheet` for `BACKGROUND.svg` | B/C metadata |
| `sheet` | boolean | only `getParts` for `BACKGROUND.svg`; set manually on the bin in `index.mjs` | `start`/`launchWorkers` | B nesting metadata |

`importsvg` returns these same part objects (and appends them to
`deepNest.parts`).

## Sheet Representation

There is **no separate geometry type for a sheet**. A sheet is just a part with
`.sheet === true`. `index.mjs` imports the bin SVG first and sets
`sheetSVG.sheet = true`, which is why the bin occupies `deepNest.parts[0]`.

In `deepNest.start` (`main/deepnest.js:951`) each part is reduced to
`{ quantity, sheet, polygontree: cloneTree(...), filename }`. Then `offsetTree`
is applied: parts get `+0.5*spacing` outward, sheets get `-0.5*spacing` inward
(`main/deepnest.js:971-978`). The worker payload separates sheets:

```text
payload.sheets        // Array<polygon tree>
payload.sheetids      // instance index per sheet (incremented per quantity copy)
payload.sheetsources  // index `i` in deepNest.parts (0 for the single bin)
payload.sheetchildren // parallel array of hole children
```

`placeParts` (`main/background.js:752`) shifts sheets off `payload.sheets`,
opens new sheets until parts are placed or sheets are exhausted, and groups
results as `{ sheet: sheet.source, sheetid: sheet.id, sheetplacements }`.

`index.mjs` imports exactly one bin copy, so in practice `sheetid` is always `0`
and `sheet` is always the bin's index (`0`). The structures allow multiple sheet
parts, but nothing currently exercises that path (`if (sheets.length == 0) break`).

## Holes and Topology

`getParts.toTree` (`main/deepnest.js:678`) builds the topology by containment:
for each polygon it samples points (`Math.min(10, length)`) and tests
`Clipper.PointInPolygon` against every other polygon; the first container wins
and the polygon is pushed into `container.children`. `toTree` recurses into
`parents[i].children`, so the structure supports **arbitrary nesting depth**
(outer → hole → island → ...). Winding/orientation is not used to decide
outer vs hole at import; containment decides.

How far the algorithm consumes it:
- `getOuterNfp` and the native `calculateNFP` consume `A.children` (direct
  holes) as a polygon set subtraction (`main/background.js:129-150, 196-211`).
- `getInnerNfp` consumes `A.children` (direct holes) when computing inner NFPs.
- `rotatePolygon`, `shiftPolygon`, `mergedLength` recurse through `children` at
  any depth (`main/background.js:426, 442, 558`).

So: the **tree supports arbitrary depth**, but the NFP/placement math only
explicitly models **one level of holes** (direct `children`). Islands inside a
hole are structurally preserved but not treated as separate solids by NFP.

`cloneTree` (`main/deepnest.js:932`) copies only `{x, y, exact}` for points and
recurses `children`; it drops `source`, `id`, `parent` from nested polygons.
`launchWorkers` then sets `id`/`source`/`filename` only on the root polygon.

## Curve Flattening

Curves become polygons in `SvgParser.polygonifyPath` (`main/svgparser.js:1498`)
and `SvgParser.polygonify` (`main/svgparser.js:1403`), using
`GeometryUtil.QuadraticBezier/CubicBezier/Arc.linearize` and a segment count
derived from `this.conf.tolerance` (which equals `config.curveTolerance`, default
`0.72` in `index.mjs`). `circle`/`ellipse` are flattened with the same
tolerance. `rect`/`polygon`/`polyline` contribute their corners directly.

After `polygonify`, the algorithm only ever sees `{x, y}` points — there is no
line/arc/bezier model anywhere below the parser. (A second simplification pass,
RDP via `main/util/simplify.js`, runs in `DeepNest.simplifyPolygon` during
`offsetTree`, tolerance `4*curveTolerance`; `config.simplify === true` instead
takes the convex hull via a global `d3` that is **not imported in Node** — known
debt, not touched here.)

## DeepNest Internal Fields

Classification of the geometry object fields:

- **A. Geometry** (cannot nest without): point arrays (`x`,`y`), `children`.
  `exact` is optional and is recomputed by the engine.
- **B. Nesting metadata**: `quantity`, `sheet`, and per-instance `id`/`source`/
  `rotation` (assigned by `deepnest.start`/GA, not present at import),
  plus `filename` pass-through (used to map results back).
- **C. SVG/import metadata**: `part.svgelements` (DOM nodes), `part.bounds`,
  `part.area` (dead), `polygon.parent`, the original `polygon.source` index.
- **D. Runtime/calculated state**: `exact` flags, spacing-offset geometry,
  NFP cache entries (`nfpcache` in `background.js`), placements (`x`,`y`,
  `rotation`, `mergedLength`, `mergedSegments`), GA `individual.rotation`.

## Worker Payload

`worker.postMessage` payload (from `deepnest.launchWorkers`):

```text
{
  index,                       // GA individual index
  sheets,                      // Array<polygon tree>        (plain)
  sheetids, sheetsources, sheetchildren,
  individual: { placement: polygonTree[], rotation: number[], processing },
  config,
  ids, sources, children, filenames   // parallel arrays (metadata that cannot cross IPC as array props)
}
```

Runtime evidence (probe on `part-hole.svg` + `bin.svg`):

```text
individual.placement[0].length        = 4
individual.placement[0] array props   = [children, id, source, filename]
individual.placement[0] point keys    = [exact, x, y]
individual.placement[0].children[0]   = 4-point hole, point keys [x, y, exact]
payload.sheets[0]                      = 4-point sheet, point keys [x, y, exact]
payload.ids/sources/filenames          = [0,1] / [1,2] / [null,null]   (bin is parts[0])
```

`background.processMessage` reassembles `parts[i].rotation/id/source/filename/
children` from the parallel arrays and then runs `placeParts`.

## DOM Boundary

DOM is required **up to and including `getParts`**:
`DOMParser`, `SVGPathElement.prototype`, `pathSegList`,
`createElementNS`, `querySelectorAll`, `ownerDocument`, `getAttribute`, and the
vendored `pathsegpolyfill`.

DOM is **no longer required** once the payload parts are built in
`deepNest.start` (`main/deepnest.js:962-969`): `cloneTree` / `offsetTree` /
`polygonOffset` / `simplifyPolygon` / clipper operate on plain arrays, and the
worker payload contains no DOM. `part.svgelements` stays on the main thread and
is used **only** by `nestingToSVG`.

> **Boundary candidate:** the object produced just before `worker.postMessage`:
> `{ quantity, sheet, polygontree: cloneTree(...), filename }`. Everything below
> that point is DOM-free.

## Result Geometry

`placeParts` returns placements `{ x, y, rotation, id, source, filename,
mergedLength?, mergedSegments? }` grouped per sheet. There is **no polygon
geometry in the result**: clients re-apply `translate(x, y) rotate(rotation)` to
their own source shape (SVG transform order: rotate about origin, then
translate). `x`,`y` are in native units (`scale` per inch). `mergedSegments` is a
list of `{x,y}` endpoint pairs.

`main/nestingToSVG.mjs` is a **renderer**, not core: it needs
`deepNest.parts[].svgelements`, `deepNest.parts[].bounds`, `deepNest.config()`
and the placements. It clones the original DOM nodes and applies the placement
transform. It can be understood as `Canonical Geometry + placements → SVG`, but
today it is coupled to the originally imported DOM nodes.

## Implemented Canonical Boundary (2026-09-25)

The candidate below is now the implemented internal boundary. The nesting
algorithm was not changed; the existing plain polygon tree became explicit.

```text
SVG string ──> src/geometry/svg-adapter.mjs ──> Canonical Geometry
                                                   │
                              src/geometry/engine.mjs nestGeometry()
                                                   │
                                         existing Deepnest engine
```

- `src/geometry/canonical.mjs` — documents/validates/normalises canonical
  geometry and deep-clones polygon trees. No SVG/DOM/HTTP.
- `src/geometry/engine.mjs` — `nestGeometry(geometry, callback, options)`:
  DOM-independent engine entry, same progress/result/abort semantics. Also
  exports `nestWithRender(...)` used only by the SVG path. It does **not**
  import the SVG parser or the SVG renderer.
- `src/geometry/svg-adapter.mjs` — `parseSvgInput(svgInput, options)` returns
  `{ geometry, renderContext }`. Uses `main/svgparser.js` and jsdom unchanged.
  `renderContext` (DOM elements, bounds) is adapter-owned and never enters
  canonical geometry.
- `index.mjs nest()` — thin composer: `parseSvgInput` → `nestGeometry` (with
  `nestingToSVG` injected as render function). Public callback/HTTP behaviour is
  unchanged.
- `main/deepnest.js` now `require`s `svgparser` lazily, so the engine is
  importable and runnable without DOM globals.

Implemented canonical shape:

```js
{
  units?: "mm",                      // metadata only; math is unit-agnostic
  sheets: [ { id, quantity?, polygontree } ],   // `bin` accepted as alias
  parts:  [ { id, quantity?, polygontree } ]
}
```

`exact`, `source`, `id`, `rotation` are engine-derived and **not** part of the
input. `svgelements`/`bounds` stay in the adapter's `renderContext`.

Limitations (unchanged, documented deliberately): current nesting semantics are
guaranteed for outer polygons with **direct** hole children; deeper topology and
multiple sheets are representable structurally but not guaranteed by the engine.

### Current implementation vs new boundary

- **Current implementation** (still in place): `main/svgparser.js` (DOM),
  `DeepNest.getParts`/`importsvg`, polygon trees on `deepNest.parts`, worker
  payload `individual.placement`, NFP/clipper/native/GA in `main/background.js`.
- **New boundary**: the explicit canonical object exchanged between the SVG
  adapter and `nestGeometry`; `nestGeometry` runs the same engine unchanged.
- `main/nestingToSVG.mjs` remains DOM-coupled and is injected only on the SVG
  path; canonical nesting works without it.

## Candidate Canonical Boundary

From the analysis above:

```text
Candidate canonical input to nesting =
  bin:   polygon tree                       (array of {x,y} with children)
  parts: [{
           id: string,                      // adapter-level, carried as filename today
           polygontree: polygon tree,       // {x,y} points + children (holes)
           quantity?: number,               // default 1
           sheet?: boolean                  // or bin/parts split at container level
         }]
  config: engine options
```

`exact` is engine-computed and must **not** be part of the canonical input.
`id`/`source`/`rotation` are engine/GA state and must not be part of it.

Answers:

1. **Minimal object needed by the nesting algorithm** — a polygon tree: arrays
   of `{x,y}` points, with optional nested `children` for holes, plus `quantity`
   and `sheet`. Verified by the synthetic probe: `start()` accepts hand-built
   trees and produces a valid worker payload with no SVG parser or DOM.
2. **Should canonical contain curves?** No. All curves are flattened to points
   by `curveTolerance` in the SVG adapter; the core has no curve concept.
3. **Is polygon points enough?** Yes, plus topology (`children`).
4. **How to represent holes?** Nested arrays via `children` (current model),
   one level consumed by NFP.
5. **Arbitrary nesting depth?** The data structure supports it; the NFP path
   only consumes direct holes. Depth >1 is untested and should be made explicit
   before relying on it.
6. **Engine metadata needed** — `quantity`, `sheet`; `filename` as an opaque
   pass-through id. `id`/`source`/`rotation` are assigned internally.
7. **Metadata that can stay in adapter/application** — client `partId`,
   `svgelements`, `bounds`, `area`, SVG attributes, rendering.
8. **Can the SVG parser return this without algorithm change?** Yes:
   `deepnest.start` already projects a part down to
   `{quantity, sheet, polygontree, filename}`; the parser could emit that shape
   directly and the engine would be unchanged.
9. **Can a JSON/geometry adapter create this directly?** Yes — demonstrated by
   the synthetic tree proof-of-concept.
10. **Where does SVG adapter responsibility end?** At producing plain polygon
    trees with `quantity`/`sheet`/id, in the engine's coordinate/units space.
    DOM, curves, transform baking and line merging belong to the adapter;
    rendering is separate.

## Open Questions

- **Islands / depth > 1**: structure supports it, NFP does not consume it. Needs
  a policy/test before canonical format promises arbitrary nesting.
- **Multiple sheets**: engine supports multiple sheet parts structurally, but
  `index.mjs` imports one bin. `sheetId` semantics for >1 sheet are unproven.
- **Units/scale ownership**: `SvgParser` bakes `scale/localscale` into points;
  a canonical format must define which units points are in.
- **Winding/orientation**: import decides holes by containment, but clipper and
  the native addon reverse based on signed area (`nfpToClipperCoordinates`
  checks `GeometryUtil.polygonArea`). Whether canonical input needs a winding
  convention is unresolved.
- **`exact` flags**: currently produced during offset/simplify; confirm they are
  purely derived (so canonical input need not carry them).
- **`mergedSegments`**: produced but not consumed by any client yet.
- **`part.area`**: set but never read — dead import metadata.
