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

`npm start` listens on `http://127.0.0.1:8080`. The only nesting endpoint is
`POST /nest` (SSE). See `docs/DEVELOPMENT_LOG.md` for the current HTTP contract
and baseline.

### Test

```sh
npm test          # core + native addon + HTTP server
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
