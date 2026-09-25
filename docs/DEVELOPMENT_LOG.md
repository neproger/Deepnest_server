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

Cleanup commit: `d07ab1e` (baseline commit: `b41fd0d`).

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

Дополнительно (follow-up cleanup commit): удалены `.vscode/launch.json` (Electron debug config) и `main/readme.md` (SVGNest browser UI readme). Tracked files: 200 → 103.

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

---

## 2026-09-25 — HTTP Job API v1 (`/api/v1/jobs`)

### Goal

Построить первый собственный API проекта — долгоживущий nesting Job поверх
существующего `nest()`, не меняя nesting algorithm/geometry pipeline и не
удаляя legacy `POST /nest`.

Job API commit: `e47e9a4`.

### Starting state

- baseline: `b41fd0d`; cleanup: `d07ab1e`, follow-up `8e831ba`
- `npm test` = 3/3 PASS
- core: `index.mjs`, `main/**`, `src/{addon,minkowski}.cc`
- server: `server.mjs` (`GET /health`, `POST /nest`)

### Design

- **Lifecycle ≠ placement quality.** `status.complete` от Deepnest означает «в
  текущем placement размещены все детали», а не «генетический поиск завершён».
  Поэтому `job.status` не переходит в `completed` по `placementComplete`.
  Отдельно хранится `placementComplete` (best placement state).
- Job states: `queued → running → stopped | completed | failed`.
  `completed` только при заданном нормальном stop condition (`execution.timeLimitMs`).
  Без limit job работает до явного stop.
- `maxConcurrentJobs = 1`, лишние jobs в `queued` (не ошибка), следующий
  стартует автоматически после stop/finish.
- External IDs: `part.id`/`bin.id` → placements `partId`/`instanceId`/`sheetId`.
  Legacy `source/id/filename` спрятаны в `placement.raw`.
- Structured placements — основной контракт; SVG вынесен в
  `GET /api/v1/jobs/:id/result.svg`.

### Tried

#### 1. Изоляция engine на уровне job

`index.mjs` создавал `eventEmitter` на уровне модуля. Для последовательных jobs
это давало бы cross-job события и утечку listeners. Перенёс создание emitter
внутрь `nest()`. Поведение одного job не изменилось.

#### 2. Новые модули

- `src/jobs/input.mjs` — валидация + адаптер `HTTP svg → nest()` и
  `engine result → external placements`;
- `src/jobs/job.mjs` — Job (lifecycle, best result, abort, timeLimit);
- `src/jobs/job-manager.mjs` — registry + очередь + maxConcurrent;
- `src/api/jobs-router.mjs` — express router + SSE + единый JSON error format.

#### 3. Первый прогон `tests/server/jobs.test.mjs` — FAIL

Command:

```bash
node --test tests/server/jobs.test.mjs
```

Result:

- процесс завис; в TAP: `no result while running`, затем `A did not start`.
- в выводе: `Excluding poly null SVGSVGElement { children: HTMLCollection {} }`.

Cause (две проблемы):

1. `nest()` оборачивает строковый `bin` в `<svg>...</svg>`. Новый API передаёт
   уже полный SVG-document для bin → двойная обёртка → вложенный `<svg>`,
   который `flatten` не разворачивает, sheet-полигон не извлекается → `No sheet`.
2. Каскад: упавший тест не остановил job, слот `maxConcurrentJobs = 1` остался
   занят, поэтому следующий job вечно `queued` («A did not start»).

Resolution:

- `index.mjs`: если строка `bin` уже похожа на `<svg ...>`, не оборачивать
  (`/<svg[\s>]/i.test(bin) ? bin : wrap`);
- тесты теперь всегда останавливают/удаляют jobs (и `after → jobs.stopAll()`).

#### 4. Dependency cleanup

- `sse.js` — после удаления demo HTML больше нигде не импортируется (новый SSE
  endpoint пишет поток сам) → удалён из `package.json`;
- `config.json` — не referenced ни одним runtime/test/CLI path (только legacy
  sample robot-конфига) → удалён.

