# Deepnest Server — Project Vision

## 1. Что мы строим

Проект должен превратиться из форка desktop Deepnest в самостоятельный headless Node.js сервис для 2D nesting.

Основной продукт этого репозитория:

```text
Deepnest HTTP Server
```

Он запускается локально или на сервере и предоставляет API для расчета раскладки двумерных деталей.

Сервис не должен быть привязан к конкретному CAD, UI или клиентскому приложению.

Клиентами в будущем могут быть:

```text
CorelDRAW plugin
SketchUp plugin
Web UI
CLI
другие CAD/CAM системы
автоматизированные производственные сервисы
```

Все они должны использовать один и тот же HTTP API.

Пример архитектуры:

```text
CorelDRAW ─────┐
SketchUp ──────┤
Web UI ────────┼── HTTP API ──> Deepnest Server
CLI ───────────┤
Other clients ─┘
```

Интеграции с CorelDRAW, SketchUp и другими программами являются отдельными клиентскими проектами.

Сам Deepnest Server ничего не должен знать о CorelDRAW Shape, SketchUp Entity, COM, Ruby API и т.п.

---

# 2. Главная архитектурная идея

Проект должен иметь несколько четко разделенных слоев:

```text
Transport
   ↓
HTTP API

Application
   ↓
Job Manager

Geometry Input
   ↓
SVG / JSON / другие adapters

Canonical Geometry
   ↓
Nesting Core

Execution
   ↓
Workers / NFP / Clipper / Native Minkowski
```

Связи должны идти сверху вниз.

Вычислительное ядро не должно зависеть от HTTP.

HTTP слой не должен содержать nesting logic.

Форматы импорта не должны определять внутреннюю модель ядра.

---

# 3. HTTP Server

Ближайшая практическая цель проекта — полноценный HTTP API поверх существующего работающего Node.js Deepnest.

Nesting является длительной итерационной операцией, поэтому основной abstraction API:

```text
Job
```

а не один блокирующий HTTP request.

Ожидаемая модель:

```text
POST   /api/v1/jobs
GET    /api/v1/jobs/:id
GET    /api/v1/jobs/:id/result
GET    /api/v1/jobs/:id/events
POST   /api/v1/jobs/:id/stop
DELETE /api/v1/jobs/:id
```

Точные URI могут измениться, но семантика должна сохраниться.

Job должен поддерживать:

```text
queued
running
completed
stopped
failed
```

Во время расчета клиент должен иметь возможность:

* получать progress;
* получать лучший найденный placement;
* наблюдать улучшение результата;
* остановить расчет;
* получить текущий или финальный результат.

Для streaming событий предполагается SSE, если не появится веская причина использовать другой транспорт.

---

# 4. Deepnest остается вычислительным ядром

На данном этапе не требуется переписывать алгоритм Deepnest.

Нужно сохранить работающие:

* Genetic Algorithm;
* NFP;
* Minkowski operations;
* Clipper;
* spacing;
* rotations;
* multiple sheets;
* holes;
* mergeLines;
* fitness calculation;
* workers;
* native addon;
* остальные реально работающие возможности существующего движка.

Рефакторинг должен сначала отделять инфраструктуру от алгоритмов, а не менять математическое поведение.

Все существенные изменения алгоритма должны происходить только отдельно и иметь regression tests.

---

# 5. SVG сейчас является главным входным форматом

В первой версии HTTP API основным форматом геометрии остается SVG.

Причины:

* существующий Deepnest уже надежно работает с SVG;
* SVG хорошо представляет 2D геометрию;
* формат простой для генерации;
* легко передается через HTTP;
* поддерживает paths, curves и holes;
* значительно проще многих CAD exchange formats.

На данном этапе не нужно отказываться от SVG.

Однако SVG не должен навсегда оставаться внутренней моделью nesting core.

---

# 6. Долгосрочная цель — отвязать ядро от SVG и DOM

Сейчас цепочка примерно:

```text
SVG
 ↓
jsdom / DOMParser
 ↓
SVG parser
 ↓
Deepnest polygons
 ↓
nesting
```

Целевая архитектура:

```text
SVG ──────────┐
JSON geometry ┤
DXF adapter ──┤
Corel adapter ┼──> Canonical Geometry ──> Nesting Core
SketchUp ─────┘
```

То есть SVG должен стать адаптером:

```text
SVG → Canonical Geometry
```

а не обязательным форматом ядра.

В будущем должно существовать программное API примерно такого уровня:

```js
const geometry = parseSvg(svg);

const job = nest(geometry, config);
```

где `nest()` не знает, откуда geometry появилась.

---

# 7. Canonical Geometry

Проекту в будущем понадобится собственная простая внутренняя геометрическая модель.

Она не должна копировать SVG DOM или DXF.

Пример концепции:

```js
{
  id: "part-A",

  polygons: [
    {
      points: [
        { x: 0, y: 0 },
        { x: 100, y: 0 },
        { x: 100, y: 50 },
        { x: 0, y: 50 }
      ],

      children: [
        {
          points: [...],
          children: []
        }
      ]
    }
  ]
}
```

Это только концептуальный пример.

Не фиксировать сейчас окончательную schema без анализа внутреннего polygon tree Deepnest.

Важно сохранить:

* внешние контуры;
* отверстия;
* вложенность;
* координаты;
* идентичность детали;
* необходимые metadata.

---

# 8. Primitive geometry и CAD entities

В будущем внешние клиенты могут захотеть передавать геометрию не через SVG, а непосредственно:

```text
line
arc
circle
bezier
polyline
```

Например:

```js
{
  type: "line",
  from: { x: 0, y: 0 },
  to: { x: 100, y: 0 }
}
```

Но такие primitives не должны становиться основной моделью nesting core.

Между primitive entities и nesting должно существовать преобразование:

```text
entities
 ↓
chain building
 ↓
closed contours
 ↓
topology / holes
 ↓
curve flattening
 ↓
polygon tree
 ↓
nesting
```

Nesting core должен получать уже подготовленные области/полигоны.

---

# 9. DXF

DXF может быть добавлен позже как importer.

DXF не рассматривается как основной внутренний контракт системы.

Причина: DXF — большой CAD exchange format, содержащий множество сущностей и возможностей, не относящихся напрямую к nesting:

* layers;
* blocks;
* inserts;
* splines;
* units;
* transformations;
* огромное количество entity types.

Если поддержка DXF понадобится:

```text
DXF → DXF adapter → Canonical Geometry
```

Никакая DXF-specific логика не должна попадать в nesting core.

---

# 10. Stable identifiers

Любой клиент должен иметь возможность передать собственный стабильный ID.

Например:

```text
partId
sheetId
```

После nesting эти же идентификаторы должны присутствовать в placements.

Пример:

```js
{
  partId: "door-left",
  sheetId: "sheet-2",
  x: 120.5,
  y: 80.3,
  rotation: 90
}
```

Внутренние Deepnest поля:

```text
source
id
filename
sheetid
```

могут существовать внутри движка или debug output, но внешние клиенты API не должны зависеть от внутренних индексных соглашений Deepnest.

---

# 11. Result model

Главный результат nesting — структурированные placement data.

Не итоговый SVG.

SVG result является полезным дополнительным представлением результата, но не основным контрактом.

Основная информация:

```text
partId
instanceId
sheetId
x
y
rotation
fitness
other relevant metrics
```

Именно placements будут использовать CAD-клиенты для перемещения своих исходных объектов.

При этом API может дополнительно предоставлять:

```text
result.svg
preview
mergedLines
statistics
raw Deepnest result
```

если это полезно.

---

# 12. Job Manager

HTTP слой не должен напрямую управлять Deepnest worker lifecycle.

Для этого нужен Job Manager.

Ответственность Job Manager:

```text
create job
queue job
start job
track status
track progress
store best result
stop job
cleanup job
handle failure
```

Первоначально допустимо:

```text
maxConcurrentJobs = 1
```

если существующий Deepnest пока не гарантирует безопасную параллельную работу.

Другие jobs в таком случае должны переходить в:

```text
queued
```

Архитектура API не должна ограничивать будущую поддержку нескольких параллельных jobs.

---

# 13. Локальный сервер как основной сценарий

Первоначальный сценарий:

```text
user starts Deepnest Server
user starts CAD/application
application calls localhost API
```

По умолчанию сервер должен слушать только:

```text
127.0.0.1
```

Например:

```text
http://127.0.0.1:8080
```

Host и port должны быть конфигурируемыми.

Удаленный/cloud deployment возможен в будущем, но сейчас он не является основной задачей.

---

# 14. Что не является задачей Deepnest Server

В этом репозитории не нужно сейчас реализовывать:

```text
CorelDRAW UI
CorelDRAW COM integration
SketchUp plugin
Web frontend
CAM toolpaths
G-code
DXF editor
CAD modeling
authentication
cloud infrastructure
database
user accounts
licensing server
```

Некоторые из этих компонентов могут появиться отдельными проектами или отдельными packages позднее.

Но они не должны проникать в nesting core.

---

# 15. Основной принцип развития

При архитектурных решениях использовать следующий вопрос:

> Может ли тот же Deepnest Server одинаково обслуживать CorelDRAW, SketchUp, CLI и Web UI?

Если решение требует знаний о конкретном клиенте внутри nesting engine или Job Manager — скорее всего граница выбрана неправильно.

Другой важный вопрос:

> Можно ли заменить SVG adapter на другой geometry adapter, не меняя nesting core?

В долгосрочной архитектуре ответ должен быть: да.

---

# 16. Текущий этап

На сегодняшний момент подтверждено:

* проект запускается headless на Node.js;
* Electron не требуется для выполнения nesting;
* native Minkowski addon собирается и работает;
* реальный nesting job проходит;
* worker_threads работают;
* progress доступен;
* abort работает;
* CLI способен получить SVG и JSON result;
* имеются smoke/regression tests ядра.

