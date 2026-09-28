# CorelDRAW client for Deepnest Server

`corel_addon` is the CorelDRAW integration layer. It accesses Corel through the
Corel Object Model and communicates with Deepnest Server only through the
public HTTP API.

```text
CorelDRAW selection
  -> Corel SVG export
  -> POST /api/v1/jobs
  -> Deepnest SVG import pipeline
  -> placements
  -> CorelDRAW preview / Apply
```

The addon does not import internal server modules. The old direct
`DisplayCurve`/PolygonDTO extraction path has been removed; SVG is the only
Corel input path.

## What works

The addon exposes two commands:

- `TestDeepnestConnection` calls `GET http://127.0.0.1:8080/health`.
- `NestSelectedShapes` opens the nesting form for the current Corel selection.

For each selected top-level Shape or Group, the addon creates a temporary Corel
document, converts text in the temporary copy to curves, and exports the
selection as SVG. The active document and source objects are not modified.

The server uses the original Deepnest SVG pipeline:

```text
load -> clean -> getParts -> polygon trees -> nesting
```

Nested contours become holes. Independent outer contours become independent
physical parts, matching the original Deepnest UI. If one Corel export contains
several outer roots, their result ids are `part-N#1`, `part-N#2`, and so on.

The form:

- polls the current best result every 200 ms off the UI thread (updates are
  marshalled back to the form), so the UI stays responsive;
- shows a compact status line with the best-variant index, density, part and sheet
  counts, and elapsed time (the engine reports no solve percentage);
- renders parts, holes, rotations, and multiple sheet instances in a single row
  (each sheet stretched to the full available height) with horizontal scrolling;
- keeps searching until **Stop** or the configured time limit;
- stops instantly: the stop request is sent asynchronously and the server
  acknowledges it immediately, tearing the engine down in the background;
- saves the last entered settings;
- starts the bundled server when it opens and stops it when it closes, so no
  orphan `node` process is left behind (a server left by an older build is
  detected and restarted);
- can apply single-root results to a new `Deepnest Result HHmmss` layer (the
  layout is rotated 180° about the sheet centre to match the preview);
- groups the complete Apply operation into one CorelDRAW undo step.

**Apply to CorelDRAW** is disabled if a selected Corel object produced several
independent outer roots. Deepnest treats those roots as separate parts, while
Corel still exposes one source object; applying them correctly requires the
source object to be split first.

## Build and load

Requirements:

- CorelDRAW with VSTA support;
- .NET Framework 4.8 build tools;
- MSBuild available through Visual Studio Build Tools or Visual Studio.

Start the server from the repository root:

```powershell
npm start
```

Build the addon:

```powershell
cd corel_addon
.\build-addon.ps1
```

Load this file from CorelDRAW's **Scripts** docker:

```text
corel_addon\dist\CorelDeepnest.CGSaddon
```

The package contains the stable VSTA loader plus bundled Runtime and Contracts
DLLs. A build made while CorelDRAW is running publishes a versioned Runtime to:

```text
%LOCALAPPDATA%\CorelDeepnest\Runtime
```

Close the addon form and invoke the command again to use that Runtime. A
CorelDRAW restart is only needed after changing `VstaLoader.cs`, the exposed
command list, or the `.CGSaddon` package itself.

See [`BUILD.md`](BUILD.md) for the verified setup, cache behavior, package
layout, and troubleshooting steps.

## Distribution (end-user install)

`build-distribution.ps1` (repository root) produces:

```text
dist\DeepnestCorel\
  CorelDeepnest.CGSaddon     the VSTA addon, with the whole server embedded
  README.txt
```

The `.CGSaddon` embeds the Node server (`server.zip`) next to the Runtime and
Contracts. On the first command the loader extracts the server to
`%LOCALAPPDATA%\CorelDeepnest\Server\<hash>` and the addon starts
`node.exe server.mjs` from there (after checking `http://127.0.0.1:8080/health`).
The end user only loads `CorelDeepnest.CGSaddon` in CorelDRAW (Scripts docker →
Visual Studio Tools for Applications → Load): no Node install, no folder to
choose, no manual server start.

(A folder-picker fallback remains for development packages without an embedded
server; the distributed package never needs it.)

See [../docs/DISTRIBUTION.md](../docs/DISTRIBUTION.md) for the full build and
release notes.

## Settings

The compact row contains:

- sheet width and height in millimeters;
- spacing in millimeters (minimum gap between parts);
- edge offset in millimeters (minimum gap from the sheet edge; `sheetSpacing`);
- rotation variant count (`4` means 0°, 90°, 180°, and 270°).

The expandable settings row contains the native OpenNest (`nfp_nest`) engine
settings:

- **Packing** — `placementType`: `Box`, `Gravity`, or `Squeeze`;
- **Population** — genetic population size (`populationSize`);
- **Mutation, %** — mutation rate (`mutationRate`);
- **Seed** — RNG seed (`seed`);
- **Generations** — generations per solve (`generations`);
- **All rotations** — try every rotation, ignoring the rotation count
  (`tryAllRotations`);
- **Exact NFP** — use the exact (slower) no-fit-polygon (`exactNfp`);
- **Exact voids** — exact hole handling (`exactVoids`);
- **Tolerance, mm** — curve flattening tolerance (`curveTolerance`);
- **Limit, sec** — optional automatic stop (`timeLimitSeconds`; `0` = run until
  Stop).

The engine derives the edge gap from two inputs: the **edge offset** above, and
a conservative safety offset it applies around every simplified part and sheet.
That safety offset is `8 × Tolerance` for non-rectangular parts (e.g. `Tolerance
= 1` mm adds ~8 mm to the visible edge gap), while simple rectangular parts use a
fast path with no such offset. To get a tight edge, keep `Tolerance` small
(e.g. `0.1`–`0.3`) and set the edge offset explicitly.

Settings are stored in:

```text
%LOCALAPPDATA%\CorelDeepnest\settings.json
```

Obsolete keys from older settings files are ignored.

## SVG diagnostics

The most recent Corel exports are retained for inspection at:

```text
%LOCALAPPDATA%\CorelDeepnest\SvgExport\part-N.svg
```

The request uses `input.format: "svg"`, `units: "mm"`, and `scale: 25.4`, so
server placement coordinates correspond to millimeters. The sheet is sent with
`mode: "auto"`; additional identical sheet instances are opened when the parts
do not fit on the first sheet. Preview and Apply arrange those sheets from left
to right.

Every selected Corel object is sent as one rigid part. Apply always duplicates
the original Corel object and converts the SVG top-left/downward coordinate
system back to Corel document coordinates; server-generated SVG is never used
as output artwork. An object's **body is derived from the SVG fill** (filled =
material, unfilled = hole). If that material is a single filled region it is
used directly; if it is several regions (disjoint bodies like the two parts of
an "i", or nested rings) the part uses their convex hull, so nothing can overlap
it. Merge/weld shapes in CorelDRAW to keep an object single-region. If an export contains several independent outer roots, the
server uses their convex hull only as conservative collision geometry. The
preview still draws the original roots.

Raster image content is outside the nesting geometry model. PowerClip and live
effects depend on how Corel expands them during SVG export and should be tested
before production use.

## Main files

- `VstaLoader.cs` — stable VSTA entry points and hot-reload loader.
- `VstaMacro.cs` — HTTP client, form, preview, settings, and job lifecycle.
- `Contracts/CorelGateway.cs` — Corel selection export and placement apply.
- `build-addon.ps1` — builds DLLs, publishes Runtime, and creates the package.
- `dist/CorelDeepnest.CGSaddon` — generated package loaded by CorelDRAW.
