param(
    [Parameter(Mandatory)][string]$SeriesDirectory,
    [Parameter(Mandatory)][ValidateSet('baseline','candidate')][string]$Kind,
    [Parameter(Mandatory)][string]$ProductionCommit,
    [Parameter(Mandatory)][string]$SourceCommit,
    [Parameter(Mandatory)][string]$Executable
)
$ErrorActionPreference = 'Stop'
$series = Get-Item -LiteralPath $SeriesDirectory
if ($series.Parent.FullName -ne 'D:\zhekarik-adaptive-pack-probe' -or $series.Name -notmatch '^next-([0-9a-f-]{36})$') { throw 'Not an owned comparison directory' }
$marker = 'adaptive-pack-next-v1:' + $Matches[1]
$markerPath = Join-Path $series.FullName 'owner.txt'
if (Test-Path -LiteralPath $markerPath) {
    if ([IO.File]::ReadAllText($markerPath) -ne $marker) { throw 'Owner mismatch' }
} else { [IO.File]::WriteAllText($markerPath, $marker, [Text.UTF8Encoding]::new($false)) }
$destination = Join-Path $series.FullName "builds\$Kind"
New-Item -ItemType Directory -Path $destination -Force | Out-Null
$target = Join-Path $destination 'probe.exe'
if ((Resolve-Path -LiteralPath $Executable).Path -ne $target) { Copy-Item -LiteralPath $Executable -Destination $target -Force }
$metadata = @{
    schema_version = 1; kind = $Kind; production_commit = $ProductionCommit; source_commit = $SourceCommit
    sha256 = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant()
    rustc = (& rustc --version); cargo = (& cargo --version); profile = 'release'; run_profile = 'Optimized'
    cargo_build_jobs = 2; root_opt_level = 0; root_codegen_units = 16; windows_opt_level = 1
    core_opt_level = $(if ($Kind -eq 'candidate') { 3 } else { $null }); core_codegen_units = $(if ($Kind -eq 'candidate') { 16 } else { $null })
    lto = $false; elevation = 'asInvoker'; saved_at_utc = [DateTime]::UtcNow.ToString('o')
}
[IO.File]::WriteAllText("$target.build.json", ($metadata | ConvertTo-Json -Depth 5), [Text.UTF8Encoding]::new($false))
$metadata | ConvertTo-Json -Depth 5