### Successful

* `POST /api/v1/jobs` создаёт реальный nesting job (202 + jobId).
* Lifecycle `queued/running/stopped/completed/failed` работает.
* Best result доступен во время `running`.
* SSE транслирует `job.status` (snapshot), `job.started`, `engine.progress`,
  `result.updated`, `job.stopped`, `job.completed`, `job.failed`.
* Stop корректно завершает Deepnest (abort) и освобождает слот; следующий
  queued job стартует.
* Stable IDs: `part-A`/`part-B` возвращаются как `partId`; `sheetId` = `bin.id`.
* Structured placements — основной ответ; `result.svg` сохранён.
* Legacy `POST /nest` не изменён и проходит baseline test.
* `npm test` = 10/10 PASS.

### Problems found

#### Problem: full-document bin double-wrapped

Observed: нет placements, warning `Excluding poly null SVGSVGElement`.

Cause: см. выше `nest()` оборачивал полный `<svg>` в ещё один `<svg>`.

Resolution: adapter fix in `index.mjs`. Status: fixed.

#### Problem: MaxListeners / cross-job contamination (потенциальный)

Observed: теоретически — общий module-level emitter.

Cause: `const eventEmitter = new EventTarget()` на уровне модуля.

Resolution: emitter теперь создаётся в `nest()` per job. Sequential A/B/C тест
проверяет отсутствие `MaxListenersExceededWarning` и contamination. Status: fixed.

#### Problem: `source`/`filename` semantics

Observed: engine `source` — индекс в `deepNest.parts` с офсетом листа; наружу
не отдаём.

Resolution: boundary adapter мапит `filename` → `partId`, `id` → `instanceId`,
`sheetid`/sheet → `binId`; legacy поля в `raw`. Status: handled.

### Changes made

- `index.mjs` — per-job `EventTarget`; поддержка full-svg `bin` без повторной обёртки.
- `src/jobs/input.mjs`, `src/jobs/job.mjs`, `src/jobs/job-manager.mjs` — новые.
- `src/api/jobs-router.mjs` — новый.
- `server.mjs` — `express.json`, `JobManager`, роутер `/api/v1`, error middleware,
  экспорт `jobs`.
- `package.json`/`package-lock.json` — удалён `sse.js`; удалён `config.json`.
- `README.md` — раздел HTTP API.
- `tests/fixtures/{bin,part}.svg`, `tests/server/jobs.test.mjs` — новые.
- `docs/DEVELOPMENT_LOG.md` — эта запись.

### Verification

```bash
node --test tests/server/jobs.test.mjs   # 7/7 PASS (~3.4s)
npm test                                 # 10/10 PASS (~3.6s)
```

Покрытые сценарии: create+status, result while running, stable ids, SSE,
stop, next job, queue, sequential isolation, invalid input (JSON error),
time limit → completed, delete running → 409, unknown job → 404.

### Current known-good baseline

1. `npm test` — 10/10 PASS: core nest, core native addon, legacy `/nest`,
   Job API (7).
2. `npm start` — `GET /health`, legacy `POST /nest`, `POST /api/v1/jobs`.
3. Job API: `POST/GET/GET result/GET result.svg/GET events/POST stop/DELETE`.
4. `maxConcurrentJobs = 1`; queue автоматически продвигается.
5. `placementComplete` отделён от `job.status === "completed"`.

### Debt (не трогали)

- legacy `POST /nest`, `busboy` и multipart-контракт — ещё нужны как baseline;
- CLI (`cli.mjs`, `commander`, `ora`) — отдельный этап;
- jsdom/DOM в core, canonical geometry, native ABI — по-прежнему debt;
- `sheetId` пока всегда `bin.id` (engine открывает несколько sheets при
  нехватке места; внешний `sheetId` для них ещё не различается);
- `instanceId` вычисляется адаптером по engine `id` (стабилен per job);
- best result и svg-функция удерживаются в памяти до `DELETE` job.

### Next

1. Сравнить новый API с legacy `/nest` (отчёт), затем отдельным этапом удалить
   legacy `/nest` + `busboy` и старый multipart-контракт.
