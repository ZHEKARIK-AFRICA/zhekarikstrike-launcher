param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,

    [switch]$Publish
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )

    Write-Host "> $Command $($Arguments -join ' ')" -ForegroundColor Cyan
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code ${LASTEXITCODE}: $Command $($Arguments -join ' ')"
    }
}

function Get-RequiredCommand {
    param([Parameter(Mandatory = $true)][string]$Name)

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($null -eq $command) {
        throw "Required command is not installed or not on PATH: $Name"
    }
    return $command.Source
}

function Assert-Version {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Actual,
        [Parameter(Mandatory = $true)][string]$Expected
    )

    if ($Actual -ne $Expected) {
        throw "Version mismatch in ${Source}: expected $Expected, got $Actual"
    }
}

function Invoke-MinisignWithPassword {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$Password
    )

    $passwordFile = Join-Path ([System.IO.Path]::GetTempPath()) "zhekarik-minisign-password-$([guid]::NewGuid().ToString('N')).tmp"
    try {
        [System.IO.File]::WriteAllText($passwordFile, "$Password`r`n", [Text.Encoding]::ASCII)
        $quotedExecutable = '"' + $Executable.Replace('"', '\"') + '"'
        $quotedArguments = $Arguments | ForEach-Object { '"' + $_.Replace('"', '\"') + '"' }
        $commandLine = "$quotedExecutable $($quotedArguments -join ' ') < `"$passwordFile`""
        & cmd.exe /d /s /c $commandLine
        if ($LASTEXITCODE -ne 0) {
            throw "minisign failed with exit code $LASTEXITCODE"
        }
    } finally {
        if (Test-Path -LiteralPath $passwordFile) {
            Remove-Item -LiteralPath $passwordFile -Force
        }
    }
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $repoRoot

$temporarySecretKey = $null
$loadedLocalPassword = $false
try {
    $npm = Get-RequiredCommand 'npm.cmd'
    $npx = Get-RequiredCommand 'npx.cmd'
    $cargo = Get-RequiredCommand 'cargo.exe'
    $git = Get-RequiredCommand 'git.exe'
    $minisign = Get-RequiredCommand 'minisign.exe'

    $package = Get-Content -LiteralPath (Join-Path $repoRoot 'package.json') -Raw | ConvertFrom-Json
    $packageLock = Get-Content -LiteralPath (Join-Path $repoRoot 'package-lock.json') -Raw | ConvertFrom-Json
    $tauriConfig = Get-Content -LiteralPath (Join-Path $repoRoot 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json
    $cargoToml = Get-Content -LiteralPath (Join-Path $repoRoot 'src-tauri\Cargo.toml') -Raw
    $cargoMatch = [regex]::Match($cargoToml, '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"')
    if (-not $cargoMatch.Success) {
        throw 'Unable to read [package].version from src-tauri/Cargo.toml'
    }

    Assert-Version 'package.json' $package.version $Version
    Assert-Version 'package-lock.json' $packageLock.version $Version
    Assert-Version 'src-tauri/Cargo.toml' $cargoMatch.Groups[1].Value $Version
    Assert-Version 'src-tauri/tauri.conf.json' $tauriConfig.version $Version

    $expectedTag = "v$Version"
    $tag = $env:GITHUB_REF_NAME
    if ([string]::IsNullOrWhiteSpace($tag)) {
        $tagOutput = & $git describe --tags --exact-match 2>$null
        if ($LASTEXITCODE -eq 0) {
            $tag = ($tagOutput | Select-Object -First 1).Trim()
        } else {
            $tag = $null
        }
    }
    if ([string]::IsNullOrWhiteSpace($tag)) {
        throw "A release must be built from the exact Git tag $expectedTag"
    }
    Assert-Version 'Git tag' $tag $expectedTag

    $defaultKeyDirectory = Join-Path $env:LOCALAPPDATA 'ZHEKARIKSTRIKE\release-keys'
    $secretKeyPath = $env:MINISIGN_SECRET_KEY_PATH
    if (-not [string]::IsNullOrWhiteSpace($env:MINISIGN_SECRET_KEY_BASE64)) {
        $temporarySecretKey = Join-Path ([System.IO.Path]::GetTempPath()) "zhekarik-minisign-$([guid]::NewGuid().ToString('N')).key"
        [System.IO.File]::WriteAllBytes(
            $temporarySecretKey,
            [Convert]::FromBase64String($env:MINISIGN_SECRET_KEY_BASE64)
        )
        $secretKeyPath = $temporarySecretKey
    }
    if ([string]::IsNullOrWhiteSpace($secretKeyPath)) {
        $defaultSecretKey = Join-Path $defaultKeyDirectory 'updater.key'
        if (Test-Path -LiteralPath $defaultSecretKey -PathType Leaf) {
            $secretKeyPath = $defaultSecretKey
        }
    }
    if ([string]::IsNullOrWhiteSpace($secretKeyPath) -or -not (Test-Path -LiteralPath $secretKeyPath -PathType Leaf)) {
        throw 'Set MINISIGN_SECRET_KEY_BASE64 or MINISIGN_SECRET_KEY_PATH to a private key outside Git.'
    }
    $secretKeyPath = (Resolve-Path -LiteralPath $secretKeyPath).Path
    $repoPrefix = $repoRoot.TrimEnd('\') + '\'
    if ($secretKeyPath.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The minisign private key must not be stored inside the repository.'
    }
    if ([string]::IsNullOrWhiteSpace($env:MINISIGN_PASSWORD)) {
        $protectedPasswordPath = Join-Path $defaultKeyDirectory 'updater.password.dpapi'
        if (Test-Path -LiteralPath $protectedPasswordPath -PathType Leaf) {
            $securePassword = Get-Content -LiteralPath $protectedPasswordPath -Raw | ConvertTo-SecureString
            $credential = [PSCredential]::new('minisign', $securePassword)
            $env:MINISIGN_PASSWORD = $credential.GetNetworkCredential().Password
            $loadedLocalPassword = $true
        } else {
            throw 'MINISIGN_PASSWORD must be set so minisign never prompts during a release.'
        }
    }

    $publicKeyPath = Join-Path $repoRoot 'src-tauri\updater.pub'
    $publicKeyLines = Get-Content -LiteralPath $publicKeyPath | Where-Object {
        -not [string]::IsNullOrWhiteSpace($_) -and -not $_.StartsWith('untrusted comment:')
    }
    if ($publicKeyLines.Count -ne 1 -or $publicKeyLines[0] -notmatch '^RWQ[A-Za-z0-9+/=]+$') {
        throw 'src-tauri/updater.pub must contain a complete minisign public key.'
    }
    $publicKeyBase64 = $publicKeyLines[0]

    Invoke-Native $npm @('ci')
    Invoke-Native $npm @('run', 'lint')
    Invoke-Native $npm @('run', 'test:unit')
    Invoke-Native $npm @('run', 'test:e2e:browser')
    Invoke-Native $npm @('run', 'build:frontend')
    Invoke-Native $cargo @('fmt', '--manifest-path', 'src-tauri/Cargo.toml', '--', '--check')
    Invoke-Native $cargo @('clippy', '--manifest-path', 'src-tauri/Cargo.toml', '--all-targets', '--', '-D', 'warnings')
    Invoke-Native $cargo @('test', '--manifest-path', 'src-tauri/Cargo.toml')
    Invoke-Native $npm @('run', 'test:e2e:tauri')

    # Debug + WebDriver artifacts are large on Windows and are not release inputs.
    Invoke-Native $cargo @('clean', '--manifest-path', 'src-tauri/Cargo.toml', '--profile', 'dev')

    $targetTriple = 'x86_64-pc-windows-msvc'
    Invoke-Native $npx @('tauri', 'build', '--target', $targetTriple, '--bundles', 'nsis')

    $targetRelease = Join-Path $repoRoot "src-tauri\target\$targetTriple\release"
    $rawSource = Join-Path $targetRelease 'zhekarikstrike_launcher.exe'
    $nsisDirectory = Join-Path $targetRelease 'bundle\nsis'
    if (-not (Test-Path -LiteralPath $rawSource -PathType Leaf)) {
        throw "Raw launcher was not produced: $rawSource"
    }
    $installer = Get-ChildItem -LiteralPath $nsisDirectory -Filter '*.exe' -File |
        Where-Object { $_.Name -like "*${Version}*" } |
        Select-Object -First 1
    if ($null -eq $installer) {
        throw "NSIS installer for version $Version was not produced in $nsisDirectory"
    }

    $releaseRoot = Join-Path $repoRoot 'release'
    $artifactDirectory = [System.IO.Path]::GetFullPath((Join-Path $releaseRoot $Version))
    $expectedReleasePrefix = [System.IO.Path]::GetFullPath($releaseRoot).TrimEnd('\') + '\'
    if (-not $artifactDirectory.StartsWith($expectedReleasePrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Unsafe artifact directory: $artifactDirectory"
    }
    if (Test-Path -LiteralPath $artifactDirectory) {
        Remove-Item -LiteralPath $artifactDirectory -Recurse -Force
    }
    New-Item -ItemType Directory -Path $artifactDirectory | Out-Null

    $updateAssetName = "ZHEKARIK-STRIKE_${Version}_windows-x86_64.exe"
    $installerAssetName = "ZHEKARIK-STRIKE_${Version}_windows-x86_64-setup.exe"
    $updateAsset = Join-Path $artifactDirectory $updateAssetName
    $installerAsset = Join-Path $artifactDirectory $installerAssetName
    Copy-Item -LiteralPath $rawSource -Destination $updateAsset
    Copy-Item -LiteralPath $installer.FullName -Destination $installerAsset

    Invoke-Native $npx @('tauri', 'build', '--target', $targetTriple, '--no-bundle', '--features', 'portable')
    Invoke-Native 'powershell.exe' @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass',
        '-File', (Join-Path $repoRoot 'scripts\build-portable.ps1'),
        '-Version', $Version,
        '-OutputDirectory', $artifactDirectory,
        '-TargetTriple', $targetTriple
    )

    $signaturePath = "$updateAsset.minisig"
    Invoke-MinisignWithPassword $minisign @('-S', '-s', $secretKeyPath, '-m', $updateAsset, '-x', $signaturePath) $env:MINISIGN_PASSWORD
    Invoke-Native $minisign @('-Vm', $updateAsset, '-P', $publicKeyBase64, '-x', $signaturePath)

    $repository = $env:GITHUB_REPOSITORY
    if ([string]::IsNullOrWhiteSpace($repository)) {
        $remote = (& $git remote get-url origin).Trim()
        if ($LASTEXITCODE -ne 0 -or $remote -notmatch 'github\.com[:/]([^/]+/[^/]+?)(?:\.git)?$') {
            throw 'Unable to determine GitHub repository from GITHUB_REPOSITORY or origin.'
        }
        $repository = $Matches[1]
    }

    $sha256 = (Get-FileHash -LiteralPath $updateAsset -Algorithm SHA256).Hash.ToLowerInvariant()
    $signature = Get-Content -LiteralPath $signaturePath -Raw
    $downloadUrl = "https://github.com/$repository/releases/download/$expectedTag/$updateAssetName"
    $manifest = [ordered]@{
        version = $Version
        notes = ''
        pub_date = [DateTimeOffset]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
        platforms = [ordered]@{
            'windows-x86_64' = [ordered]@{
                url = $downloadUrl
                sha256 = $sha256
                signature = $signature
                size = (Get-Item -LiteralPath $updateAsset).Length
            }
        }
    }
    $manifestJson = $manifest | ConvertTo-Json -Depth 6
    $manifestPath = Join-Path $artifactDirectory "launcher-update-${Version}.json"
    [System.IO.File]::WriteAllText($manifestPath, $manifestJson, [System.Text.UTF8Encoding]::new($false))

    if ($Publish) {
        if ([string]::IsNullOrWhiteSpace($env:GH_TOKEN)) {
            throw 'GH_TOKEN is required with -Publish.'
        }
        if ([string]::IsNullOrWhiteSpace($env:LAUNCHER_RELEASE_API_TOKEN)) {
            throw 'LAUNCHER_RELEASE_API_TOKEN is required with -Publish.'
        }
        $gh = Get-RequiredCommand 'gh.exe'
        $assets = Get-ChildItem -LiteralPath $artifactDirectory -File | ForEach-Object { $_.FullName }
        & $gh release view $expectedTag --repo $repository *> $null
        if ($LASTEXITCODE -eq 0) {
            Invoke-Native $gh (@('release', 'upload', $expectedTag, '--repo', $repository, '--clobber') + $assets)
        } else {
            Invoke-Native $gh (@('release', 'create', $expectedTag, '--repo', $repository, '--verify-tag', '--title', $expectedTag, '--generate-notes') + $assets)
        }

        $apiBase = $env:LAUNCHER_RELEASE_API_BASE_URL
        if ([string]::IsNullOrWhiteSpace($apiBase)) {
            $apiBase = 'https://api.zhekarik.africa'
        }
        $publishUri = "$($apiBase.TrimEnd('/'))/admin/launcher/releases/windows/x86_64/$Version"
        Invoke-RestMethod -Method Put -Uri $publishUri -Headers @{
            Authorization = "Bearer $($env:LAUNCHER_RELEASE_API_TOKEN)"
        } -ContentType 'application/json' -Body $manifestJson | Out-Null
        Write-Host "Published signed launcher manifest: $publishUri" -ForegroundColor Green
    }

    Write-Host "Signed release bundle: $artifactDirectory" -ForegroundColor Green
} finally {
    if ($null -ne $temporarySecretKey -and (Test-Path -LiteralPath $temporarySecretKey)) {
        Remove-Item -LiteralPath $temporarySecretKey -Force
    }
    if ($loadedLocalPassword) {
        Remove-Item Env:MINISIGN_PASSWORD -ErrorAction SilentlyContinue
    }
    Pop-Location
}
