# Deepnest Server Development Log

Рабочий журнал разработки.

Здесь фиксируются выполненные этапы, эксперименты, найденные проблемы,
временные решения и следующие действия.

Это не замена PROJECT_VISION.md.

- PROJECT_VISION.md — куда идет проект.
- DEVELOPMENT_LOG.md — что фактически происходило при разработке.

---

## 2026-09-25 — Existing HTTP server baseline

### Goal

Поднять существующий HTTP server без архитектурного рефакторинга
и доказать реальный nesting через HTTP. Зафиксировать baseline, против
которого будут проверяться будущие удаления legacy-кода.

### Starting state

- Node version: `v20.19.5`
- npm version: `11.10.0`
- native addon: `build/Release/addon.node` present (собран `npm install` через node-gyp)
- core tests: только `tests/core/nest.test.mjs` (создан на предыдущем этапе)
- relevant commit: `d0f26e3 docs: add project vision and link from README`
- uncommitted changes:
  - `index.mjs` (worker path через `import.meta.url`, идемпотентный abort)
  - `package.json` (scripts `test`/`test:core`/`test:ui`)
  - untracked: `tests/core/`, `tests/fixtures/`
- server dependencies: `express`, `busboy`, `opentype.js`, `sse.js` установлены (не удалялись)

#### Note on the task brief

В задании часть контекста не совпадала с фактическим репозиторием, поэтому
фиксирую реальное состояние:

- `tests/core/native-addon.test.mjs` **не существовал** на старте этапа — добавлен в рамках этого этапа.
- Сборка/проверки выполнялись на **Node 20.19.5**, а не на Node 24.
- `simplify:true` — только **подозрение** на проблему (`deepnest.js` вызывает `d3.polygonHull`, но `d3` в `deepnest.js` не импортирован). Не проверялось, не исправлялось.

### Previously confirmed (before this stage)

- headless Node nesting работает (`tests/core/nest.test.mjs`);
- native addon собирается и загружается;
- Electron runtime для core не нужен;
- CLI выполняет реальный nesting и пишет `result.svg` + `data.json`, но **не завершается сам после `timeout`** (спиннер `ora` держит event loop);
- worker path больше не зависит от CWD (исправлено в `index.mjs` через `import.meta.url`);
- утечка SIGINT-listener убрана (abort идемпотентен и снимает listener);
- module-level `eventEmitter` в `index.mjs` — **не исправлено**;
- `source/id/filename` — legacy-семантика (см. ниже);
- `docs/PROJECT_VISION.md` создан, commit `d0f26e3`.

### Tried

#### 1. Осмотр существующего server и способа запуска

Command:

```bash
# чтение server.mjs, package.json
```

Result:

- `server.mjs` на верхнем уровне вызывал `start()`, который **сразу делал `app.listen(8080)`** и перед этим **синхронно тянул `https://api.fontsource.org/v1/fonts`**.
- Из-за этого сервер нельзя было импортировать в тест без auto-listen и без сетевого запроса.
- Роуты: `GET /`, `GET /upload` (HTML), `POST /nest` (SSE), `GET /text` (HTML), `POST /nest/text` (SSE).
- `POST /nest` ждёт `multipart/form-data` (busboy): файлы с `mimeType === image/svg+xml` + поле `config` (JSON).
- `bin` захардкожен `{width:3000,height:1000}`, но переопределяется через `config` (spread идёт после дефолтов).

#### 2. Минимальное изменение для testability

Разделил сборку `app` и запуск:

- `export const app` и `export async function start({ port, host, prefetchFonts })`;
- font prefetch стал best-effort (`try/catch`), не блокирует listen;
- auto-start только при прямом запуске (`import.meta.url === pathToFileURL(process.argv[1]).href`).

HTTP-контракт роутов не менялся.

#### 3. Первый прогон server integration test

Command:

```bash
node --test tests/server/server.test.mjs
```

Result:

- тест `ok`, subtest `~694 ms`, но **весь файл — `duration_ms 12642`**.
- Заподозрил, что после abort остаются workers/timers.

#### 4. Дебаг «утечки» (оказалась моей ошибкой измерения)

Скрипт: поднять server → реальный SSE nesting → abort → `server.close()` → печать `process.getActiveResourcesInfo()`.

Наблюдение: после закрытия сервера в логе висели `['PipeWrap','PipeWrap','Timeout']`, процесс жил до принудительного `process.exit`.

Причина: **наш собственный guard-`setTimeout`** удерживал event loop. Аналогичный дебаг core-пути без guard завершился сам:

```bash
# coredbg2: abort resolved at 436ms, beforeExit at 438ms, exit at 438ms
```

Отдельный дебаг server без guard:

```bash
# srvdbg2: server closed at 503ms, beforeExit at 508ms, exit at 508ms
```

Вывод: server и core завершаются чисто за ~1s; 12.6s в первом прогоне — разовый оверхед (первый запуск `node --test`/скан Windows), повторные прогоны `~1s`.

### Successful

* Существующий HTTP server запускается как есть (после minimal testability-разделения) и отвечает `GET /` → 200 HTML.
* Реальный nesting проходит по HTTP: `POST /nest` → `nest()` → Deepnest → worker_threads → native addon → placements.
* SSE отдаёт `connection`, `progress`, `response` события.
* Результат получен и проверен: `status.complete=true`, placements с `x/y/rotation/source/id`.
* Серверный сценарий автоматизирован (`tests/server/server.test.mjs`) и воспроизводим.
* `npm test` гоняет core + server + native addon: 3/3 pass, ~1.2s.
* Сервер и тест не оставляют живых workers/timers после завершения.

### Problems found

#### Problem: server нельзя импортировать без auto-listen и сетевого font prefetch

Observed: `server.mjs` вызывал `start()` на верхнем уровне и до `listen()` тянул Fontsource.

Cause: demo-ориентированный монолитный `server.mjs`.

Resolution: выделены `app`/`start()`, prefetch сделан best-effort, auto-start только при direct run. Роуты не тронуты.

#### Problem: CLI не завершается сам после `timeout`

Observed: `node cli.mjs -t 8000 ...` пишет результат, но процесс не выходит.

Cause: спиннер `ora` держит event loop; abort ядра при этом срабатывает корректно (проверено отдельно mid-flight).

Status: unresolved (презентационный слой CLI; на baseline сервера не влияет).

#### Problem: `simplify:true` может падать в Node

Observed: в `main/deepnest.js` `getHull` использует `d3.polygonHull`, но `d3` не импортирован (в Electron он был глобальным из `<script>`).

Cause: неявная зависимость от renderer-глобалов.

Status: unresolved / не проверялось. `simplify:false` — дефолт, baseline не затронут.

#### Problem: module-level `eventEmitter` в `index.mjs`

Observed: emitter общий на модуль; параллельные `nest()` будут перекрёстно получать события.

Cause: emitter создан на уровне модуля.

Status: unresolved. Для server baseline достаточно `maxConcurrentJobs = 1`.

#### Problem: abort/NFP listeners не снимаются после job

Observed: `nest()` добавляет `background-*`/`placement` listeners на общий emitter и не удаляет их.

Status: unresolved; проявится при нескольких последовательных job'ах (утечка listeners). Baseline с одним job чист.

#### Problem: `source` — не индекс `elements`, а индекс `DeepNest.parts` с офсетом листа

Observed: 4 детали → `source: 1..4` (лист занимает `parts[0]`).

Cause: `index.mjs` импортирует лист первым; `launchWorkers` пишет `poly.source = i` по общему массиву parts.

Status: known legacy semantics; внешний `partId` не поддерживается.

### Changes made

- `server.mjs` — выделены `export const app` и `export async function start({port,host,prefetchFonts})`; font prefetch best-effort; auto-start только при direct run. Причина: testability без смены HTTP-контракта.
- `tests/server/server.test.mjs` — новый end-to-end тест существующего server (реальный HTTP+SSE+nest, без mock).
- `tests/core/native-addon.test.mjs` — новый прямой smoke-тест native addon (`calculateNFP`).
- `package.json` — `test` = `node --test tests/`; добавлены `test:server` и `server` (`node server.mjs`). `start` оставлен как Electron (`electron .`).
- `docs/DEVELOPMENT_LOG.md` — этот документ.
- `README.md` — ссылка на DEVELOPMENT_LOG рядом с PROJECT_VISION.

### Verification

```bash
npm test
```

Results:

* core (`tests/core/nest.test.mjs`): PASS
* native addon (`tests/core/native-addon.test.mjs`): PASS
* server (`tests/server/server.test.mjs`): PASS
* итого: `# tests 3 / # pass 3 / # fail 0`, `~1194 ms`

Ручной запуск сервера:

```bash
npm run server
# Server listening on http://localhost:8080
```

Проверка вручную:

```bash
GET http://127.0.0.1:8080/ -> 200 (HTML)
```

Реальный HTTP nesting (сэмпл, `config={units:"mm",spacing:0,bin:{width:200,height:100}}`, 2×parts.svg):