2. Job Manager: вынести в `src/api`/`src/jobs` финализацию; рассмотреть
   `maxConcurrentJobs > 1` после проверки engine lifecycle.
3. Начать отделять `SVG adapter → geometry → core` (см. PROJECT_VISION).

---

## 2026-09-25 — Legacy HTTP removal / Job API becomes baseline

### Goal

Сделать `/api/v1/jobs` единственной публичной HTTP-границей: удалить legacy
`POST /nest` + multipart/busboy, предварительно подтвердив capability parity
по возможностям (не по HTTP-формату).

Cleanup commit: `ce5ebac`.

### Starting state

- `npm test` = 10/10 PASS (core, native addon, legacy `/nest`, Job API)
- Job API commit: `e47e9a4`; docs follow-up `9f411d9`

### Capability parity audit

| Capability | Legacy `/nest` | Job API `/api/v1/jobs` | Core support | Test coverage |
|---|---|---|---|---|
| engine config passthrough | `...config` | `...spec.config` в `nest()` | yes | `config passthrough` test |
| spacing | yes | yes (`config.spacing`) | yes | via config test |
| rotations | yes | yes, `rotations=1` observable | yes | `rotations=1` test |
| populationSize / mutationRate | yes | yes | yes | config test (non-default) |
| placementType / fitness | yes | yes (`gravity`/`box`/`convexhull`) | yes | config test (`box`) |
| mergeLines | yes (default true) | yes | yes | `result.svg` path |
| curveTolerance | yes | yes | yes | config test (`0.5`) |
| simplify | yes | yes | yes* | — (*known `d3` debt, не трогали) |
| timeRatio | yes | yes | yes | config default |
| progress | SSE `progress` | SSE `engine.progress` | yes | SSE test |
| progress phase | yes | yes (`phase`) | yes | SSE test |
| best/current placement | на событии | `GET /result` в любой момент | yes | create/result test |
| final/current SVG | `svg()` в SSE | `GET /result.svg` | yes | create test |
| abort | закрытие соединения | `POST /stop`, idempotent | yes | stop/delete tests |
| multiple parts | yes | yes | yes | create test (2 parts) |
| holes | engine-level | yes (SVG pass-through) | yes | `holes` test |
| quantity/copies | нет | `quantity` → instances | yes | `quantity` test (3) |
| sheet/bin handling | hardcoded object bin (override via config) | SVG bin document (`bin.id`/`bin.data`) | yes | create test (`sheetId`) |
| stable client IDs | нет | `partId`/`instanceId`/`sheetId` | adapter | stable ids test |

Вывод: Job API — надмножество полезных возможностей сервера; multipart-контракт
больше ничего не защищает.

### Tried / Changed

1. `server.mjs` сокращён до bootstrap: `express.json` + `GET /health` +
   `app.use("/api/v1", jobsRouter)` + error middleware + `start()`.
   Удалены: `busboy` import, `parseForm`, `nestSSE`, `POST /nest`,
   `node:stream/consumers`, зависимость `nest` из `index.node.mjs`.
   Globals (jsdom/DOMParser) по-прежнему поднимаются через
   `JobManager → job.mjs → index.node.mjs`.
2. `src/jobs/input.mjs`: добавлен guard на reserved-ключи config
   (`bin`, `progressCallback`, `timeout`) → `INVALID_CONFIG`.
3. Удалён `tests/server/server.test.mjs` (тестировал только `/nest`).
4. `package.json` → удалён `busboy`; `npm install` (lock обновлён).
5. Tests: добавлены parity-тесты — config passthrough (`rotations=1`),
   `quantity=3`, part с отверстием, reserved config guard; в SSE-тест
   добавлена проверка `engine.progress`.
6. `README.md`: `/api/v1` — единственный документированный API, добавлен
   полный lifecycle-пример; убрано упоминание legacy `/nest`.

### Successful

