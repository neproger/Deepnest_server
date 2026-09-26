[CmdletBinding()]
param(
    [string]$CorelProject
)

$ErrorActionPreference = "Stop"
$projectDirectory = $PSScriptRoot
$macroSource = Join-Path $projectDirectory "VstaMacro.cs"
$distDirectory = Join-Path $projectDirectory "dist"
$addonFile = Join-Path $distDirectory "CorelDeepnest.CGSaddon"

if ([string]::IsNullOrWhiteSpace($CorelProject)) {
    $CorelProject = Join-Path $projectDirectory "VstaTemplate\CorelDeepnest.CGSaddon"
}

if (-not (Test-Path -LiteralPath $CorelProject)) {
    throw "Corel-created VSTA project was not found: $CorelProject"
}

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

New-Item -ItemType Directory -Path $distDirectory -Force | Out-Null
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

    $macroText = [System.IO.File]::ReadAllText($macroSource)
    Replace-Entry $archive "content/VSTA_CS_Project/Macro.cs" $macroText

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

    if ($projectText -notmatch 'System\.Net\.Http') {
        $projectText = $projectText.Replace(
            '<Reference Include="System.Windows.Forms" />',
            "<Reference Include=`"System.Windows.Forms`" />`r`n`t`t<Reference Include=`"System.Net.Http`" />"
        )
    }

    if ($projectText -notmatch 'System\.Web\.Extensions') {
        $projectText = $projectText.Replace(
            '<Reference Include="System.Windows.Forms" />',
            "<Reference Include=`"System.Windows.Forms`" />`r`n`t`t<Reference Include=`"System.Web.Extensions`" />"
        )
    }

    Replace-Entry $archive "content/VSTA_CS_Project/CorelDeepnest.csproj" $projectText
}
finally {
    $archive.Dispose()
}

Write-Host "Created from the Corel VSTA project: $addonFile"
