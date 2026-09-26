<img src="https://deepnest.io/img/logo-large.png" alt="Deepnest" width="250">

## **Deepnest**

A fast nesting tool for laser cutters and other CNC tools

Deepnest is a node application originally based on [SVGNest](https://github.com/Jack000/SVGnest)

- New nesting engine with speed critical code written in C
- Merges common lines for laser cuts
- Support for DXF files (via conversion)
- New path approximation feature for highly complex parts

## Project Vision

See [docs/PROJECT_VISION.md](docs/PROJECT_VISION.md) for the target architecture: turning this fork into a universal headless 2D nesting service with an HTTP Job API. It defines the project boundaries and the preferred order of development. Read it before making architectural changes.

See [docs/DEVELOPMENT_LOG.md](docs/DEVELOPMENT_LOG.md) for the chronological development journal and the current known-good baseline. Run `npm test` (core + native addon + HTTP server) or `npm run server` for a manual HTTP server.

See [docs/GEOMETRY_PIPELINE.md](docs/GEOMETRY_PIPELINE.md) for the current SVG → polygon → worker geometry model and the candidate canonical-geometry boundary.

## Fork History

This repo was forked from [deepnest-io](https://github.com/deepnest-io/Deepnest) in order to make Deepnest work on node, decoupled from the electron app, refer to the [example](server.mjs).

## Prerequisites

- **Node 20+:** [Node.js](https://nodejs.org). You can use the Node Version Manager (nvm):
  - [nvm-windows](https://github.com/coreybutler/nvm-windows/releases) to download Node and change versions.
- **Python 3.7.9+** You can use the Python Version Manager (pyenv):
  - [pyenv-win](https://github.com/pyenv-win/pyenv-win) to download and change versions.
- **Visual Studio with Desktop Development with C++ extension**
  - Install VS2022 from https://visualstudio.microsoft.com/vs/features/cplusplus/
  - or, as an administrator via `npm install --global windows-build-tools` (older VS version)

### Possible Problems

- On Windows 10 1905 or newer, you might need to **disable the built-in Python launcher** via
  - **Start** > "**Manage App Execution Aliases**" and turning off the "**App Installer" aliases for Python**"
- close-and-open all command shells and your IDE to activate the latest setup

## Getting Started

```sh
git clone https://github.com/deepnest-io/Deepnest
cd Deepnest
npm install # also builds the native Minkowski addon for Node
```

### Run

```sh
npm start        # run the headless HTTP server (alias: npm run server)
node cli.mjs     # run the CLI
```

`npm start` listens on `http://127.0.0.1:8080`. This loopback host is the
default so the service is available to local CAD and browser clients but is not
exposed on the network. Browser clients from any origin can call the API: CORS
preflight requests and `Content-Type: application/json` are allowed, including
for SSE connections.

To use a different listener, set both values explicitly when starting the
server. In PowerShell:

```powershell
$env:DEEPNEST_HOST = "127.0.0.1"
$env:DEEPNEST_PORT = "8080"
npm start
```

For a LAN or reverse-proxy deployment, set `DEEPNEST_HOST=0.0.0.0` deliberately.
The API has no authentication, so do not expose that listener directly to an
untrusted network.

### HTTP API

`/api/v1` is the public HTTP boundary. The internal `nest()` is not a stable
external API and may be refactored (SVG parser, DOM, canonical geometry,
workers, native addon) without changing `/api/v1` semantics.

```text
POST   /api/v1/jobs              create a job (202 + jobId)
GET    /api/v1/jobs/:id          lifecycle snapshot
GET    /api/v1/jobs/:id/result   best structured placements
GET    /api/v1/jobs/:id/result.svg
GET    /api/v1/jobs/:id/events   SSE: job.status / job.started / engine.progress / result.updated / job.stopped / job.completed / job.failed
POST   /api/v1/jobs/:id/stop     stop the job (idempotent)
DELETE /api/v1/jobs/:id          delete a finished job
GET    /health
```

`POST /api/v1/jobs` accepts two input formats on the same endpoint. Both produce
the same structured result model.

**`format: "svg"`** — geometry is sent as SVG documents:

```json
{
  "input": {
    "format": "svg",
    "bin": { "id": "sheet-1", "data": "<svg>...</svg>" },
    "parts": [
      { "id": "part-A", "data": "<svg>...</svg>", "quantity": 1 }
    ]
  },
  "config": { "units": "mm", "spacing": 0 },
  "execution": { "timeLimitMs": 10000 }
}
```

**`format: "geometry"`** — geometry is sent as plain polygons. This path does not
use SVG, jsdom, or the SVG parser; the request is converted into Canonical
Geometry (see `docs/GEOMETRY_PIPELINE.md`). The public DTO is
`{ points: [{x,y}], children?: [...] }` (a serialisable representation; the
internal canonical polygon tree is different):

```json
{
  "input": {
    "format": "geometry",
    "units": "mm",
    "sheets": [
      {
        "id": "sheet-A",
        "quantity": 1,
        "polygontree": {
          "points": [
            { "x": 0, "y": 0 },
            { "x": 300, "y": 0 },
            { "x": 300, "y": 200 },
            { "x": 0, "y": 200 }
          ],
          "children": []
        }
      }
    ],
    "parts": [
      {
        "id": "part-A",
        "quantity": 2,
        "polygontree": {
          "points": [
            { "x": 0, "y": 0 },
            { "x": 80, "y": 0 },
            { "x": 80, "y": 50 },
            { "x": 0, "y": 50 }
          ],
          "children": [
            {
              "points": [
                { "x": 20, "y": 15 },
                { "x": 40, "y": 15 },
                { "x": 40, "y": 30 },
                { "x": 20, "y": 30 }
              ],
              "children": []
            }
          ]
        }
      }
    ]
  },
  "config": { "spacing": 2, "rotations": 4 }
}
```

```sh
# geometry input, same job lifecycle as SVG
curl -s -X POST http://127.0.0.1:8080/api/v1/jobs \
  -H 'content-type: application/json' \
  -d '{"input":{"format":"geometry","units":"mm","sheets":[{"id":"sheet-A","quantity":1,"polygontree":{"points":[{"x":0,"y":0},{"x":300,"y":0},{"x":300,"y":200},{"x":0,"y":200}],"children":[]}}],"parts":[{"id":"part-A","quantity":2,"polygontree":{"points":[{"x":0,"y":0},{"x":80,"y":0},{"x":80,"y":50},{"x":0,"y":50}],"children":[]}}]},"config":{"spacing":2}}'
# {"jobId":"<id>","status":"queued"}
```

Notes on `format: "geometry"`:

- Coordinates may use any unit, but they must be consistent; `spacing` and
  `curveTolerance` use the same unit. `units` is metadata only.
- One sheet **type** per job. `quantity` (default 1) or `mode: "auto"` control
  how many physical instances are given to the engine. `auto` expands to one
  instance per part instance; the engine opens them lazily and unused instances
  are dropped. If the provided instances run out while parts remain, the job
  returns a **partial result** — `placementComplete: false`, `unplaced:
  [{partId, instanceId}]`, `sheetsUsed: [{sheetId, instancesUsed}]` — instead of
  an error. Multiple different sheet geometries need a resource-selection
  strategy and are out of scope for now.
- `GET /result.svg` is unavailable and returns
  `400 {"error":{"code":"RESULT_FORMAT_UNAVAILABLE",...}}`. `GET /result` still
  works.
- Holes are represented as recursive `children`. Current engine semantics are
  guaranteed for an outer polygon with direct hole children; deeper nesting is
  representable but not guaranteed by NFP processing.

Full lifecycle example (SVG; identical for geometry except `result.svg`):

```sh
# 1. create (returns 202 + jobId)
curl -s -X POST http://127.0.0.1:8080/api/v1/jobs \
  -H 'content-type: application/json' \
  -d '{"input":{"format":"svg","bin":{"id":"sheet-1","data":"<svg/>"},"parts":[{"id":"part-A","data":"<svg/>"}]},"config":{"units":"mm"}}'
# {"jobId":"<id>","status":"queued"}

# 2. poll status (placementComplete is independent of status=="completed")
curl -s http://127.0.0.1:8080/api/v1/jobs/<id>

# 3. best placements (available while still running)
curl -s http://127.0.0.1:8080/api/v1/jobs/<id>/result

# 4. optional SVG rendering of the best result (SVG input only)
curl -s http://127.0.0.1:8080/api/v1/jobs/<id>/result.svg

# 5. live events (SSE)
curl -N http://127.0.0.1:8080/api/v1/jobs/<id>/events

# 6. stop, then delete
curl -s -X POST http://127.0.0.1:8080/api/v1/jobs/<id>/stop
curl -s -X DELETE http://127.0.0.1:8080/api/v1/jobs/<id>
```

Placements use the client's own ids:
`{ partId, instanceId, sheetId, sheetInstanceId, x, y, rotation }`.
`placementComplete` means every part is placed in the current best result; it is
independent of `job.status === "completed"` (the genetic search may keep
improving until stopped or `execution.timeLimitMs` elapses). Lifecycle:
`queued → running → stopped | completed | failed` (`completed` only for a
configured `execution.timeLimitMs`). Concurrency is `maxConcurrentJobs = 1`;
extra jobs are queued and start automatically.

`config` is passed through to the engine (spacing, rotations, populationSize,
mutationRate, placementType, mergeLines, curveTolerance, simplify, timeRatio,
units, scale, endpointTolerance, ...). The adapter reserves `bin`,
`progressCallback`, `timeout` and rejects them inside `config`.

### Test

```sh
npm test          # core + native addon + Job API end-to-end
npm run test:core
npm run test:server
```

### Desktop app

The Electron desktop application has been removed from this fork. Use the
upstream [deepnest-io/Deepnest](https://github.com/deepnest-io/Deepnest) for the
desktop build.

## License

The main license is the MIT.

- [LICENSE](LICENSE)

Further Licenses:

- [LICENSES](LICENSES.md)
