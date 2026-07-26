$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$plainText = 'release-password-regression'
$path = Join-Path ([IO.Path]::GetTempPath()) "release-password-$([guid]::NewGuid()).dpapi"
try {
    ConvertTo-SecureString $plainText -AsPlainText -Force |
        ConvertFrom-SecureString |
        Set-Content -LiteralPath $path -Encoding ASCII
    . "$PSScriptRoot\..\scripts\release-helpers.ps1"
    $actual = Read-DpapiProtectedSecret -Path $path
    if ($actual -ne $plainText) {
        throw 'DPAPI password did not round-trip'
    }
} finally {
    Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
}

$npm = (Get-Command npm.cmd -ErrorAction Stop).Source
$packageDirectory = Join-Path ([IO.Path]::GetTempPath()) "release-npm-$([guid]::NewGuid())"
$previousWorkspaces = [string]$env:npm_config_workspaces
try {
    New-Item -ItemType Directory -Path $packageDirectory | Out-Null
    @{
        name = 'release-npm-regression'
        version = '1.0.0'
        private = $true
        scripts = @{ probe = 'node -e "process.exit(0)"' }
    } | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $packageDirectory 'package.json') -Encoding UTF8
    $env:npm_config_workspaces = 'true'
    Push-Location $packageDirectory
    try {
        Invoke-NpmWithoutWorkspaces -Executable $npm -Arguments @('run', 'probe')
    } finally {
        Pop-Location
    }
} finally {
    $env:npm_config_workspaces = $previousWorkspaces
    Remove-Item -LiteralPath $packageDirectory -Recurse -Force -ErrorAction SilentlyContinue
}
