[CmdletBinding()]
param(
    [string]$CorelProject,
    [switch]$SkipCacheCleanup
)

$ErrorActionPreference = "Stop"
$projectDirectory = $PSScriptRoot
$macroSource = Join-Path $projectDirectory "VstaLoader.cs"
$runtimeProject = Join-Path $projectDirectory "Runtime\CorelDeepnest.Runtime.csproj"
$distDirectory = Join-Path $projectDirectory "dist"
$addonFile = Join-Path $distDirectory "CorelDeepnest.CGSaddon"

$corelProcesses = @(Get-Process -Name "CorelDRW" -ErrorAction SilentlyContinue)
if ($corelProcesses.Count -gt 0) {
    $processIds = ($corelProcesses | ForEach-Object { $_.Id }) -join ", "
    Write-Host "CorelDRAW is running (PID: $processIds). Publishing a hot-reload runtime; the VSTA loader package will also be refreshed for the next restart."
}

function Clear-CorelDeepnestCache {
    $removed = 0
    $roamingCorel = Join-Path $env:APPDATA "Corel"
    if (Test-Path -LiteralPath $roamingCorel) {
        Get-ChildItem -LiteralPath $roamingCorel -Filter "CorelDeepnest.CGSaddon" `
            -File -Recurse -Force -ErrorAction SilentlyContinue | ForEach-Object {
            try {
                Remove-Item -LiteralPath $_.FullName -Force -ErrorAction Stop
                Write-Host "Removed cached addon: $($_.FullName)"
                $removed++
            }
            catch {
                Write-Warning "Could not remove cached addon '$($_.FullName)'. Unload it or close CorelDRAW. $($_.Exception.Message)"
            }
        }
    }

    $vstaRoot = Join-Path $env:LOCALAPPDATA "Temp\Vsta"
    if (Test-Path -LiteralPath $vstaRoot) {
        $resolvedRoot = [System.IO.Path]::GetFullPath($vstaRoot).TrimEnd('\') + '\'
        Get-ChildItem -LiteralPath $vstaRoot -Directory -Force -ErrorAction SilentlyContinue |
            ForEach-Object {
                $cacheDirectory = $_
                $containsProject = Get-ChildItem -LiteralPath $cacheDirectory.FullName `
                    -Filter "CorelDeepnest.csproj" -File -Recurse -Force `
                    -ErrorAction SilentlyContinue | Select-Object -First 1
                if (-not $containsProject) {
                    return
                }

                $resolvedTarget = [System.IO.Path]::GetFullPath($cacheDirectory.FullName)
                if (-not $resolvedTarget.StartsWith(
                    $resolvedRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
                    throw "Refusing to remove a VSTA cache outside '$vstaRoot': $resolvedTarget"
                }

                try {
                    Remove-Item -LiteralPath $resolvedTarget -Recurse -Force -ErrorAction Stop
                    Write-Host "Removed VSTA build cache: $resolvedTarget"
                    $removed++
                }
                catch {
                    Write-Warning "Could not remove VSTA cache '$resolvedTarget'. Unload the project or close CorelDRAW. $($_.Exception.Message)"
                }
            }
    }

    if ($removed -eq 0) {
        Write-Host "No cached CorelDeepnest addon or VSTA project was found."
    }
}

if (-not $SkipCacheCleanup -and $corelProcesses.Count -eq 0) {
    Clear-CorelDeepnestCache
}
elseif (-not $SkipCacheCleanup) {
    Write-Host "Skipping VSTA loader cache cleanup while CorelDRAW is running."
}

if ([string]::IsNullOrWhiteSpace($CorelProject)) {
    $CorelProject = Join-Path $projectDirectory "VstaTemplate\CorelDeepnest.CGSaddon"
}

if (-not (Test-Path -LiteralPath $CorelProject)) {
    throw "Corel-created VSTA project was not found: $CorelProject"
}

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

New-Item -ItemType Directory -Path $distDirectory -Force | Out-Null

$msbuild = Join-Path $env:WINDIR "Microsoft.NET\Framework64\v4.0.30319\MSBuild.exe"
if (-not (Test-Path -LiteralPath $msbuild)) {
    throw "MSBuild for .NET Framework was not found: $msbuild"
}

$buildId = [DateTime]::UtcNow.ToString("yyyyMMddHHmmssfff")
$buildLabel = [DateTime]::Now.ToString("yyyyMMdd-HHmmss")
$generatedBuildInfo = Join-Path $projectDirectory "Runtime\GeneratedBuildInfo.cs"
$generatedBuildInfoText = @"
namespace CorelDeepnest.Runtime
{
    internal static class BuildInfo
    {
        internal const string Id = "$buildLabel";
    }
}
"@
[System.IO.File]::WriteAllText(
    $generatedBuildInfo,
    $generatedBuildInfoText,
    [System.Text.UTF8Encoding]::new($false))

& $msbuild $runtimeProject /t:Rebuild /p:Configuration=Release /v:minimal /nologo
if ($LASTEXITCODE -ne 0) {
    throw "CorelDeepnest runtime build failed with exit code $LASTEXITCODE."
}

$runtimeOutput = Join-Path $projectDirectory "Runtime\bin\Release\CorelDeepnest.Runtime.dll"
if (-not (Test-Path -LiteralPath $runtimeOutput)) {
    throw "The runtime build did not produce: $runtimeOutput"
}
$contractsOutput = Join-Path $projectDirectory "Contracts\bin\Release\CorelDeepnest.Contracts.dll"
if (-not (Test-Path -LiteralPath $contractsOutput)) {
    throw "The contracts build did not produce: $contractsOutput"
}

$runtimeInstallDirectory = Join-Path $env:LOCALAPPDATA "CorelDeepnest\Runtime"
New-Item -ItemType Directory -Path $runtimeInstallDirectory -Force | Out-Null
$runtimeVersionDirectory = Join-Path $runtimeInstallDirectory $buildId
New-Item -ItemType Directory -Path $runtimeVersionDirectory -Force | Out-Null
$installedRuntime = Join-Path $runtimeVersionDirectory "CorelDeepnest.Runtime.dll"
Copy-Item -LiteralPath $runtimeOutput -Destination $installedRuntime -Force
Copy-Item -LiteralPath $contractsOutput `
    -Destination (Join-Path $runtimeVersionDirectory "CorelDeepnest.Contracts.dll") -Force

$pointerPath = Join-Path $runtimeInstallDirectory "current.txt"
$pointerTemporaryPath = Join-Path $runtimeInstallDirectory "current.tmp"
[System.IO.File]::WriteAllText(
    $pointerTemporaryPath,
    $buildId,
    [System.Text.UTF8Encoding]::new($false))
Move-Item -LiteralPath $pointerTemporaryPath -Destination $pointerPath -Force

Get-ChildItem -LiteralPath $runtimeInstallDirectory -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -ne $buildId -and $_.Name -ne "bundled" } |
    ForEach-Object {
        $oldRuntimePath = $_.FullName
        try {
            $resolvedRuntimeRoot = [IO.Path]::GetFullPath($runtimeInstallDirectory).TrimEnd('\') + '\'
            $resolvedOldVersion = [IO.Path]::GetFullPath($oldRuntimePath)
            if (-not $resolvedOldVersion.StartsWith(
                $resolvedRuntimeRoot, [StringComparison]::OrdinalIgnoreCase)) {
                throw "Refusing to remove a runtime outside '$runtimeInstallDirectory'."
            }
            Remove-Item -LiteralPath $resolvedOldVersion -Recurse -Force -ErrorAction Stop
        }
        catch {
            Write-Warning "Old runtime remains in use and will be cleaned later: $oldRuntimePath"
        }
    }

Get-ChildItem -LiteralPath $runtimeInstallDirectory -Filter "CorelDeepnest.Runtime.*.dll" `
    -File -ErrorAction SilentlyContinue | ForEach-Object {
        try {
            Remove-Item -LiteralPath $_.FullName -Force -ErrorAction Stop
        }
        catch {
            Write-Warning "Legacy runtime remains in use and will be cleaned later: $($_.FullName)"
        }
    }

Copy-Item -LiteralPath $CorelProject -Destination $addonFile -Force

$archive = [System.IO.Compression.ZipFile]::Open($addonFile, [System.IO.Compression.ZipArchiveMode]::Update)
try {
    function Replace-Entry {
        param(
            [System.IO.Compression.ZipArchive]$Archive,
            [string]$EntryName,
            [string]$Text,
            [System.Text.Encoding]$Encoding = [System.Text.UTF8Encoding]::new($false)
        )

        $existing = $Archive.GetEntry($EntryName)
        if (-not $existing) {
            throw "Required VSTA package entry was not found: $EntryName"
        }

        $existing.Delete()
        $entry = $Archive.CreateEntry($EntryName, [System.IO.Compression.CompressionLevel]::Optimal)
        $stream = $entry.Open()
        try {
            $preamble = $Encoding.GetPreamble()
            if ($preamble.Length -gt 0) {
                $stream.Write($preamble, 0, $preamble.Length)
            }
            $bytes = $Encoding.GetBytes($Text)
            $stream.Write($bytes, 0, $bytes.Length)
        }
        finally {
            $stream.Dispose()
        }
    }

    function Replace-BinaryEntry {
        param(
            [System.IO.Compression.ZipArchive]$Archive,
            [string]$EntryName,
            [string]$SourcePath
        )

        $existing = $Archive.GetEntry($EntryName)
        if ($existing) {
            $existing.Delete()
        }

        $entry = $Archive.CreateEntry($EntryName, [System.IO.Compression.CompressionLevel]::Optimal)
        $source = [System.IO.File]::OpenRead($SourcePath)
        $destination = $entry.Open()
        try {
            $source.CopyTo($destination)
        }
        finally {
            $destination.Dispose()
            $source.Dispose()
        }
    }

    $macroText = [System.IO.File]::ReadAllText($macroSource)
    Replace-Entry $archive "content/VSTA_CS_Project/Macro.cs" $macroText
    Replace-BinaryEntry $archive `
        "content/VSTA_CS_Project/CorelDeepnest.Runtime.dll" $runtimeOutput
    Replace-BinaryEntry $archive `
        "content/VSTA_CS_Project/CorelDeepnest.Contracts.dll" $contractsOutput

    $projectEntry = $archive.GetEntry("content/VSTA_CS_Project/CorelDeepnest.csproj")
    if (-not $projectEntry) {
        throw "The Corel-created C# project was not found inside the addon."
    }

    $reader = [System.IO.StreamReader]::new($projectEntry.Open())
    try {
        $projectText = $reader.ReadToEnd()
    }
    finally {
        $reader.Dispose()
    }

    $projectText = $projectText.Replace(
        '<TargetFrameworkVersion>v4.5</TargetFrameworkVersion>',
        '<TargetFrameworkVersion>v4.8</TargetFrameworkVersion>'
    )

    if ($projectText -notmatch 'EmbeddedResource Include="CorelDeepnest\.Runtime\.dll"') {
        $runtimeResource = @"
	<ItemGroup>
		<EmbeddedResource Include="CorelDeepnest.Runtime.dll">
			<LogicalName>CorelDeepnest.Runtime.dll</LogicalName>
		</EmbeddedResource>
		<EmbeddedResource Include="CorelDeepnest.Contracts.dll">
			<LogicalName>CorelDeepnest.Contracts.dll</LogicalName>
		</EmbeddedResource>
	</ItemGroup>
"@
        $projectText = $projectText.Replace(
            '<Import Project="$(MSBuildToolsPath)\Microsoft.CSharp.targets" />',
            $runtimeResource + "`r`n`t<Import Project=`"`$(MSBuildToolsPath)\Microsoft.CSharp.targets`" />"
        )
    }

    Replace-Entry $archive "content/VSTA_CS_Project/CorelDeepnest.csproj" $projectText

    $manifestEntry = $archive.GetEntry("content/VSTA_CS_Project/Project")
    if (-not $manifestEntry) {
        throw "The Corel VSTA project manifest was not found inside the addon."
    }

    $manifestReader = [System.IO.StreamReader]::new(
        $manifestEntry.Open(), [System.Text.Encoding]::Unicode, $true)
    try {
        $manifestText = $manifestReader.ReadToEnd()
    }
    finally {
        $manifestReader.Dispose()
    }

    if ($manifestText -notmatch 'File Include="CorelDeepnest\.Runtime\.dll"') {
        $manifestText = $manifestText.Replace(
            '<msb:File Include="Macro.Internal.cs"/>',
            '<msb:File Include="Macro.Internal.cs"/>' + "`r`n`t`t<msb:File Include=`"CorelDeepnest.Runtime.dll`"/>"
        )
    }

    if ($manifestText -notmatch 'File Include="CorelDeepnest\.Contracts\.dll"') {
        $manifestText = $manifestText.Replace(
            '<msb:File Include="CorelDeepnest.Runtime.dll"/>',
            '<msb:File Include="CorelDeepnest.Runtime.dll"/>' +
            "`r`n`t`t<msb:File Include=`"CorelDeepnest.Contracts.dll`"/>"
        )
    }

    Replace-Entry $archive "content/VSTA_CS_Project/Project" $manifestText `
        ([System.Text.UnicodeEncoding]::new($false, $true))
}
finally {
    $archive.Dispose()
}

Write-Host "Published hot-reload runtime: $installedRuntime"
Write-Host "Created VSTA loader package: $addonFile"