* `/nest` отсутствует; multipart-контракт отсутствует; `busboy` удалён.
* Job API покрывает все полезные возможности старого server.
* `server.mjs` содержит только bootstrap/router wiring.
* Новый HTTP E2E baseline (`tests/server/jobs.test.mjs`) проходит реальный
  путь: `POST /api/v1/jobs → nest() → worker_threads → native addon →
  progress → best result → result.svg → stop`.
* `npm test` = 13/13 PASS.

### Problems found

#### Problem: config мог утащить adapter-поля (`bin`/`timeout`/`progressCallback`)

Observed: `nest()` destructures `timeout`, `progressCallback`, `bin` поверх
spread-конфига; клиентский `config.timeout` мог тихо включить engine-timeout,
не меняя job lifecycle.

Resolution: reserved-ключи отклоняются на входе (`INVALID_CONFIG`). Status: fixed.

#### Problem: старый E2E тест исчезал вместе с `/nest`

Observed: `tests/server/server.test.mjs` проверял только legacy route.

Resolution: end-to-end покрытие перенесено в `tests/server/jobs.test.mjs`
(реальный engine, без mock). Status: resolved.

### Verification

```bash
npm test
```

Results:

* core nest: PASS
* core native addon: PASS
* Job API (`tests/server/jobs.test.mjs`): 11/11 PASS
* итого: `# tests 13 / # pass 13 / # fail 0` (~4.8s)

Сценарии Job API: create+status, result while running, stable ids, SSE
(`job.status`/`job.started`/`engine.progress`/`result.updated`), stop, next job,
queue, sequential isolation, invalid input/errors, time limit→completed,
delete running→409, config passthrough (`rotations=1`), quantity, holes,
reserved config guard.

### Current known-good baseline

1. `npm start` — `GET /health` + `/api/v1/jobs` только.
2. `npm test` — 13/13 PASS.
3. Публичная граница: `/api/v1`; внутренний `nest()` не является обещанием
   стабильного API.
4. Concurrency `maxConcurrentJobs = 1`; очередь автоматически продвигается.

### Debt (не трогали)

- CLI (`cli.mjs`, `commander`, `ora`);
- jsdom/DOM в core, SVG parser, canonical geometry, native ABI;
- `simplify:true` + `d3` (известный debt);
- `sheetId` пока всегда `bin.id`; `instanceId` вычисляется адаптером;
- best result и svg-функция в памяти до `DELETE`;
- `express`/`ora`/`fs-extra` версии не пересматривались.

### Next

1. Отдельный этап: определить границу `svgparser.js` ↔ polygon tree (какой
   минимальный объект реально является входом алгоритма) и из него вывести
   Canonical Geometry boundary.
2. Не удалять `jsdom` до появления этой границы.
3. Затем — SVG adapter как один из geometry adapters, DOM-independent core.

---

## 2026-09-25 — Geometry pipeline investigation

### Goal

Исследовать (не менять) фактическую геометрическую границу между SVG parsing и
nesting algorithm. Определить минимальный plain-JS объект — фактический вход
Deepnest. Результат: `docs/GEOMETRY_PIPELINE.md`.

Production behavior не изменялся.

Investigation commit: `fe80ee4`.

### Что исследовали

- `main/svgparser.js`, `main/deepnest.js`, `main/background.js`,
  `main/processPair.mjs`, `main/processPairs.node.mjs`,
  `main/util/{geometryutil,clipper,simplify}.js`, `main/nestingToSVG.mjs`,
  `index.mjs`, `index.node.mjs`.
- Runtime probe (`probe.tmp.mjs`, временный, удалён после исследования):
  разбор fixture с внешним контуром + hole, импорт bin, перехват
  `background-start` payload, плюс synthetic polygon tree без SVG parser.

### Неожиданные детали

- В worker payload **нет ключа `parts`**: геометрия лежит в
  `payload.individual.placement[]` (это GA seed `adam`), а metadata — в
  параллельных массивах `ids/sources/children/filenames/rotation`.
- `individual.placement[i]` — это **сам polygon tree** (не `.polygontree`).
- `placement.rotation` на этом этапе отсутствует; углы лежат в параллельном
  `individual.rotation`.
