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

- polls the current best result every 200 ms;
- renders parts, holes, rotations, and multiple sheet instances;
- keeps searching until **Stop** or the configured time limit;
- saves the last entered settings;
- can apply single-root results to a new `Deepnest Result HHmmss` layer;
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

## Settings

The compact row contains:

- sheet width and height in millimeters;
- spacing in millimeters;
- allowed rotation count.

The expandable settings row contains:

- placement strategy;
- population size and mutation rate;
- worker count;
- SVG curve tolerance;
- shared-line detection and fitness weight;
- optional automatic time limit.

Settings are stored in:

```text
%LOCALAPPDATA%\CorelDeepnest\settings.json
```

Obsolete `inputFormat` and `corelCurvePrecision` values from older settings
files are ignored.

## SVG diagnostics

The most recent Corel exports are retained for inspection at:

```text
%LOCALAPPDATA%\CorelDeepnest\SvgExport\part-N.svg
```

The request uses `input.format: "svg"`, `units: "mm"`, and `scale: 25.4`, so
server placement coordinates correspond to millimeters.

Raster image content is outside the nesting geometry model. PowerClip and live
effects depend on how Corel expands them during SVG export and should be tested
before production use.

## Main files

- `VstaLoader.cs` — stable VSTA entry points and hot-reload loader.
- `VstaMacro.cs` — HTTP client, form, preview, settings, and job lifecycle.
- `Contracts/CorelGateway.cs` — Corel selection export and placement apply.
- `build-addon.ps1` — builds DLLs, publishes Runtime, and creates the package.
- `dist/CorelDeepnest.CGSaddon` — generated package loaded by CorelDRAW.
