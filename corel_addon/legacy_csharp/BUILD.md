# Building the CorelDRAW VSTA addon

This document records the working CorelDRAW 2025 VSTA workflow verified on
CorelDRAW 26.1.0.143. Follow it instead of constructing a `.CGSaddon` archive
from scratch.

## Verified architecture

```text
CorelDRAW 2025
  -> Corel VSTA project (.CGSaddon)
  -> C# Main methods marked with [CgsAddInMacro]
  -> Corel.Interop.VGCore object model
  -> HttpClient
  -> Deepnest Server http://127.0.0.1:8080
```

The following path has been tested successfully:

```text
selected Corel Curve shapes
  -> PolygonDTO JSON
  -> POST /api/v1/jobs
  -> poll /api/v1/jobs/{jobId}/result
  -> placements JSON displayed in WinForms
```

The tested result placed two selected parts, including a 270-degree rotation,
with zero unplaced parts.

## Prerequisites

- CorelDRAW Graphics Suite 2025 with **Visual Studio Tools for Applications**
  enabled in the Corel installer.
- Visual Studio 2019 or newer, or compatible Visual Studio Build Tools. The
  tested machine has Build Tools 2022 and Visual Studio Community installed.
- The .NET Framework reference assemblies installed by Visual Studio/Corel.
- `Corel.Interop.VGCore.dll`, installed by CorelDRAW under
  `Programs64\Assemblies`.
- The VS Code C# extension or C# Dev Kit is optional. It improves editing but
  does not load the addon or build the VSTA container.

The Corel-created project targets .NET Framework 4.5. Do not retarget the VSTA
template merely because command-line MSBuild cannot find the old 4.5 targeting
pack. Corel's VSTA host successfully loads and compiles the original template.

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

## Source of the addon

The active source is:

```text
VstaMacro.cs
```

It defines the partial `CorelDeepnest.Main` class and currently exposes:

```text
TestDeepnestConnection
NestSelectedShapes
```

`Macro.Internal.cs` remains inside the Corel-generated template. It owns the
real `[CgsAddInModule]` class, `[CgsAddInConstructor]` constructor, and injected
Corel `Application` instance. Do not replace it with a normal class-library
entry point.

The addon uses framework assemblies already present on Windows/CorelDRAW:

- `System.Net.Http` for the server connection;
- `System.Web.Extensions` for `JavaScriptSerializer`;
- `System.Windows.Forms` for messages and the nesting form;
- `Corel.Interop.VGCore` for the Corel object model.

No NuGet packages are required.

## Build from VS Code

Start Deepnest Server in one terminal:

```powershell
cd C:\Users\ASUS\Deepnest_server
npm start
```

Build the addon in another terminal:

```powershell
cd C:\Users\ASUS\Deepnest_server\corel_addon\legacy_csharp
.\build-addon.ps1
```

The result is always written to:

```text
dist\CorelDeepnest.CGSaddon
```

The script does not manufacture a new VSTA project and does not change its
IDs. It copies `VstaTemplate\CorelDeepnest.CGSaddon`, replaces `Macro.cs`, and
adds the `System.Net.Http` and `System.Web.Extensions` references to the
Corel-created `.csproj`. Corel compiles the source when it loads the package.

To use a different clean Corel-created template explicitly:

```powershell
.\build-addon.ps1 -CorelProject "C:\path\to\CorelDeepnest.CGSaddon"
```

## Load and reload in CorelDRAW

The working development setup uses manual loading from `dist`:

1. Open the **Scripts** docker.
2. Select **Visual Studio Tools for Applications**.
3. Click **Load**.
4. Select `legacy_csharp\dist\CorelDeepnest.CGSaddon`.
5. Expand `CorelDeepnest -> Main` and run a command.

Do not keep another `CorelDeepnest.CGSaddon` in the Corel roaming `CorelVSTA`
folder during development. Corel auto-loads that copy and may continue showing
its old commands. On the tested machine the roaming copy was removed, so the
project is loaded only from this repository.

For every update:

1. Unload `CorelDeepnest` in the Scripts docker.
2. Run `build-addon.ps1`.
3. Load the file from `dist` again.

If Corel still shows an old method list, close CorelDRAW completely, rebuild,
restart it, and load the file from `dist`. Do not load two files derived from
the same template simultaneously: they have the same internal project ID.

## Current geometry limitations

`NestSelectedShapes` currently:

- reads the active Corel selection;
- requires every selected object to be a Curve shape;
- requires exactly one closed subpath per shape;
- accepts line segments only;
- creates a rectangular sheet from the form values;
- submits one part per selected shape;
- shows the raw placements JSON;
- does not modify, rotate, duplicate, or move Corel objects yet.

Sheet width, height, spacing, and selected-shape coordinates use the current
Corel document unit consistently. Negative placement `x` or `y` values are
valid translation offsets for shapes whose source coordinates are not based at
the origin.

## Known errors

### `No entry points found`

The package was not produced from the untouched Corel-created template, its
project metadata/encoding changed, or Corel loaded a different cached copy.
Rebuild from `VstaTemplate`, remove duplicate roaming copies, unload the old
project, and load the result from `dist`.

### Only `Macro1` appears

Corel loaded the original template instead of the built package. Check that
the selected file is `legacy_csharp\dist\CorelDeepnest.CGSaddon`.

### Only an older command appears

Another package with the same internal project ID is already loaded. Unload
it or restart CorelDRAW before loading the current build.

### Server connection fails

Check `http://127.0.0.1:8080/health` and start the server with `npm start`.
The C# client uses `HttpClient`; browser CORS does not apply.
