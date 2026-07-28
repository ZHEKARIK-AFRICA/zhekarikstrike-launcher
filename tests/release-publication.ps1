$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

. "$PSScriptRoot\..\scripts\release-helpers.ps1"

$signaturePath = Join-Path ([IO.Path]::GetTempPath()) "release-signature-$([guid]::NewGuid()).minisig"
try {
    [IO.File]::WriteAllText($signaturePath, "untrusted comment: test`r`nsignature`r`n")
    $signatureText = Read-ReleaseTextFile -Path $signaturePath
    $roundTrip = @{ signature = $signatureText } | ConvertTo-Json | ConvertFrom-Json
    if ($roundTrip.signature -isnot [string]) {
        throw 'Release text files must serialize as JSON strings without PowerShell provider metadata'
    }
} finally {
    Remove-Item -LiteralPath $signaturePath -Force -ErrorAction SilentlyContinue
}

function New-TestMinisignText([string]$Algorithm) {
    $record = [byte[]]::new(74)
    $record[0] = [byte][char]$Algorithm[0]
    $record[1] = [byte][char]$Algorithm[1]
    return "untrusted comment: test`n$([Convert]::ToBase64String($record))`ntrusted comment: test`n$([Convert]::ToBase64String([byte[]]::new(64)))`n"
}

Assert-StreamingMinisignSignature -SignatureText (New-TestMinisignText 'ED')

$legacyRejected = $false
try {
    Assert-StreamingMinisignSignature -SignatureText (New-TestMinisignText 'Ed')
} catch {
    $legacyRejected = $_.Exception.Message -match 'streaming ED'
}
if (-not $legacyRejected) {
    throw 'Legacy Ed launcher signature was accepted by the release pipeline.'
}

function New-LauncherManifest {
    param(
        [string]$Version = '1.6.1',
        [string]$Sha256 = ('a' * 64)
    )

    return [pscustomobject]@{
        version = $Version
        notes = ''
        pub_date = '2026-07-26T12:00:00Z'
        platforms = [pscustomobject]@{
            'windows-x86_64' = [pscustomobject]@{
                url = "https://github.com/d3affy/zhekarikstrike-launcher/releases/download/v$Version/ZHEKARIK-STRIKE_${Version}_windows-x86_64.exe"
                sha256 = $Sha256
                signature = "signature-$Sha256"
                size = 123
            }
        }
    }
}

$candidate = New-LauncherManifest
$sameActive = New-LauncherManifest
$action = Resolve-LauncherPublicationAction `
    -CandidateManifest $candidate `
    -ActiveManifest $sameActive `
    -ReleaseExists $true
if ($action -ne 'VerifyExistingRelease') {
    throw "Expected VerifyExistingRelease, got $action"
}

$createAction = Resolve-LauncherPublicationAction `
    -CandidateManifest $candidate `
    -ActiveManifest $null `
    -ReleaseExists $false
if ($createAction -ne 'CreateRelease') {
    throw "Expected CreateRelease, got $createAction"
}

$reuseAction = Resolve-LauncherPublicationAction `
    -CandidateManifest $candidate `
    -ActiveManifest $null `
    -ReleaseExists $true
if ($reuseAction -ne 'VerifyExistingRelease') {
    throw "Expected VerifyExistingRelease, got $reuseAction"
}

$conflictRejected = $false
try {
    Resolve-LauncherPublicationAction `
        -CandidateManifest $candidate `
        -ActiveManifest (New-LauncherManifest -Sha256 ('b' * 64)) `
        -ReleaseExists $true | Out-Null
} catch {
    if ($_.Exception.Message -notmatch 'already active with another artifact') {
        throw
    }
    $conflictRejected = $true
}
if (-not $conflictRejected) {
    throw 'Conflicting active artifact was accepted'
}

$downgradeRejected = $false
try {
    Resolve-LauncherPublicationAction `
        -CandidateManifest $candidate `
        -ActiveManifest (New-LauncherManifest -Version '1.6.2') `
        -ReleaseExists $true | Out-Null
} catch {
    if ($_.Exception.Message -notmatch 'active version 1\.6\.2 is newer') {
        throw
    }
    $downgradeRejected = $true
}
if (-not $downgradeRejected) {
    throw 'Launcher release downgrade was accepted'
}

$releaseSource = Get-Content -LiteralPath "$PSScriptRoot\..\scripts\release.ps1" -Raw
if ($releaseSource -match '(?i)--clobber') {
    throw 'Published launcher assets must never be overwritten with --clobber'
}

$tauriConfig = Get-Content -LiteralPath "$PSScriptRoot\..\src-tauri\tauri.conf.json" -Raw | ConvertFrom-Json
$resourcesProperty = $tauriConfig.bundle.PSObject.Properties['resources']
if ($null -ne $resourcesProperty -and $resourcesProperty.Value.Count -ne 0) {
    throw 'Tauri release builds must not bundle launcher game patch directories'
}

$portableSource = Get-Content -LiteralPath "$PSScriptRoot\..\scripts\build-portable.ps1" -Raw
if ($portableSource -match 'Compress-Archive|game_files|game_files_pure|portable\.zip') {
    throw 'Portable release must be one EXE without copied patch directories or ZIP packaging'
}
if ($portableSource -notmatch 'ZHEKARIK-STRIKE_\$\{Version\}_windows-x86_64\.exe') {
    throw 'Portable builder must produce the canonical launcher EXE name'
}

$portableBuild = $releaseSource.IndexOf("'--features', 'portable'", [StringComparison]::Ordinal)
$portablePackaging = $releaseSource.IndexOf("'scripts\build-portable.ps1'", [StringComparison]::Ordinal)
if ($portableBuild -lt 0 -or $portablePackaging -lt 0 -or $portableBuild -gt $portablePackaging) {
    throw 'Canonical updater EXE must be packaged only after the portable-feature build'
}
if ($releaseSource -match 'portable\.zip') {
    throw 'Release publication must not include a portable ZIP'
}
