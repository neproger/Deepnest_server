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

`npm start` listens on `http://127.0.0.1:8080`.

### HTTP API

The new Job API is versioned under `/api/v1`:

```text
POST   /api/v1/jobs              create a job (202 + jobId)
GET    /api/v1/jobs/:id          lifecycle snapshot
GET    /api/v1/jobs/:id/result   best structured placements
GET    /api/v1/jobs/:id/result.svg
GET    /api/v1/jobs/:id/events   SSE: job.started / engine.progress / result.updated / job.stopped / job.completed / job.failed
POST   /api/v1/jobs/:id/stop     stop the job (idempotent)
DELETE /api/v1/jobs/:id          delete a finished job
GET    /health
```

`POST /api/v1/jobs` body:

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

Placements use the client's own ids: `{ partId, instanceId, sheetId, x, y, rotation }`.
`placementComplete` means every part is placed in the current best result; it is
independent of `job.status === "completed"` (the genetic search may keep
improving until stopped or `execution.timeLimitMs` elapses). Concurrency is
`maxConcurrentJobs = 1`; additional jobs are queued.

The legacy `POST /nest` (multipart, SSE) endpoint is kept as a regression
baseline. See `docs/DEVELOPMENT_LOG.md` for details.

### Test

```sh
npm test          # core + native addon + HTTP server (legacy + Job API)
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
