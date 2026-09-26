# Corel SVG input

SVG export is the only Corel geometry input path. The former direct
`DisplayCurve` path was removed after side-by-side testing; the SVG work was
merged into `master`. This document describes the current pipeline.

## Run

1. Start Deepnest Server:

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
5. Select shapes and press **Run nesting**. Corel exports the temporary SVG and
   the server performs its normal SVG import and flattening.

Form settings are persisted in `%LOCALAPPDATA%\CorelDeepnest\settings.json`.
Old `inputFormat` values are ignored.

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
The Corel client marks every selected object as `rigid`, so disconnected outer
roots retain one `part-N` identity and one placement transform. Their convex
hull is used as conservative collision geometry because the current engine has
one outer polygon per rigid part. Preview metadata retains the original roots.

## Current limitations

- SVG mode draws the polygon trees produced by the server in the WinForms
  preview.
- The Corel exporter and server parser must be tested for coordinate origin,
  rotation direction, physical dimensions, text, PowerClip, and effects.
- Raster `<image>` content is outside the nesting geometry model.
- Apply always duplicates the selected Corel object. The SVG and its polygonal
  approximation are calculation and preview data only.
- A multi-root rigid object uses a convex hull for collision, which is safe but
  can leave more unused material than a native multi-polygon NFP implementation.

## Acceptance checklist

Run each selection and check nesting plus Apply results:

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
