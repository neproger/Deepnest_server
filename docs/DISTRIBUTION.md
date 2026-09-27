# Сборка дистрибутива DeepnestCorel

Как собрать папку `dist\DeepnestCorel` для конечного пользователя и как это
устроено внутри.

## Что получается

```text
dist\DeepnestCorel\
  CorelDeepnest.CGSaddon   VSTA-аддон CorelDRAW; внутрь вшиты
                           CorelDeepnest.Runtime.dll, Contracts.dll и server.zip
  BUILD.md                 этот документ
  README.txt               инструкция для конечного пользователя
```

Папка самодостаточна: конечному пользователю не нужно ставить Node, Python или
что-то ещё. Достаточно загрузить `.CGSaddon` в CorelDRAW.

## Требования к машине сборки

- **Node 20+** (используется и для сборки, и для бандла): `node --version`.
- **Python + Visual Studio C++ Build Tools** — для сборки native-аддона
  Deepnest (`build\Release\addon.node`) через `npm install` (node-gyp).
- **.NET Framework 4.8 MSBuild** — для сборки Corel-аддона
  (`%WINDIR%\Microsoft.NET\Framework64\v4.0.30319\MSBuild.exe`, входит в Windows).
- CorelDRAW не нужен для сборки (только Corel.Interop для загрузки аддона).

## Команда сборки

Из корня репозитория:

```powershell
npm install                # соберёт native-аддон build\Release\addon.node
.\build-distribution.ps1   # соберёт dist\DeepnestCorel
```

Скрипт по шагам:

1. готовит приложение сервера во временный каталог `dist\.build-server\server`
   (`server.mjs`, `index*.mjs`, `main\`, `src\{api,geometry,jobs}`, `build\Release\addon.node`);
2. кладёт туда `node.exe` (из текущего рантайма — ABI совпадает с addon.node)
   и ставит продовые зависимости (`bindings`, `express`, `jsdom`) через
   `npm install --omit=dev`;
3. упаковывает каталог в `server.zip`;
4. вызывает `corel_addon\build-addon.ps1 -ServerZip server.zip`, который вшивает
   `server.zip` в `.CGSaddon` (EmbeddedResource + запись в UTF-16 манифесте);
5. собирает `dist\DeepnestCorel` (аддон + `README.txt` + этот `BUILD.md`);
6. удаляет временные артефакты.

## Как это работает в рантайме

- `VstaLoader.cs` (загрузчик, `Macro.cs` в пакете) при первом запуске команды
  распаковывает `server.zip` в `%LOCALAPPDATA%\CorelDeepnest\Server\<hash>` и
  пишет `current.txt`. Хэш SHA-256 содержимого = версия, поэтому повторный
  запуск ничего не распаковывает.
- `VstaMacro.cs` (`CorelDeepnest.Runtime.dll`) проверяет
  `http://127.0.0.1:8080/health`; если сервер не поднят — запускает
  `node.exe server.mjs` из распакованной папки и ждёт health.
- Выбор папки вручную остался только как fallback для dev-сборок без `ServerZip`.

## Проверка в CorelDRAW

1. Скрипты → Visual Studio Tools for Applications → **Load** →
   `dist\DeepnestCorel\CorelDeepnest.CGSaddon`.
2. `CorelDeepnest → Main → TestDeepnestConnection` — должен быть OK.
3. `NestSelectedShapes` — раскладка выделения.

Изменения `VstaMacro.cs`/Runtime подхватываются hot-reload (перезагрузить форму).
Изменения `VstaLoader.cs` (загрузчик) требуют **перезапуска CorelDRAW** и
повторной загрузки пакета.

## Обновление

Изменил сервер/аддон → пересобери `.\build-distribution.ps1` и закоммить
обновлённый `dist\DeepnestCorel` (он отслеживается в git, чтобы не пересобирать
каждый раз).

## Переменные окружения (на машине сборки)

- `node` должен быть в `PATH` (берётся `(Get-Command node).Source`).
- Иные пути не требуются; Corel установка не читается.