```text
HTTP 200 text/event-stream
event: connection
event: progress {"progress":0,"phase":"NFP"}
event: response -> keys: [ 'svg', 'data', 'status' ]
  status: {"better":true,"complete":true,"placed":4,"total":4}
  placements: 4
  first placement: {"x":113.38,"y":258.68,"id":0,"source":1,"rotation":180,"filename":"part-a.svg",...}
  svg starts: <svg xmlns="http://www.w3.org/2000/svg" width="200.000...
```

### Current known-good baseline

Гарантированно работает на Node 20.19.5:

1. `npm install` собирает native addon в `build/Release/addon.node`.
2. `npm test` (core + native + server) — 3/3 pass.
3. `npm run server` поднимает существующий HTTP server на `:8080`.
4. `POST /nest` принимает multipart (`image/svg+xml` файлы + `config`), выполняет реальный nesting и стримит SSE (`connection`/`progress`/`response`).
5. `GET /` отдаёт demo HTML.
6. Core abort останавливает worker; server/core завершаются сами без висящих threads.

### Next

1. Зафиксировать/закоммитить код baseline (server testability + tests + scripts).
2. Добавить в server test явную проверку отсутствия висящих handles (например, сравнение `process.getActiveResourcesInfo()` или `worker` registry).
3. Начать безопасную очистку legacy: после каждого удаления прогонять `npm test` (core + server).
4. Кандидаты на удаление (проверять import-ы перед удалением): Electron (`main.js`, `main.test.js`, `robot.js`, `tests/index.spec.ts`, `playwright.config.ts`, UI-ассеты `main/index.html`/`style.css`/`img`/`font`, UI-only util `ractive/interact/svgpanzoom/filesaver/...`).
5. Не трогать `server.mjs` роуты, пока HTTP contract не стабилизирован (см. PROJECT_VISION, этап 3).

---

## 2026-09-25 — Legacy cleanup (Electron/UI/demo removal)

### Goal

Удалить legacy desktop/UI/demo части репозитория, **не меняя поведение `/nest`**.
HTTP baseline (см. предыдущую запись) — regression contract.

### Starting state

- baseline commit: `b41fd0d test: add headless core, native addon and HTTP server baselines`
- `npm test` = 3/3 PASS
- tracked files at baseline: 200

### Tried

Порядок: сначала фиксируем baseline коммитом, затем режем небольшими группами с прогоном `npm test` после каждой.

#### 0. Commit baseline

```bash
git add README.md index.mjs package.json server.mjs docs/DEVELOPMENT_LOG.md tests/core tests/fixtures tests/server
git commit -m "test: add headless core, native addon and HTTP server baselines"
# -> b41fd0d ; npm test 3/3 PASS
```

#### 1. Electron runtime + UI test config

Проверка references (`grep`): `main.js`/`main/preload.js` не требуются из core/server;
`main.test.js`/`robot.js`/`tests/index.spec.ts`/`playwright.config.ts` — desktop Playwright только.

Удалено: `main.js`, `main.test.js`, `robot.js`, `main/preload.js`, `tests/index.spec.ts`, `playwright.config.ts`.
Также удалены desktop-only CI workflows `.github/workflows/{build,playwright,robot}.yml` (ссылались на удалённые файлы/electron-packager).
`package.json`: `start` → `node server.mjs`, `main` → `index.node.mjs`, удалены `exec`/`test:ui`/`serve`/`build*`/`dist*`/`copy`/`clean-all`.

Result: `npm test` 3/3 PASS; `npm start` поднял headless server.

#### 2. Desktop/UI assets

Удалено: `main/index.html`, `main/style.css`, `main/background.html`, `main/img/`, `main/font/`, `icons/`, `icon.ico`, `icon.icns`.

Result: `npm test` 3/3 PASS.

#### 3. Util candidates (по одному, с проверкой references)

`grep` по `require/import` подтвердил: ни один не требуется из core/server.

Удалено (UI-only / legacy unused): `ractive.js`, `interact.js`, `svgpanzoom.js`, `filesaver.js`, `json.js`, `hull.js`, `parallel.js`, `placementworker.js`, `eval.js`, `clippernode.js`, `domparser.js`.

Оставлено (runtime dependency ядра): `clipper.js`, `d3-polygon.js`, `geometryutil.js`, `matrix.js`, `pathsegpolyfill.js`, `simplify.js`.

Result: `npm test` 3/3 PASS.

#### 4. Demo text/font + demo HTML

`server.mjs`: удалены `GET /`, `GET /upload`, `GET /text`, `POST /nest/text`, `getFont`, Fontsource prefetch, `subscribeToSeverEvents`. Добавлен `GET /health`. `start()` больше не имеет `prefetchFonts`.

