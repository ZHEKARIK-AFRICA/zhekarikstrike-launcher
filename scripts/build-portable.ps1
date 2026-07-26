param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,

    [string]$OutputDirectory,

    [string]$TargetTriple = 'x86_64-pc-windows-msvc'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$releaseDir = Join-Path $repoRoot "src-tauri\target\$TargetTriple\release"
$portableDir = Join-Path $releaseDir 'portable'
$sourceExe = Join-Path $releaseDir 'zhekarikstrike_launcher.exe'

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = $releaseDir
}
$OutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
$archivePath = Join-Path $OutputDirectory "ZHEKARIK-STRIKE_${Version}_windows-x86_64-portable.zip"
$targetExe = Join-Path $portableDir 'ZHEKARIK STRIKE.exe'

if (-not (Test-Path -LiteralPath $sourceExe -PathType Leaf)) {
    throw "Release exe not found: $sourceExe"
}

$expectedPortablePrefix = [System.IO.Path]::GetFullPath($releaseDir).TrimEnd('\') + '\'
$resolvedPortableDir = [System.IO.Path]::GetFullPath($portableDir)
if (-not $resolvedPortableDir.StartsWith($expectedPortablePrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Unsafe portable directory: $resolvedPortableDir"
}

if (Test-Path -LiteralPath $portableDir) {
    Remove-Item -LiteralPath $portableDir -Recurse -Force
}
New-Item -ItemType Directory -Path $portableDir | Out-Null
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null

Copy-Item -LiteralPath $sourceExe -Destination $targetExe
Copy-Item -LiteralPath (Join-Path $repoRoot 'public\game_files') -Destination (Join-Path $portableDir 'game_files') -Recurse
Copy-Item -LiteralPath (Join-Path $repoRoot 'public\game_files_pure') -Destination (Join-Path $portableDir 'game_files_pure') -Recurse

if (Test-Path -LiteralPath $archivePath) {
    Remove-Item -LiteralPath $archivePath -Force
}
Compress-Archive -Path (Join-Path $portableDir '*') -DestinationPath $archivePath
Write-Host "Portable artifact: $archivePath"
