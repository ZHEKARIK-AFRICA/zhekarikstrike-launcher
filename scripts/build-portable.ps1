$ErrorActionPreference = 'Stop'

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
$releaseDir = Join-Path $repoRoot 'src-tauri\target\release'
$portableDir = Join-Path $releaseDir 'portable'
$archivePath = Join-Path $releaseDir 'ZHEKARIK STRIKE_1.6.0_x64-portable.zip'
$sourceExe = Join-Path $releaseDir 'zhekarikstrike_launcher.exe'
$targetExe = Join-Path $portableDir 'ZHEKARIK STRIKE.exe'

if (-not (Test-Path $sourceExe)) {
    throw "Release exe not found: $sourceExe"
}

if (Test-Path $portableDir) {
    Remove-Item -LiteralPath $portableDir -Recurse -Force
}
New-Item -ItemType Directory -Path $portableDir | Out-Null

Copy-Item -LiteralPath $sourceExe -Destination $targetExe -Force
Copy-Item -LiteralPath (Join-Path $repoRoot 'public\game_files') -Destination (Join-Path $portableDir 'game_files') -Recurse -Force
Copy-Item -LiteralPath (Join-Path $repoRoot 'public\game_files_pure') -Destination (Join-Path $portableDir 'game_files_pure') -Recurse -Force

if (Test-Path $archivePath) {
    Remove-Item -LiteralPath $archivePath -Force
}
Compress-Archive -Path (Join-Path $portableDir '*') -DestinationPath $archivePath -Force
Write-Host "Portable artifact: $archivePath"