Result: `POST /nest` не менялся; `npm test` 3/3 PASS; `npm start` + `GET /health` → 200.

#### 5. npm dependencies

Удалены: `electron`, `@electron/packager`, `@electron/rebuild`, `@electron/remote`, `@playwright/test`, `nodemon`, `shx`, `axios`, `graceful-fs`, `opentype.js`, `pathseg`; удалён блок `build` (electron-builder).

`npm install` → `removed 296 packages, audited 155 packages`.

Result: `npm test` 3/3 PASS.

### Successful

* Electron runtime полностью удалён из кода и зависимостей.
* Desktop UI/assets и UI-only util удалены.
* Demo `/text`, `/nest/text`, `/upload`, Fontsource, `opentype.js` удалены.
* `npm start` запускает headless HTTP server (`GET /health` → 200, `POST /nest` работает).
* `npm test` = core + native addon + HTTP server, 3/3 PASS без mocks.
* Tracked files: 200 → 105.

### Problems found

#### Problem: что выглядело unused, но оказалось runtime dependency

- `d3-polygon.js` — **нужен** (`background.js` использует `d3.polygonHull`). Legacy `hull.js` (Graham scan) удалён, но настоящий convex hull живёт в `d3-polygon.js` и в `deepnest.js`/`background.js`.
- `pathseg` (npm) — **не использовался вообще**: `svgparser.js` использует vendored `main/util/pathsegpolyfill.js`, а не npm-пакет.
- `sse.js` (npm) — после удаления demo HTML **стал неиспользуемым сервером** (сервер пишет SSE сам). Оставлен по явному указанию «не удалять пока», до этапа замены HTTP-обвязки.
- `graceful-fs` — удалён как прямая зависимость, но остаётся в `node_modules` транзитивно через `fs-extra`.

#### Problem: удаление `.github/workflows`

Workflows ссылались на удалённые `main.test.js`/playwright-конфиг и на electron-packager. Удалены целиком; headless CI пока не добавлен.

Status: unresolved (CI for headless — отдельная задача).

### Changes made

- удалены 95 tracked-файлов (Electron/UI/assets/util/demo);
- `server.mjs` — оставлены только `GET /health` и `POST /nest`; `start()` без font prefetch;
- `package.json` — headless scripts (`start`/`server`/`test*`), `main` → `index.node.mjs`, очищены deps;
- `package-lock.json` — пересчитан `npm install`;
- `README.md` — разделы Run/Test/Desktop переписаны под headless, Electron-инструкции убраны;
- `docs/DEVELOPMENT_LOG.md` — эта запись.

### Verification

```bash
npm test
```

Results:

* core (`tests/core/nest.test.mjs`): PASS
* native addon (`tests/core/native-addon.test.mjs`): PASS
* server (`tests/server/server.test.mjs`): PASS
* итого: `# tests 3 / # pass 3 / # fail 0`

Ручная проверка:

```bash
npm start
# Server listening on http://localhost:8080
GET /health -> 200 {"status":"ok"}
GET /       -> 404 (demo HTML удалён)
POST /nest  -> 200 text/event-stream (контракт не менялся)
```

Проверка зависимостей:

```bash
# в коде не осталось require/import удалённых пакетов
# единственное упоминание — sse.js (оставлен осознанно)
```

### Current known-good baseline

1. `npm install` собирает native addon (`build/Release/addon.node`).
2. `npm test` — 3/3 PASS (core + native addon + HTTP server), ~1.2s.
3. `npm start` / `npm run server` — headless HTTP server на `:8080`.
4. `POST /nest` — тот же контракт, что до очистки (multipart SVG + `config`, SSE `connection`/`progress`/`response`).
5. `GET /health` — новый простой health endpoint.
6. Core-файлы: `index.mjs`, `index.node.mjs`, `main/{deepnest,background,svgparser,nestingToSVG,processPair,processPairs.node}.{js,mjs}`, `main/util/{clipper,d3-polygon,geometryutil,matrix,pathsegpolyfill,simplify}.js`, `src/**`, `binding.gyp`.

### Next

1. Отдельный этап: заменить demo-контракт `/nest`, ввести Job Manager и `/api/v1/jobs` (см. PROJECT_VISION, этапы 3–6).
2. `sse.js` можно удалить одновременно с заменой HTTP-обвязки.
3. Добавить headless CI (сборка addon + `npm test`).
4. Не смешивать cleanup с behavioral refactoring (engine issues остаются в debt, см. PROJECT_VISION `Known Architectural Debt`).
