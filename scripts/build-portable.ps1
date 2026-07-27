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
$sourceExe = Join-Path $releaseDir 'zhekarikstrike_launcher.exe'

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = $releaseDir
}
$OutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
$targetExe = Join-Path $OutputDirectory "ZHEKARIK-STRIKE_${Version}_windows-x86_64.exe"

if (-not (Test-Path -LiteralPath $sourceExe -PathType Leaf)) {
    throw "Release exe not found: $sourceExe"
}

New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null

if (Test-Path -LiteralPath $targetExe) {
    Remove-Item -LiteralPath $targetExe -Force
}
Copy-Item -LiteralPath $sourceExe -Destination $targetExe
Write-Host "Portable artifact: $targetExe"
