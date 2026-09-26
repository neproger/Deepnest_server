# Corel SVG input experiment

This experiment lives on branch `experiment/corel-svg-input`. It keeps the
direct COM geometry path available and adds an SVG path for side-by-side tests.

## Run

1. Restart Deepnest Server so it loads the experimental SVG adapter:

   ```powershell
   npm start
   ```

2. Close the existing CorelDeepnest form.
3. Build if necessary:

   ```powershell
   cd corel_addon
   .\build-addon.ps1
   ```

4. Run `NestSelectedShapes` again.
5. Open **Settings** and choose either:
   - `Geometry (COM)` for the existing `DisplayCurve` path;
   - `SVG (experimental)` for Corel SVG export and server-side flattening.

The selected input mode is persisted in
`%LOCALAPPDATA%\CorelDeepnest\settings.json`. SVG is the default on a clean
settings file for this branch.

## SVG path

Each selected top-level Corel Shape or Group is copied into a temporary Corel
document and exported as one SVG. Text shapes in the temporary copy are
converted to curves. The working document and its selection are restored, and
the temporary document and temporary file are removed.

The request uses `input.format: "svg"`, `units: "mm"`, and `scale: 25.4`, so one
engine coordinate corresponds to one millimeter. The rectangular sheet is also
sent as SVG.

For diagnostics the last export of every selected part is retained at:

```text
%LOCALAPPDATA%\CorelDeepnest\SvgExport\part-N.svg
```

The server passes every SVG through the original Deepnest import pipeline:
`load -> clean -> getParts`. A root polygon keeps its nested contours as holes.
Every disconnected outer root becomes an independent nesting part, matching
the behavior of the original Deepnest UI. When one Corel export produces more
than one root, stable result ids use `part-N#1`, `part-N#2`, and so on.

## Current limitations

- SVG mode draws the polygon trees produced by the server in the WinForms
  preview.
- The Corel exporter and server parser must be tested for coordinate origin,
  rotation direction, physical dimensions, text, PowerClip, and effects.
- Raster `<image>` content is outside the nesting geometry model.
- **Apply to CorelDRAW** is disabled when one selected Corel shape produces
  several disconnected outer roots. Original Deepnest treats those roots as
  separate physical parts; mapping them back requires splitting the source
  Corel object first. Single-root shapes can still be applied.

## Comparison matrix

Run the same selection in both modes and compare nesting and Apply results:

1. rectangle with known dimensions;
2. ellipse;
3. closed Bezier shape;
4. curve with a hole;
5. grouped overlapping shapes;
6. grouped disconnected shapes;
7. text;
8. nested group;
9. rotated and scaled objects;
10. PowerClip or another effect.

For every case check dimensions, holes, rotation direction, placement origin,
group rigidity, output appearance, point count/performance, and errors.
