param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Executable,

    [Parameter(Position = 1, ValueFromRemainingArguments = $true)]
    [string[]]$RemainingArguments
)

$ErrorActionPreference = 'Stop'

$fileName = [System.IO.Path]::GetFileName($Executable)
if ($fileName -like 'zhekarikstrike_launcher_lib-*.exe' -and $Executable -like '*\deps\*') {
    $manifest = Join-Path $PSScriptRoot '..\src-tauri\tests\windows-test.manifest'
    $kitsRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $manifestTool = Get-ChildItem -Path $kitsRoot -Filter mt.exe -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like '*\x64\mt.exe' } |
        Sort-Object { [version]$_.Directory.Parent.Name } -Descending |
        Select-Object -First 1
    if (-not $manifestTool) {
        throw 'Windows SDK manifest tool (mt.exe) was not found'
    }

    & $manifestTool.FullName -manifest $manifest "-outputresource:$Executable;#1" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "mt.exe failed with exit code $LASTEXITCODE"
    }
}

& $Executable @RemainingArguments
exit $LASTEXITCODE
