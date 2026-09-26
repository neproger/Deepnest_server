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

The server now preserves the public API rule that one `input.parts[]` element
is one rigid part. A single SVG root polygon keeps its holes. If an exported SVG
contains several disconnected root polygons, the experiment uses their convex
hull as a safe rigid proxy. The older programmatic `nest()` API keeps its
previous behavior of treating roots independently.

## Current limitations

- SVG mode does not yet draw source outlines in the WinForms preview. Sheets,
  placement count, status, fitness, and raw JSON remain available; **Apply to
  CorelDRAW** duplicates the original Shape or Group.
- The Corel exporter and server parser must be tested for coordinate origin,
  rotation direction, physical dimensions, text, PowerClip, and effects.
- Raster `<image>` content is outside the nesting geometry model.
- Disconnected group components use a convex hull until rigid MultiPolygon is
  implemented in the engine.

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
