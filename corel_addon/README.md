# CorelDRAW client for Deepnest Server

`corel_addon` contains the CorelDRAW integration layer. It communicates with
Deepnest Server only through the public HTTP API:

```text
CorelDRAW
  -> C# VSTA addon
  -> Corel Object Model / PolygonDTO
  -> Deepnest Server /api/v1
```

The intended pipeline is:

```text
Corel Shape
  -> Curve/SubPaths
  -> PolygonDTO
  -> POST /api/v1/jobs
  -> placements
  -> Duplicate / Rotate / Move
```

## What works now

The active client lives directly in this directory. A small C# VSTA loader
stays loaded in CorelDRAW, while the form and integration code run from a
replaceable .NET Framework Runtime DLL in a temporary `AppDomain`.

It exposes two Corel commands:

- `TestDeepnestConnection` sends `GET http://127.0.0.1:8080/health` and displays
  the response.
- `NestSelectedShapes` opens a WinForms dialog, reads selected Corel Curve
  shapes, submits a geometry job, polls the result every 200 ms, and displays
  placements as a sheet preview and raw JSON. The preview follows improved
  results while the genetic search remains active. **Stop** accepts the latest
  best result, which can then be applied to CorelDRAW.

The geometry path has been verified with two real selected Corel shapes. Both
were placed successfully, including rotated placements returned by the server.

`Apply to CorelDRAW` creates a new `Deepnest Result HHmmss` layer on the active
page, draws every sheet instance used by the server, and places duplicated
source shapes on the corresponding sheet. Multiple sheets are arranged from
left to right with a 20 mm gap. The source objects are not changed. The complete
operation is one CorelDRAW undo step.

The preview displays each placement using the extracted polygon, including its
rotation and sheet instance. Compact labels such as `#1` are drawn near the
visual center of a part. Rotation degrees are intentionally omitted from the
label.

The addon currently provides:

- a real `/health` connectivity check through `HttpClient`;
- geometry jobs through the public `/api/v1/jobs` API;
- live updates of the latest better placement while the job is running;
- manual **Stop** and an optional automatic time limit;
- automatic use of additional instances of the rectangular sheet;
- placement preview for one or more output sheets;
- application of the accepted result back to a new CorelDRAW layer;
- extraction of rectangles, ellipses, polygons, and Curve shapes through
  Corel's non-destructive `DisplayCurve` representation;
- Bezier-to-polygon conversion, direct holes, disconnected contours, and
  nested islands;
- recursive Corel group extraction with the complete group kept as one rigid
  placement;
- persistent form and engine settings;
- versioned Runtime and Contracts DLLs for development without reloading the
  VSTA project after ordinary implementation changes.

## Recent project changes

The first connectivity-only prototype has grown into an end-to-end nesting
client. The main changes are:

- Corel COM access was moved behind `CorelGateway`; COM objects remain in
  Corel's main `AppDomain`, while only JSON and primitive values cross into the
  reloadable Runtime domain;
- the job no longer stops at the first complete placement: it continues to
  accept better server results until the user presses **Stop** or the time limit
  expires;
- the form now renders the current result, reports fitness and sheet count, and
  retains the raw server response for diagnostics;
- server placements can be applied by duplicating, rotating, and moving the
  selected source objects;
- multiple `sheetInstanceId` values are rendered and applied as separate sheet
  rectangles;
- nesting options were moved into a compact expandable **Settings** row and are
  saved between sessions;
- the build publishes versioned runtime directories and embeds a portable copy
  of Runtime and Contracts into `dist\CorelDeepnest.CGSaddon`.
- geometry extraction now uses `DisplayCurve.GetCopy()` and Corel's polyline
  conversion; contour containment is calculated without relying on winding;
- disconnected solids and islands inside holes become independent numbered
  parts, while immediate child contours are sent as holes.

## Build and load

Start the server at the repository root:

```powershell
npm start
```

