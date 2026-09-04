[CmdletBinding()]
param(
    [switch]$SkipFocusedTests
)

$repoRoot = Split-Path -Parent $PSScriptRoot
$adaptiveProbeVariable = 'ZHEKARIK_ADAPTIVE_PACK_PROBE'
$cargoJobsVariable = 'CARGO_BUILD_JOBS'
$hadAdaptiveProbeValue = Test-Path "Env:$adaptiveProbeVariable"
$originalAdaptiveProbeValue = [Environment]::GetEnvironmentVariable($adaptiveProbeVariable, 'Process')
$hadCargoJobsValue = Test-Path "Env:$cargoJobsVariable"
$originalCargoJobsValue = [Environment]::GetEnvironmentVariable($cargoJobsVariable, 'Process')
$locationPushed = $false

function Invoke-NativeChecked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [Parameter(Mandatory = $true)]
        [string[]]$ArgumentList
    )

    & $FilePath @ArgumentList
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code ${LASTEXITCODE}: $FilePath $($ArgumentList -join ' ')"
    }
}

Write-Host 'Adaptive pack probe uses <=2GiB received pack bodies in fixed2/adaptive/adaptive/fixed2 order.'
Write-Host 'Reports live under D:\zhekarik-adaptive-pack-probe; user installation untouched.'

try {
    Push-Location $repoRoot
    $locationPushed = $true

    if (-not $hadCargoJobsValue) {
        [Environment]::SetEnvironmentVariable($cargoJobsVariable, '2', 'Process')
    }

    if (-not $SkipFocusedTests) {
        Invoke-NativeChecked -FilePath 'cargo' -ArgumentList @('fmt', '--manifest-path', 'src-tauri/Cargo.toml', '--', '--check')
        Invoke-NativeChecked -FilePath 'cargo' -ArgumentList @('test', '--manifest-path', 'src-tauri/Cargo.toml', '--release', 'drive_pack_', '--lib')
    }

    [Environment]::SetEnvironmentVariable($adaptiveProbeVariable, '1', 'Process')
    Invoke-NativeChecked -FilePath 'cargo' -ArgumentList @('test', '--manifest-path', 'src-tauri/Cargo.toml', '--release', '--lib', 'drive_pack_probe_tests::drive_pack_live_adaptive_probe', '--', '--ignored', '--exact', '--nocapture')
}
finally {
    if ($hadAdaptiveProbeValue) {
        [Environment]::SetEnvironmentVariable($adaptiveProbeVariable, $originalAdaptiveProbeValue, 'Process')
    } else {
        [Environment]::SetEnvironmentVariable($adaptiveProbeVariable, $null, 'Process')
    }
    if ($hadCargoJobsValue) {
        [Environment]::SetEnvironmentVariable($cargoJobsVariable, $originalCargoJobsValue, 'Process')
    } else {
        [Environment]::SetEnvironmentVariable($cargoJobsVariable, $null, 'Process')
    }
    if ($locationPushed) {
        Pop-Location
    }
}
