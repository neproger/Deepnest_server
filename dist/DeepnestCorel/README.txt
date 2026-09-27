Deepnest для CorelDRAW — установка
=================================

Всё внутри аддона: сервер (Node) уже вшит, отдельно ничего ставить не нужно.

1. Загрузить аддон в CorelDRAW:
   - открой докер «Скрипты» (Scripts), выбери «Visual Studio Tools for
     Applications»;
   - нажми «Загрузить» (Load) и выбери файл CorelDeepnest.CGSaddon;
   - разверни «CorelDeepnest → Main» и запусти команду.

2. Команды:
   - «TestDeepnestConnection» — проверка связи с сервером;
   - «NestSelectedShapes» — разложить выделенные объекты.

При первом запуске аддон сам распакует и запустит сервер
(http://127.0.0.1:8080). Ничего указывать не нужно.

Требования: CorelDRAW 2025 с компонентом VSTA (Visual Studio Tools for
Applications). .NET Framework 4.8 уже есть в Windows.