Build the addon:

```powershell
cd corel_addon
.\build-addon.ps1
```

Load this generated file from CorelDRAW's **Scripts** docker:

```text
dist\CorelDeepnest.CGSaddon
```

The generated `.CGSaddon` contains the bundled Runtime and Contracts DLLs and
can be copied to another computer as one file. On first use the loader installs
them under `%LOCALAPPDATA%\CorelDeepnest\Runtime`. Subsequent local builds
publish a versioned directory containing the canonical DLL names; close the
addon form, run the build, and invoke the command again without restarting
CorelDRAW. This hot-reload loop was verified with two consecutive Runtime
builds while CorelDRAW remained open. Runtime and Gateway implementations are
loaded from the current version directory. Reload the `.CGSaddon` only after
changing `VstaLoader.cs` or the exposed command list.

`placementComplete` only means that the current result contains every selected
part. It does not stop the search. Leave the job running while its fitness or
layout improves, then click **Stop** to keep the latest result and enable
**Apply to CorelDRAW**.

The compact top row contains sheet width, sheet height, spacing, and rotation
count. **Settings** expands the engine options relevant to Corel geometry:

- placement strategy (`gravity`, `box`, or `convexhull`);
- population size and mutation rate;
- worker threads;
- Corel curve detail (`1..100`, default `50`) used when curves are converted to
  polygon points;
- geometry tolerance;
- shared-line fitness weight and shared-line detection;
- optional automatic time limit (`0` keeps manual Stop behavior).

The last entered values are restored when the addon opens again. They are saved
outside the repository in:

```text
%LOCALAPPDATA%\CorelDeepnest\settings.json
```

SVG/DXF conversion options and internal numeric constants are not shown because
the Corel client sends already-extracted polygon geometry. The engine's
experimental `simplify` option is also omitted because it is not currently safe
in the headless server runtime.

The complete verified setup, build, reload, troubleshooting, and package
format notes are in [`BUILD.md`](BUILD.md).

## Current geometry support

The addon now accepts Corel shapes that provide a `DisplayCurve`, including the
main primitive and curve types:

- rectangles, including rounded rectangles;
- ellipses and circles;
- polygons and stars represented by Corel as curves;
- line and Bezier Curve shapes;
- compound curves containing one or more closed contours;
- outer contours with one or more direct holes;
- disconnected outer contours and islands nested inside holes;
- nested Corel groups whose leaf shapes expose closed `DisplayCurve` contours.

The sheet is a rectangle entered in the form. Sheet dimensions and spacing are
millimeters; selected-shape coordinates are converted from the active Corel
document unit to millimeters.

Extraction follows this pipeline:

```text
Shape.DisplayCurve.GetCopy()
  -> closed SubPaths
  -> Corel GetPolyline(curve detail)
  -> duplicate/collinear point cleanup
  -> containment tree
  -> PolygonDTO with direct holes
```

Every even-depth solid becomes a separately placeable part. Its immediate
odd-depth children become holes. This means multiple outer contours and an
island inside a hole can move independently in the nesting result. The Apply
operation creates a Curve for each extracted component, preserving its original
Bezier geometry and copying fill and outline properties from the source shape.

A selected Corel group follows a different policy: its child shapes are walked
recursively and the group is submitted as one rigid part. Until the server has
a true multi-root `MultiPolygon` part, nesting uses the convex hull of all group
contours. Apply duplicates the original group, so every child and disconnected
island keeps its exact relative position and appearance. The empty space between
disconnected group members is conservatively treated as occupied.

Open, degenerate, self-intersecting, touching, or crossing contours are rejected
with a shape and contour number. Each flattened contour is limited to 10,000
points. Geometry is not silently welded or repaired.

PowerClip, text conversion, outline-to-object conversion, effects, exact
multi-root group nesting, and self-intersection repair remain future work.
Unsupported leaf objects should be converted or flattened on a copy in Corel
first.
