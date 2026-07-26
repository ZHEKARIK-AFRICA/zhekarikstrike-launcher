$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

. "$PSScriptRoot\..\scripts\release-helpers.ps1"

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