- `part.area` создаётся (`getParts`), но **нигде не читается** — dead field.
- `bounds` и `svgelements` нужны только `nestingToSVG`, в worker не уходят.
- `filename` в probe = `null`, т.к. `importsvg` вызывался с `null`; в реальном
  `index.mjs`/Job API filename = basename(id).
- `sources` в payload = `[1,2]` (bin занимает `parts[0]`) — подтверждает
  offset `source`.

### Фактический polygon format

- polygon = plain `Array<{x, y}>`; array-свойства: `source`, `children`, `id`,
  (на holes) `parent`. Array props легальны, т.к. массивы — объекты.
- После import точки имеют только `{x,y}`. `exact` добавляется позже
  (`simplifyPolygon`) и используется `mergedLength`.
- `cloneTree` в payload оставляет на root `children/id/source/filename`, точки
  `{x,y,exact}`; у вложенных holes — только точки (metadata снимается).

### Holes topology

- `toTree` строит вложенность по containment (`Clipper.PointInPolygon`), не по
  winding; рекурсия → структура поддерживает произвольную глубину.
- NFP/placement (`getOuterNfp`, `getInnerNfp`, native `calculateNFP`) используют
  только **прямые** `children` (один уровень holes). Islands глубже структурно
  сохраняются, но математикой явно не моделируются.

### Curve flattening

- Curves → points в `polygonifyPath`/`polygonify` через
  `GeometryUtil.*.linearize` с tolerance = `config.curveTolerance`.
- Ниже parser'а curves не существуют; core видит только точки.
- RDP simplify (`util/simplify.js`) применяется в `DeepNest.simplifyPolygon`;
  `config.simplify:true` уходит в `getHull` → глобальный `d3` (известный debt).

### DOM boundary

- DOM нужен до `getParts` включительно (DOMParser/SVGPathElement/pathSegList).
- После сборки payload в `deepNest.start` DOM не нужен: `cloneTree/offsetTree/
  polygonOffset/simplifyPolygon/clipper` работают с plain arrays.
- **Кандидатная граница:** объект
  `{ quantity, sheet, polygontree: cloneTree(...), filename }` прямо перед
  `worker.postMessage`.
- `svgelements` остаётся в main thread и нужен только `nestingToSVG` (renderer).

### Synthetic experiment

- Создали `new DeepNest(...)`, вручную заполнили `deepNest.parts` plain polygon
  trees (лист + деталь с hole), вызвали `start()` → payload собрался, holes
  сохранились, точки `{x,y,exact}`. **Engine запускается без SVG parser и DOM.**
- Зафиксировано тестом `tests/core/geometry.test.mjs` (3 теста), без изменения
  production API.

### Известные открытые вопросы

- depth > 1 (islands) структурно есть, NFP не использует.
- multiple sheets структурно есть, `index.mjs` импортирует один bin; `sheetId`
  для >1 sheet не проверен.
- units/scale: parser запекает `scale/localscale` в точки; canonical должен
  определить единицы.
- winding/orientation: import решает holes по containment, а clipper/native
  разворачивают по знаку площади — нужна ли winding-конвенция, не решено.

### Changes made

- `docs/GEOMETRY_PIPELINE.md` — новый (фактический flow, структуры, границы,
  candidate canonical boundary, open questions).
- `tests/core/geometry.test.mjs` — новый research/regression test (3 теста):
  polygon tree с hole, DOM-free worker payload, synthetic geometry без SVG.
- `README.md` — ссылка на GEOMETRY_PIPELINE.
- `docs/DEVELOPMENT_LOG.md` — эта запись.
- production-код не менялся.

### Verification

```bash
node --test tests/core/geometry.test.mjs   # 3/3 PASS (~0.8s)
npm test
```

Results: `npm test` полностью зелёный (core nest, native addon, geometry,
Job API). Runtime probe удалён (`probe.tmp.mjs`), в production коде debug
логирования не осталось.

### Candidate boundary (кратко)

```text
bin:   polygon tree
parts: [{ id, polygontree, quantity?, sheet? }]
```