Текущая ближайшая задача:

```text
построить стабильный HTTP Job API поверх существующего работающего engine
```

Не заниматься преждевременным переписыванием geometry core.

После появления стабильного HTTP API и regression tests можно постепенно отделять SVG adapter и jsdom от nesting core.

---

# 17. Предпочтительный порядок развития

```text
1. Working Node Deepnest Core            DONE
2. Regression tests                      DONE / expanding
3. HTTP Job API                          NEXT
4. Stable external IDs
5. Job queue/lifecycle
6. SSE progress/results
7. Document full config/capabilities
8. Remove remaining Electron/UI legacy
9. Stabilize public HTTP contract
10. Extract Canonical Geometry boundary
11. Move SVG parsing into SVG adapter
12. Remove DOM dependency from nesting core
13. Native JSON geometry API
14. Additional adapters when actually needed
15. CAD/Web clients as independent projects
```

Не менять этот порядок без технической причины.

---

# 18. Главная цель проекта

Итог проекта должен выглядеть так:

```text
           Deepnest Server
                 │
        ┌────────┴────────┐
        │                 │
     HTTP API        Programmatic API
        │                 │
        └───────┬─────────┘
                │
           Job Manager
                │
         Geometry Boundary
                │
        Canonical Geometry
                │
          Nesting Engine
                │
      Workers / Native NFP
```

А поверх него независимо могут существовать:

```text
CorelDRAW Nesting Plugin
SketchUp Nesting Plugin
Web Nesting Application
CLI
Batch processing tools
production automation
```

Deepnest Server должен оставаться самостоятельным универсальным 2D nesting engine/service.

---

# Known Architectural Debt

Ниже перечислены конкретные расхождения текущей реализации с целевым видением. Это не список задач на «сейчас», а известные границы, которые нельзя путать с уже достигнутым результатом.

1. **Нет HTTP Job API.** Существует только демонстрационный `server.mjs` (Express + HTML-страницы + SSE + `/text` + загрузка шрифтов через `api.fontsource.org`). Требуемых `POST /api/v1/jobs` и lifecycle job нет. `server.mjs` не является целевым API.

2. **Нет Job Manager.** Управление жизненным циклом расчёта отсутствует. `nest()` в `index.mjs` — низкоуровневый вызов, а не job.

3. **Модульный `eventEmitter` в `index.mjs`.** `const eventEmitter = new EventTarget()` создаётся на уровне модуля и переиспользуется всеми вызовами `nest()`. Параллельные job'ы будут перекрёстно получать события `background-*` / `placement`. Поэтому целевое `maxConcurrentJobs = 1` — вынужденная мера, пока emitter не вынесен внутрь `nest()`.

4. **Внутренние идентификаторы утекают наружу.** `source` — это индекс в `DeepNest.parts`, причём лист импортируется первым и занимает `parts[0]`, поэтому пользовательские детали начинаются с `source = 1`. `id` — сквозной индекс экземпляра `0..n-1`. `filename` = `null` для сырых SVG-строк. Внешний стабильный `partId`/`sheetId` не поддерживается.

5. **Группировка по листам теряется в `result`.** Callback `index.mjs` отдаёт плоский `result` (только `sheetplacements`); `sheet`/`sheetid` доступны лишь в `data.placements[j]`. Это неудобно как публичный контракт.

6. **DOM/jsdom внутри ядра.** `main/svgparser.js` использует глобальные `DOMParser`, `window`, `document`, `SVGPathSegList`; `index.node.mjs` поднимает их через jsdom. `nestingToSVG.mjs` также зависит от DOM. Ядро пока не отделено от SVG/DOM, что противоречит целевому Canonical Geometry boundary.

7. **CLI не завершается самостоятельно после `timeout`.** `abort()` останавливает worker, но `ora`-спиннер удерживает event loop. Программный `abort()` ядра работает; проблема — в презентационном слое CLI.

8. **Пакет всё ещё Electron-центричен.** `package.json` содержит `main: main.js`, скрипты `start`/`build`/`dist`/`exec`, зависимости `electron`, `@electron/packager`, `@electron/rebuild`, `@electron/remote`, а также UI/demo-зависимости (`express`, `busboy`, `opentype.js`, `sse.js`, `axios`, `graceful-fs`). Скрипт `build` делает `electron-rebuild` и ломает Node ABI native addon; для headless допустим только обычный `npm install`.

9. **Native addon привязан к ABI.** `main/background.js` грузит addon через `bindings('addon.node')` из `build/Release`. Смена ABI (electron-rebuild) ломает Node-путь. Нужна предсказуемая стратегия сборки под Node.

10. **`index.d.ts` не является стабильным публичным контрактом.** Он описывает текущий внутренний `nest(svgInput, callback, options)` и внутренний result shape, а не целевой Job/placement API.
