<#
.SYNOPSIS
Build the end-user CorelDRAW distribution folder.

Produces dist\DeepnestCorel\ containing:
  CorelDeepnest.CGSaddon   the VSTA addon, with the whole server embedded
                           (Runtime + Contracts + server.zip)
  README.txt               install instructions

The addon extracts the embedded server to
%LOCALAPPDATA%\CorelDeepnest\Server on first run and starts node.exe from there,
so the end user only loads the .CGSaddon in CorelDRAW: no Node install, no folder
to choose, no manual server start.
#>
[CmdletBinding()]
param(
    [string]$Output = "dist\DeepnestCorel"
)

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$outputDir = Join-Path $root $Output
$stageDir = Join-Path $root "dist\.build-server"
$serverDir = Join-Path $stageDir "server"
$serverZip = Join-Path $root "dist\.server.zip"

Write-Host "1/6 Stage the server application"
if (Test-Path -LiteralPath $stageDir) { Remove-Item -LiteralPath $stageDir -Recurse -Force }
New-Item -ItemType Directory -Path $serverDir -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $root "server.mjs") $serverDir
Copy-Item -LiteralPath (Join-Path $root "index.mjs") $serverDir
Copy-Item -LiteralPath (Join-Path $root "index.node.mjs") $serverDir
Copy-Item -LiteralPath (Join-Path $root "main") $serverDir -Recurse
New-Item -ItemType Directory -Path (Join-Path $serverDir "src") -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $root "src\api") (Join-Path $serverDir "src\api") -Recurse
Copy-Item -LiteralPath (Join-Path $root "src\geometry") (Join-Path $serverDir "src\geometry") -Recurse
Copy-Item -LiteralPath (Join-Path $root "src\jobs") (Join-Path $serverDir "src\jobs") -Recurse
New-Item -ItemType Directory -Path (Join-Path $serverDir "build\Release") -Force | Out-Null
# Bundled nesting engine (OpenNest). It must be built first (npm install).
$opennestAddon = Join-Path $root "build\Release\opennest.node"
if (-not (Test-Path -LiteralPath $opennestAddon)) {
    throw "opennest.node is missing. Run `npm install` (or `npm run build:native`) first."
}
Copy-Item -LiteralPath $opennestAddon (Join-Path $serverDir "build\Release")

Write-Host "2/6 Bundle the Node runtime and production dependencies"
$nodeExe = (Get-Command node).Source
Copy-Item -LiteralPath $nodeExe (Join-Path $serverDir "node.exe")

$serverPackage = @'
{
  "name": "deepnest-server",
  "private": true,
  "version": "1.0.0",
  "description": "Bundled Deepnest nesting server (OpenNest engine).",
  "dependencies": {
    "bindings": "^1.5.0",
    "express": "^4.21.2",
    "jsdom": "^25.0.1"
  }
}
'@
[System.IO.File]::WriteAllText((Join-Path $serverDir "package.json"), $serverPackage)
Push-Location $serverDir
try {
    # npm writes warnings to stderr, which PowerShell would treat as fatal under
    # $ErrorActionPreference = "Stop"; relax it and check the exit code instead.
    $ErrorActionPreference = "Continue"
    & npm install --omit=dev --no-audit --no-fund 2>&1 | Out-Host
    $npmExit = $LASTEXITCODE
    $ErrorActionPreference = "Stop"
    if ($npmExit -ne 0) { throw "npm install failed with exit code $npmExit." }
}
finally {
    Pop-Location
}

Write-Host "3/6 Zip the server"
if (Test-Path -LiteralPath $serverZip) { Remove-Item -LiteralPath $serverZip -Force }
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory(
    $serverDir, $serverZip,
    [System.IO.Compression.CompressionLevel]::Optimal, $false)

Write-Host "4/6 Build the addon package with the embedded server"
& (Join-Path $root "corel_addon\build-addon.ps1") -ServerZip $serverZip
if ($LASTEXITCODE -ne 0) { throw "build-addon.ps1 failed." }

Write-Host "5/6 Assemble the distribution folder"
if (Test-Path -LiteralPath $outputDir) { Remove-Item -LiteralPath $outputDir -Recurse -Force }
New-Item -ItemType Directory -Path $outputDir -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $root "corel_addon\dist\CorelDeepnest.CGSaddon") $outputDir

Write-Host "6/6 Write README and clean up"
$readme = @'
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

Сервер запускается сам при открытии окна раскладки и останавливается при его
закрытии; отдельно указывать или запускать ничего не нужно
(http://127.0.0.1:8080).

Требования: CorelDRAW 2025 с компонентом VSTA (Visual Studio Tools for
Applications). .NET Framework 4.8 уже есть в Windows.
'@
[System.IO.File]::WriteAllText((Join-Path $outputDir "README.txt"), $readme)
Copy-Item -LiteralPath (Join-Path $root "docs\DISTRIBUTION.md") (Join-Path $outputDir "BUILD.md")

Remove-Item -LiteralPath $stageDir -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $serverZip -Force -ErrorAction SilentlyContinue

$sizeMb = [math]::Round(((Get-ChildItem $outputDir -Recurse -File | Measure-Object Length -Sum).Sum / 1MB), 1)
Write-Host "Done: $outputDir ($sizeMb MB)"