`exact` — derived (не входит в canonical input); `id/source/rotation` — GA state.
`deepnest.start` уже выполняет проекцию в эту форму, значит SVG parser может
возвращать её без изменения алгоритма, а JSON adapter может создавать её
напрямую (доказано synthetic тестом).

### Next (рекомендуется)

1. Спроектировать `nest(geometry)`/canonical adapter на основе этой формы,
   **не меняя** алгоритм: SVG parser и новый adapter сходятся в одну точку —
   payload перед `worker.postMessage`.
2. Сначала закрыть открытые вопросы: depth>1, multiple sheets, units/winding.
3. Только после этого DOM-independent core / удаление jsdom.

---

## 2026-09-25 — Canonical Geometry boundary

### Goal

Сделать существующий plain polygon tree официальной внутренней Canonical
Geometry boundary, **не меняя** nesting algorithm. SVG становится одним из
adapters; появляется DOM-independent engine entry `nestGeometry()`.

### Выбранная canonical schema

```js
{
  units?: "mm",                                  // metadata, math unit-agnostic
  sheets: [ { id, quantity?, polygontree } ],    // `bin` — синоним одного sheet
  parts:  [ { id, quantity?, polygontree } ]
}
```

`polygontree` = массив `{x,y}` с вложенными `children` (holes).

### Почему polygonal

Curves полностью polygonized в SVG adapter (`polygonifyPath`/`polygonify`,
`curveTolerance`); ниже parser'а line/arc/bezier не существуют. Core видит
только точки. Поэтому canonical — polygonal, без line/arc/bezier schema.

### Сознательно исключено из canonical

- `exact` — engine-derived (добавляется `simplifyPolygon`, нужен `mergedLength`);
- `source`, engine `id`, `rotation`, `parent`, GA/NFP state — runtime engine;
- `svgelements`, `bounds`, `area`, DOM references — rendering/import metadata
  (остаются в adapter-owned `renderContext`);
- `filename` — adapter/application metadata (в engine передаётся как
  pass-through носитель `id`).

### Units contract

Canonical coordinates — arbitrary but consistent unit. `spacing`,
`curveTolerance`, sheet dimensions выражены в этом же unit. `units` — только
метаданные; nesting mathematics не зависит от строки `"mm"`. SVG adapter
конвертирует SVG units/scale в canonical (в `nest()` делается
`spacing * ratio * scale`); будущие CAD adapters делают это сами.

### Winding

Поведение не менялось. Import решает outer/hole по containment (`toTree`), а
clipper/native разворачивают полигоны по знаку площади
(`nfpToClipperCoordinates`). Валидация winding не добавлена; regression-тест
`outer + hole → canonical → successful nesting` есть (`holes` и canonical hole
тесты). Остаётся открытым вопросом, нужна ли winding-конвенция в canonical.

### depth > 1 limitation

Структура поддерживает arbitrary depth (`toTree` рекурсивен), NFP использует
только прямые `children`. Зафиксировано: semantics гарантированы для outer +
direct hole children; глубже — representable, но не гарантировано. Не
исправлялось.

### Sheet semantics

Canonical моделирует `sheets: [...]` (не заперт на один bin). Текущий SVG
adapter выдаёт один sheet (`sheet-0`), engine по-прежнему открывает sheets из
`deepNest.parts[].sheet`. Multiple sheet instances структурно поддержаны
engine, но не проверены; `sheetId` во внешнем API пока всегда `bin.id`.

### Что сделано

- `src/geometry/canonical.mjs` — validate/normalize/clone canonical geometry;
  `GeometryValidationError` (`INVALID_GEOMETRY`); без SVG/DOM/HTTP.
- `src/geometry/engine.mjs` — `nestGeometry(geometry, callback, options)` и
  внутренний `nestWithRender(...)`; переносит worker/event wiring из `index.mjs`;
  НЕ импортирует svgparser/nestingToSVG.
- `src/geometry/svg-adapter.mjs` — `parseSvgInput()` → `{ geometry, renderContext }`;
  svgparser/jsdom не изменялись.
