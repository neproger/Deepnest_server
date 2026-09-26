# Building the CorelDRAW VSTA addon

This document records the working CorelDRAW 2025 VSTA workflow verified on
CorelDRAW 26.1.0.143. Follow it instead of constructing a `.CGSaddon` archive
from scratch.

## Verified architecture

```text
CorelDRAW 2025
  -> stable Corel VSTA loader (.CGSaddon)
  -> CorelGateway (COM access in Corel's main AppDomain)
  -> temporary Corel SVG exports
  -> temporary .NET Framework AppDomain
  -> versioned CorelDeepnest.Runtime DLL
  -> HttpClient
  -> Deepnest Server http://127.0.0.1:8080
```

The following path has been tested successfully:

```text
selected Corel shapes
  -> SVG through Corel ExportEx
  -> POST /api/v1/jobs
  -> poll /api/v1/jobs/{jobId}/result
  -> placement preview displayed in WinForms
  -> duplicate, rotate, and move shapes on a new Corel layer
```

The tested results placed two selected parts with rotated placements and zero
unplaced parts. A later manual test also confirmed a result spanning two sheet
instances and successful application of the placements to CorelDRAW.

## Prerequisites

- CorelDRAW Graphics Suite 2025 with **Visual Studio Tools for Applications**
  enabled in the Corel installer.
- Visual Studio 2019 or newer, or compatible Visual Studio Build Tools. The
  tested machine has Build Tools 2022 and Visual Studio Community installed.
- The .NET Framework reference assemblies installed by Visual Studio/Corel.
- Corel's VSTA project supplies `Corel.Interop.VGCore` to the stable loader.
  `CorelGateway` accesses COM only in Corel's main `AppDomain` and sends JSON to
  Runtime, because Corel `IDispatchComObject` values are not serializable.
- The VS Code C# extension or C# Dev Kit is optional. It improves editing but
  does not load the addon or build the VSTA container.

The Corel-created template targets .NET Framework 4.5. The build script
retargets the copied loader project and the Runtime project to .NET Framework
4.8, matching the current development toolchain. The clean Corel-created
template remains unchanged.

## Why the template is required

Corel must create the initial VSTA project through its **Scripts** docker:

1. Select **Visual Studio Tools for Applications**.
2. Choose **New -> New macro project**.
3. Name the project `CorelDeepnest`.

That produces a valid `.CGSaddon` containing Corel-generated project IDs,
metadata, `Macro.Internal.cs`, and the VSTA project flavor. A clean copy of
that generated file is stored here:

```text
VstaTemplate\CorelDeepnest.CGSaddon
```

Do not recreate `META-INF`, `Project`, `Macro.Internal.cs`, project IDs, or the
ZIP `mimetype` entry. Hand-built containers and cloned projects with modified
IDs produced `No entry points found`, even when the resulting DLL contained
the expected attributes. The `Project` entry is UTF-16, and the `mimetype`
entry is stored without compression; changing package internals can make the
VSTA loader silently reject its entry points.

## Source layout

The stable VSTA entry points are in:

```text
VstaLoader.cs
```

It defines the partial `CorelDeepnest.Main` class and exposes
`TestDeepnestConnection` and `NestSelectedShapes`. `Macro.Internal.cs` remains
inside the Corel-generated template and injects the Corel `Application` object.

The frequently changed implementation is:

```text
VstaMacro.cs
Runtime\CorelDeepnest.Runtime.csproj
Contracts\ICorelGateway.cs
Contracts\CorelGateway.cs
Contracts\GeometryExtraction.cs
Contracts\CorelDeepnest.Contracts.csproj
```

The loader creates a new `AppDomain` for every command and activates the
current Runtime DLL directly inside that domain. A small shared Contracts DLL
lets Runtime call `CorelGateway`; the gateway reads the selection and applies
placements beside the COM objects, while only JSON and primitive values cross
the domain boundary. The gateway assembly is loaded from the current version
directory, so its implementation can be updated together with Runtime. The
child domain is unloaded when the command or form closes.

Curve node coordinates from CorelDRAW are converted from the active document's
units to millimeters and then from absolute document coordinates to local part
coordinates before they are sent to the server. The sheet fields, server
payload, and placement preview therefore all use millimeters. The same local
polygons are used by the placement preview.

The addon uses framework assemblies already present on Windows/CorelDRAW:

- `System.Net.Http` for the server connection;
- `System.Web.Extensions` for `JavaScriptSerializer`;
- `System.Drawing` for rendering the placement preview;
- `System.Windows.Forms` for messages and the nesting form;
- `Corel.Interop.VGCore` in the stable loader for the Corel object model.

No NuGet packages are required.

## Build from VS Code

Start Deepnest Server in one terminal:

```powershell
cd <repository-root>
npm start
```

Build the addon in another terminal:

```powershell
cd <repository-root>\corel_addon
.\build-addon.ps1
```

The script builds `CorelDeepnest.Runtime.dll`, publishes it under a unique
version directory below `%LOCALAPPDATA%\CorelDeepnest\Runtime`, and atomically
updates `current.txt`. Keeping the canonical DLL name inside each directory is
required for .NET remoting assembly resolution. Old versions are removed when
possible. This works while CorelDRAW is running, provided the previous addon
form has been closed.

When CorelDRAW is closed, the script also removes only `CorelDeepnest.CGSaddon`
copies below the user's Corel roaming directory and VSTA temporary directories
that contain `CorelDeepnest.csproj`. Other projects are left intact. Cache
cleanup can be skipped with `-SkipCacheCleanup`.

The result is always written to:

```text
dist\CorelDeepnest.CGSaddon
```

The generated package embeds the current Runtime and Contracts DLLs. On a
different computer, copying and loading this `.CGSaddon` is sufficient; the
first command extracts the bundled binaries to
`%LOCALAPPDATA%\CorelDeepnest\Runtime`. Repository paths and the build machine's
Corel installation path are not stored in the package. Both binaries are listed
in the MSBuild project and Corel's UTF-16 `Project` manifest; without those
manifest entries Corel does not extract them into the VSTA temporary project.

The script does not manufacture a new VSTA project or change its IDs. It copies
`VstaTemplate\CorelDeepnest.CGSaddon`, replaces `Macro.cs` with the stable
loader, and changes the copied project target from .NET Framework 4.5 to 4.8.

To use a different clean Corel-created template explicitly:

```powershell
.\build-addon.ps1 -CorelProject ".\VstaTemplate\CorelDeepnest.CGSaddon"
```

## Load and reload in CorelDRAW

The working development setup uses manual loading from `dist`:

1. Open the **Scripts** docker.
2. Select **Visual Studio Tools for Applications**.
3. Click **Load**.
4. Select `corel_addon\dist\CorelDeepnest.CGSaddon`.
5. Expand `CorelDeepnest -> Main` and run a command.

Do not keep another `CorelDeepnest.CGSaddon` in the Corel roaming `CorelVSTA`
folder during development. Corel auto-loads that copy and may continue showing
its old commands. On the tested machine the roaming copy was removed, so the
project is loaded only from this repository.

After the first loader installation, the normal development loop is:

1. Close the CorelDeepnest form if it is open.
2. Edit `VstaMacro.cs`.
3. Run `build-addon.ps1` while CorelDRAW remains open.
4. Run the same Corel command again. Its title contains the Runtime build time,
   which changes after every successful compilation.

This exact loop was verified with CorelDRAW left open: `current.txt` changed to
the new build directory and the next command displayed the new Runtime build
time. A CorelDRAW restart is not required for Runtime or Contracts changes.

If `VstaLoader.cs` or the list of `[CgsAddInMacro]` commands changes, close
CorelDRAW, rebuild, restart it, and load the new `.CGSaddon`. Do not load two
files derived from the same template simultaneously: they share a project ID.

## Current geometry behavior and limitations

`NestSelectedShapes` currently:

- reads the active Corel selection;
- copies each selected top-level Shape or Group to a temporary document;
- converts text to curves in that temporary document;
- exports the selection through Corel's SVG filter;
- sends the SVG through the public `format: svg` job input;
- relies on the original Deepnest `load -> clean -> getParts` pipeline for
  curve flattening, contour containment, holes, and independent roots;
- creates a rectangular sheet from the form values;
- draws the returned placements on a sheet preview;
- polls the result every 200 ms and refreshes the preview when the server
  publishes a better result;
- draws compact centered labels (`#1`, `#2`, ...) without rotation text;
- keeps searching after `placementComplete=true`, because that flag describes
  placement coverage rather than the genetic-search lifecycle;
- lets the user stop the search and accept the latest best result;
- enables `Apply to CorelDRAW` when every selected Corel object produced one
  Deepnest outer root;
- creates a new result layer and a rectangular sheet on the active page;
- supports multiple `sheetInstanceId` values and arranges their sheets from
  left to right with a 20 mm gap;
- duplicates, rotates, and moves each placed source shape;
- leaves the source objects unchanged;
- groups the complete application operation into one undo step.

Sheet width, height, and spacing are millimeters. The request uses SVG units in
millimeters with scale `25.4`.

Form values and the Corel-relevant engine settings are persisted in
`%LOCALAPPDATA%\CorelDeepnest\settings.json`. The settings file is independent
of Runtime build directories, so hot reload and package updates retain the last
values entered by the user.

The persisted fields are sheet width and height, spacing, rotation count,
placement strategy, population size, mutation rate, SVG curve tolerance,
shared-line weight, shared-line detection, and automatic time limit. Obsolete
direct-geometry and worker-count settings are ignored.

Independent outer roots are separate parts, matching original Deepnest. If one
Corel source object produces several roots, preview and nesting remain valid,
but Apply is disabled because those roots no longer have a one-to-one source
object mapping. Raster content is outside the nesting model. PowerClip and live
effects depend on Corel's SVG export and require production testing.

## Known errors

### `No entry points found`

The package was not produced from the untouched Corel-created template, its
project metadata/encoding changed, or Corel loaded a different cached copy.
Rebuild from `VstaTemplate`, remove duplicate roaming copies, unload the old
project, and load the result from `dist`. Also verify that the stable loader has
no direct assembly reference to `CorelDeepnest.Contracts`: Contracts must be
embedded and loaded by reflection so Corel can discover the entry points before
the bundled dependencies are extracted.

### `System.Dynamic.IDispatchComObject` is not serializable

A Corel COM object crossed the temporary `AppDomain` boundary. Keep all Corel
Object Model access inside `CorelGateway` in Corel's main domain and pass only
strings or other serializable data to Runtime. The current implementation sends
the selected geometry as JSON for this reason.

### Only `Macro1` appears

Corel loaded the original template instead of the built package. Check that
the selected file is `corel_addon\dist\CorelDeepnest.CGSaddon`.

### Only an older command appears

Another package with the same internal project ID is already loaded. Unload
it or restart CorelDRAW before loading the current build.

### Server connection fails

Check `http://127.0.0.1:8080/health` and start the server with `npm start`.
The C# client uses `HttpClient`; browser CORS does not apply.
