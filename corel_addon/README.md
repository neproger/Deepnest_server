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

The active client is the C# VSTA addon in `legacy_csharp/`. The directory keeps
its historical name, but it is now the working integration path.

It exposes two Corel commands:

- `TestDeepnestConnection` sends `GET http://127.0.0.1:8080/health` and displays
  the response.
- `NestSelectedShapes` opens a WinForms dialog, reads selected Corel Curve
  shapes, submits a geometry job, polls the result, and displays placements as
  JSON.

The geometry path has been verified with two real selected Corel shapes. Both
were placed successfully and one received a 270-degree rotation.

At this stage the addon reads the document but does not modify it. Applying
placements through Duplicate/Rotate/Move is the next milestone.

## Build and load

Start the server at the repository root:

```powershell
npm start
```

Build the addon:

```powershell
cd corel_addon\legacy_csharp
.\build-addon.ps1
```

Load this generated file from CorelDRAW's **Scripts** docker:

```text
legacy_csharp\dist\CorelDeepnest.CGSaddon
```

Unload the project before rebuilding, then load it again from `dist`. No copy
of `CorelDeepnest.CGSaddon` should remain in Corel's roaming `CorelVSTA` folder,
because Corel auto-loads that copy and can show stale commands.

The complete verified setup, build, reload, troubleshooting, and package
format notes are in [`legacy_csharp/BUILD.md`](legacy_csharp/BUILD.md).

## Current geometry constraints

Every selected part must currently be:

- a Corel Curve shape;
- exactly one closed subpath;
- at least three nodes;
- composed only of straight line segments.

The sheet is a rectangle entered in the form. Sheet dimensions, spacing, and
selected-shape coordinates use the current Corel document unit consistently.
Holes, multiple contours, groups, PowerClip, text conversion, curve flattening,
and placement application are not implemented yet.