- `index.mjs` — тонкий composer `SVG → canonical → nestWithRender` c
  injected `nestingToSVG`; публичное поведение и `/api/v1` не менялись.
- `main/deepnest.js` — `svgparser` теперь `require`-ится лениво (движок
  импортируется и работает без DOM globals). Алгоритм не тронут.
- `docs/GEOMETRY_PIPELINE.md` — раздел «Implemented Canonical Boundary».

### Обнаруженные проблемы

#### Problem: engine нельзя было импортировать без DOM

Observed: `require('./main/deepnest.js')` без globals → `ReferenceError: window is not defined` (svgparser eagerly грузит `pathsegpolyfill`).

Resolution: lazy `require('./svgparser')` в `deepnest.js`. Status: fixed.
`node -e "require('./main/deepnest.js')"` без globals теперь работает.

#### Problem: engine мутирует polygontree (offsetTree)

Observed: `deepnest.start()` меняет `parts[i].polygontree` in place; передача
canonical-объекта напрямую мутировала бы вход.

Resolution: `clonePolygonTree` перед передачей в engine; тест «does not mutate
caller input». Status: fixed.

#### Problem: renderer привязан к DOM, а canonical должен быть чистым

Observed: `nestingToSVG` нужны `svgelements`/`bounds` из import.

Resolution: `renderContext` отделён от canonical и передаётся только SVG-путём
через `nestWithRender`; `nestGeometry` его не знает. Status: handled.

### Verification

```bash
node --test tests/core/canonical.test.mjs   # 7/7 PASS, без DOM (проверяется typeof window === "undefined")
node --test tests/core/geometry.test.mjs    # 4/4 PASS (включая SVG→canonical parity)
npm test                                    # 24/24 PASS
```

Тесты добавлены: canonical nest без DOM; holes; quantity; `bin`-alias;
invalid canonical (empty/NaN/quantity/no parts/no sheet); clone без metadata;
SVG→canonical→engine parity.

### Production behavior

Не менялось: тот же `/api/v1`, тот же SVG request, тот же nesting algorithm,
тот же callback/progress/abort contract. Единственное внутреннее изменение —
lazy svgparser в `deepnest.js` и явная canonical-проекция.

### Commit

`2a4e1f1 refactor: introduce canonical geometry boundary and DOM-independent nestGeometry`.

### Next

1. Public JSON geometry adapter (`format: "geometry"`) поверх проверенной
   canonical boundary — отдельный этап.
2. Закрыть открытые вопросы: winding convention, depth>1 policy, multiple
   sheets (`sheetId`), ownership of units in future adapters.
3. `jsdom` не удалять: SVG adapter и renderer пока на нём (и это допустимо).

---

## 2026-09-25 — Public `format: "geometry"` Job input

### Goal

Добавить публичный JSON geometry input в существующий `/api/v1/jobs`, используя
уже реализованную Canonical Geometry boundary. Не менять engine math, workers,
NFP, native addon, SVG parser/renderer.

### Архитектурная проверка

Границы выбраны верно: `Job`/`JobManager`/engine **не знают** про формат.
Формат-специфичное решение сосредоточено в `src/jobs/input.mjs` (selection) и
`src/geometry/json-adapter.mjs` (DTO→canonical). Job просто вызывает
`adaptInput(spec)` → `nestWithRender(geometry, renderContext, ...)`.

```text
POST /api/v1/jobs
   ↓ validateRequest(body)              (src/jobs/input.mjs)
   ↓ adaptInput(spec)
   ├─ format "svg"      → svg-adapter  → geometry + renderContext(nestingToSVG)
   └─ format "geometry" → json-adapter → geometry (renderContext = null)
   ↓
nestWithRender()/nestGeometry()          (existing engine, unchanged)
```

### Public JSON DTO

```text
PolygonDTO { points: PointDTO[], children?: PolygonDTO[] }
PointDTO   { x: number, y: number }
```

DTO ≠ internal canonical: canonical polygon — JS Array с array-property
`children` (JSON так не умеет). Adapter конвертирует DTO → canonical tree.

