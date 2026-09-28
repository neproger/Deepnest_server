# Deepnest Changelong
(newest on top, breaking changes)


2026-09-28 fix Corel: parts exported as `<rect>` at the origin were silently
           dropped by a leftover OnShape hack in `main/svgparser.js`; removed it,
           so rectangular parts nest again. The Corel addon now exposes the
           native OpenNest settings, rotates the layout 180° in preview and
           apply, and stops the bundled server when the nesting form closes
           (a server left behind by an older build is restarted).
2026-09-28 make OpenNest the **default** engine; Deepnest is now selected with
           `DEEPNEST_ENGINE=deepnest`. `GET /result.svg` still works for SVG
           input under OpenNest (rendered via the existing SVG renderer).
2026-09-28 add a second engine: OpenNest `nfp_nest` (C++, MIT),
           vendored under `native/opennest` and built as the `opennest` N-API
           addon; select with `DEEPNEST_ENGINE=opennest`. Supports non-rectangular
           sheets and holes/voids.
2026-09-28 remove the optional ironnest (Rust) engine, its vendored source, the
           Node-API bridge and the adapter. Deepnest is the only shipping engine.
           The pluggable-engine mechanism is kept: engines are selected/installed
           through the registry (`DEEPNEST_ENGINE`, `DEEPNEST_ENGINE_MODULE`,
           `registerEngine`).
2026-09-27 restore the original Deepnest engine (JavaScript + native Minkowski NFP
           addon) as the default; keep ironnest as an optional engine behind
           `DEEPNEST_ENGINE=ironnest`; the WASM core stays removed.
           `GET /result.svg` works again for SVG input.
2026-09-27 replace the nesting engine with `ironnest` (Rust, native Node-API addon)
           removed the original Deepnest GA/native-NFP engine, the SVGnest WASM
           core, the C++ Minkowski addon and the server-side SVG renderer;
           `GET /result.svg` now returns RESULT_FORMAT_UNAVAILABLE
           build the engine with `npm run engine:build` (requires Rust, stable)
           vendored engine in `native/vendor/ironnest` (MPL-2.0) + `native/ironnest-napi`
2023-05-15 rename `npm run` scripts: fullbuild->build-all, fullclean->clean-all
           introduced dist-all that includes a full clean rebuild with dist
2023-05-14 removed all `npm run` hardcoded filesystem references to `Dogthemachine`
           aliased the w:* commands to *
2023-05-13  `npm run w:dist` now overwrites an existent dist directory of the same name

### Code changes from cmidgley fork

Aside from improving the ability to build (mostly in `binding.gyp`, `package.json`, and
`README.md`), the following changes have also been made:

- Cloned the `plus.svg` file into `add_sheet.svg` (it was missing) for the add-sheet icon.
- Added environment variable `deepnest_debug` to open the browser dev tools (see [Browser dev
  tools](#browser-dev-tools)).
- Checked for thrown errors on a couple paths that would cause expections when closing the
  application while the nest was still running.  A better solution would be to cleanly shutdown the
  workers when the app is closed, but this works for now (but could hide important exceptions).
  The two places are marked with a `// todo:` comment in `main.js`.
- Removed a bunch of unused source files.  There are likely more to be discovered.  There also
  appears to be code that is unused within the source files.
- Eliminated the dependency on a manually downloaded instance of boost (completion of work from
  prior fork, using the `polygon` sub-repo).
- Removed all build artifacts.  This means there is no pre-built binaries in this fork and a build
  is required in order to use this.  Eventually a cleaner release solution, perhaps with ZIP or an installer,
  should be added along with making releases offical in GitHub.
- Bumped the version number to 1.0.6cm (the 'cm' to indicate this has come from the `cmidgley`
  fork).
- Eliminated the dependency on the `c:\nest` directory pre-existing, instead using the os-specific temp directory
  (`os.tmpdir()`) and creating it if it does not exist.  
- Clarified the license is MIT.  Removed an old reference to GPL and added the `license` property to
  `package.json`.  Moved the `LICENSE.txt` file to the root and renamed to `LICENSE`.
