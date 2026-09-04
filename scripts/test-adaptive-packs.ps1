[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$BaselineExecutable,
    [Parameter(Mandatory)][string]$CandidateExecutable,
    [Parameter(Mandatory)][string]$SeriesDirectory
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$probeRoot = [IO.Path]::GetFullPath('D:\zhekarik-adaptive-pack-probe')
$seriesPath = (Resolve-Path -LiteralPath $SeriesDirectory).Path
$seriesItem = Get-Item -LiteralPath $seriesPath
if ($seriesItem.Parent.FullName -ne $probeRoot -or $seriesItem.Name -notmatch '^next-([0-9a-f-]{36})$') { throw 'A marked direct next-UUID probe directory is required' }
$seriesId = $Matches[1]
$marker = "adaptive-pack-next-v1:$seriesId"
if ([IO.File]::ReadAllText((Join-Path $seriesPath 'owner.txt')) -ne $marker) { throw 'Wrong probe owner' }
for ($entry = $seriesItem; $null -ne $entry; $entry = $entry.Parent) {
    if (($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Probe root is a reparse point' }
}
$ledgerPath = Join-Path $seriesPath 'comparison.json'
if (Test-Path -LiteralPath $ledgerPath) { throw 'This series already has a ledger; automatic repeat is prohibited' }
$baselinePath = (Resolve-Path -LiteralPath $BaselineExecutable).Path
$candidatePath = (Resolve-Path -LiteralPath $CandidateExecutable).Path
$baselineHash = (Get-FileHash -LiteralPath $baselinePath -Algorithm SHA256).Hash.ToLowerInvariant()
$candidateHash = (Get-FileHash -LiteralPath $candidatePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($baselineHash -eq $candidateHash) { throw 'Distinct saved baseline and candidate executables required' }
$baselineBuild = Get-Content -LiteralPath "$baselinePath.build.json" -Raw | ConvertFrom-Json
$candidateBuild = Get-Content -LiteralPath "$candidatePath.build.json" -Raw | ConvertFrom-Json
if (-not $baselineBuild.production_commit.StartsWith('80abb47') -or $baselineBuild.sha256 -ne $baselineHash -or $candidateBuild.sha256 -ne $candidateHash) { throw 'Saved executable provenance mismatch' }
$frozenRoot = 'D:\zhekarik-adaptive-pack-probe\series-442710e6-dffe-4da2-971f-58e9008523da'
$frozenReport = Get-Content -LiteralPath (Join-Path $frozenRoot 'report.json') -Raw | ConvertFrom-Json
$manifestPath = Join-Path $frozenRoot 'manifest.json'
$installFiles = @($frozenReport.files)
if ($installFiles.Count -ne 359) { throw 'Unexpected frozen install subset' }
$repairFiles = @('csgo/maps/cs_italy.bsp', 'csgo/maps/cs_rush.bsp', 'csgo/maps/de_aztec.bsp', 'csgo/maps/de_cbble.bsp')
$budget = [long]4294967296
$received = [long]0
$ledger = [ordered]@{
    schema_version = 1; status = 'running'; budget_bytes = $budget; received_bytes = 0
    baseline = $baselineBuild
    candidate = $candidateBuild
    seed = '461ea0b9-971f-4b65-bdea-0a7495a0b813'; manifest_path = $manifestPath
    runs = @(); conclusions = @{}; error = $null
}
function Save-Ledger {
    $temporary = "$ledgerPath.tmp"
    [IO.File]::WriteAllText($temporary, ($ledger | ConvertTo-Json -Depth 40), [Text.UTF8Encoding]::new($false))
    [IO.File]::Move($temporary, $ledgerPath, $true)
}
function Get-Conclusion([object[]]$Runs) {
    if ($Runs.Count -ne 4) { return @{ outcome = 'incomplete'; acceleration_confirmed = $false } }
    $a1 = [double]$Runs[0].pipeline_seconds; $b1 = [double]$Runs[1].pipeline_seconds
    $b2 = [double]$Runs[2].pipeline_seconds; $a2 = [double]$Runs[3].pipeline_seconds
    if (@($a1, $b1, $b2, $a2) | Where-Object { $_ -le 0 }) { throw 'Invalid pipeline time' }
    $pair1 = 1 - $b1 / $a1; $pair2 = 1 - $b2 / $a2
    $mean = 1 - ($b1 + $b2) / ($a1 + $a2)
    $confirmed = $pair1 -gt 0 -and $pair2 -gt 0 -and $mean -ge 0.10
    return @{ outcome = $(if ($confirmed) { 'acceleration_confirmed' } else { 'no_gain_or_unstable' }); acceleration_confirmed = $confirmed; pair_one_gain = $pair1; pair_two_gain = $pair2; mean_gain = $mean }
}
$oldOptIn = $env:ZHEKARIK_ADAPTIVE_PACK_PROBE
$oldInput = $env:ZHEKARIK_PACK_PROBE_INPUT
try {
    Save-Ledger
    $env:ZHEKARIK_ADAPTIVE_PACK_PROBE = '1'
    foreach ($scenario in @('install', 'repair')) {
        $files = if ($scenario -eq 'install') { $installFiles } else { $repairFiles }
        foreach ($slot in @('A1', 'B1', 'B2', 'A2')) {
            $exe = if ($slot.StartsWith('A')) { $baselinePath } else { $candidatePath }
            $expectedHash = if ($slot.StartsWith('A')) { $baselineHash } else { $candidateHash }
            if ((Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expectedHash) { throw 'Saved executable changed' }
            $remaining = $budget - $received
            if ($remaining -le 0) { throw 'Global network budget exhausted' }
            $name = "$scenario-$slot"
            $runReportPath = Join-Path $seriesPath "$name-report.json"
            $inputPath = Join-Path $seriesPath "$name-input.json"
            $inputObject = @{ manifest_path = $manifestPath; files = $files; budget_bytes = $remaining; seed = $ledger.seed; report_path = $runReportPath; scenario = $scenario; name = $name }
            [IO.File]::WriteAllText($inputPath, ($inputObject | ConvertTo-Json -Depth 10), [Text.UTF8Encoding]::new($false))
            $env:ZHEKARIK_PACK_PROBE_INPUT = $inputPath
            Write-Host "Starting $name; remaining global budget $remaining bytes"
            & $exe 'drive_pack_probe_tests::drive_pack_live_single_probe' '--ignored' '--exact' '--nocapture' 2>&1 | Tee-Object -FilePath (Join-Path $seriesPath "$name.log")
            $runExit = $LASTEXITCODE
            if (-not (Test-Path -LiteralPath $runReportPath)) { throw 'Run report missing; received bytes unknown, stopping entire comparison' }
            $run = Get-Content -LiteralPath $runReportPath -Raw | ConvertFrom-Json
            if ($run.status -notin @('complete', 'failed') -or $run.received_bytes -ne $run.snapshot.received_bytes -or $run.received_bytes -lt 0) { throw 'Run report incomplete or budget counters disagree; stopping entire comparison' }
            $received += [long]$run.received_bytes
            $ledger.received_bytes = $received
            $ledger.runs += $run
            $ledger.conclusions[$scenario] = Get-Conclusion @($ledger.runs | Where-Object { $_.scenario -eq $scenario })
            Save-Ledger
            if ($runExit -ne 0 -or $run.status -ne 'complete' -or $null -ne $run.error) { throw "Run $name failed; no further real downloads permitted" }
            if ($received -gt $budget -or -not $run.temporary_data_removed -or $run.snapshot.active_requests -ne 0 -or $run.snapshot.active_jobs -ne 0) { throw 'Budget/cleanup/draining invariant failed' }
            if (($run.protected_before | ConvertTo-Json -Depth 20 -Compress) -cne ($run.protected_after | ConvertTo-Json -Depth 20 -Compress)) { throw 'Protected installation changed' }
        }
    }
    $ledger.status = 'complete'
    Save-Ledger
    Write-Host "Comparison complete: $ledgerPath"
} catch {
    $ledger.status = 'incomplete'
    $ledger.error = $_.Exception.Message
    Save-Ledger
    throw
} finally {
    $env:ZHEKARIK_ADAPTIVE_PACK_PROBE = $oldOptIn
    $env:ZHEKARIK_PACK_PROBE_INPUT = $oldInput
}