### Что сделано

- `src/geometry/json-adapter.mjs` — `parseGeometryInput()`: DTO → canonical;
  `GeometryValidationError` (`INVALID_GEOMETRY`) на bad points/<3/NaN/children.
- `src/jobs/input.mjs` — `validateRequest()` теперь dispatch по
  `input.format`; `adaptInput(spec)` возвращает
  `{ geometry, renderContext, engineOptions }`; единые config/execution guards;
  engine-options учитывают разное значение spacing (SVG: `spacing*ratio*scale`,
  geometry: canonical as-is). `nestingToSVG` грузится dynamic import-ом только
  на SVG-ветке.
- `src/jobs/job.mjs` — больше не импортирует `index.node.mjs`; вызывает
  `adaptInput` + `nestWithRender`; хранит `spec.sheetId`, `svgAvailable`.
- `server.mjs` — side-effect `import "./index.node.mjs"` (Node bootstrap DOM для
  SVG-ветки); теперь именно server выбирает DOM-адаптер.
- `src/api/jobs-router.mjs` — `GET result.svg`: при `format:"geometry"` возвращает
  `400 RESULT_FORMAT_UNAVAILABLE`; `GET result` работает как раньше.
- `README.md`, `docs/GEOMETRY_PIPELINE.md` — оба input format + distinction
  Public DTO vs Internal Canonical.

### Schema / правила

- `input.format` = `"svg"` | `"geometry"`.
- geometry: `input.sheets` (только один sheet сейчас), `input.parts`,
  `input.units?` (metadata); `config.spacing` — в canonical units.
- Stable IDs: `part.id`/`sheet.id` проходят в `partId`/`sheetId` без изменений.
- Quantity поддерживается canonical semantics.
- Holes — recursive `children`.
- Validation split: структура запроса → `INVALID_REQUEST`;
  геометрия (points/<3/NaN/quantity/children/id) → `INVALID_GEOMETRY`.
- Multiple sheets: representable, но HTTP пока требует ровно один
  (`INVALID_GEOMETRY`) — чтобы не возвращать неверную identity.
- Winding: без auto-correction, текущие semantics; гарантия — outer + direct
  holes. Depth>1 representable, но не гарантируется.

### Problems found / решения

#### Problem: JSON не выражает array-property `polygon.children`

Resolution: отдельный transport DTO `{points, children}` + json-adapter.
Status: fixed.

#### Problem: DOM ставился неявно через job.mjs (`index.node.mjs`)

Observed: после переноса Job на adapters `svgparser`/renderer могли грузиться
без globals.

Resolution: `nestingToSVG` грузится dynamic import-ом только для SVG; DOM-
bootstrap теперь явно в `server.mjs` (`import "./index.node.mjs"`). Geometry-ветка
DOM не трогает. Status: fixed.

#### Problem: geometry job не должен отдавать сырой 409 на result.svg

Resolution: `RESULT_FORMAT_UNAVAILABLE` (400) при наличии result, но отсутствии
SVG renderer. Status: fixed.

### Verification

```bash
npm test   # 27/27 PASS
```

Новые HTTP integration tests (`tests/server/jobs.test.mjs`):
- geometry create + stable ids (`partId`/`sheetId`) + quantity + holes + result
  while running;
- geometry SSE (`job.started`/`engine.progress`/`result.updated`);
- geometry `result.svg` → `RESULT_FORMAT_UNAVAILABLE`;
- invalid geometry/structure: empty points, NaN, quantity 0, missing id,
  malformed children, no sheets/parts, multiple sheets.

Регрессия: все прежние 24 теста зелёные (SVG public API не менялся).

### Commit

Фиксируется отдельным commit этого этапа (hash добавляется следом).

### Далее

Не выбирать следующий шаг автоматически. Оценить реальные ограничения, которые
проявились после второго независимого input adapter: winding, multiple sheets,
topology depth, result rendering (geometry→SVG), CLI, native ABI. По фактической
боли выбрать следующий этап.
