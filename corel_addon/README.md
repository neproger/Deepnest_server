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

## Current implementation

The active client lives directly in this directory. A small C# VSTA loader
stays loaded in CorelDRAW, while the form and integration code run from a
replaceable .NET Framework Runtime DLL in a temporary `AppDomain`.

It exposes two Corel commands:

- `TestDeepnestConnection` sends `GET http://127.0.0.1:8080/health` and displays
  the response.
- `NestSelectedShapes` opens a WinForms dialog, reads selected Corel Curve
  shapes, submits a geometry job, polls the result, and displays placements as
  a sheet preview and raw JSON. The dialog can stop a running job.

The geometry path has been verified with two real selected Corel shapes. Both
were placed successfully, including rotated placements returned by the server.

At this stage the addon reads the document but does not modify it. Applying
placements through Duplicate/Rotate/Move is the next milestone.

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
builds while CorelDRAW remained open. Reload the `.CGSaddon` only after changing
`VstaLoader.cs` or the exposed command list.

The complete verified setup, build, reload, troubleshooting, and package
format notes are in [`BUILD.md`](BUILD.md).

## Current geometry constraints

Every selected part must currently be:

- a Corel Curve shape;
- exactly one closed subpath;
- at least three nodes;
- composed only of straight line segments.

The sheet is a rectangle entered in the form. Sheet dimensions and spacing are
millimeters; selected-shape coordinates are converted from the active Corel
document unit to millimeters.

Holes, multiple contours, groups, PowerClip, text conversion, curve flattening,
and placement application are not implemented yet.
