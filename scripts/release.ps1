param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,

    [switch]$Publish
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'release-helpers.ps1')

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

# Keep native C/C++ dependency builds below the memory pressure that can make
# MSVC fail to load its PDB runtime (C1356) on 16 GB Windows release hosts.
if ([string]::IsNullOrWhiteSpace([string]$env:CARGO_BUILD_JOBS)) {
    $env:CARGO_BUILD_JOBS = '2'
}

$temporarySecretKey = $null
$secretKeyBase64 = [string]$env:MINISIGN_SECRET_KEY_BASE64
$configuredSecretKeyPath = [string]$env:MINISIGN_SECRET_KEY_PATH
$signingPassword = [string]$env:MINISIGN_PASSWORD
$githubToken = [string]$env:GH_TOKEN
$releaseApiToken = [string]$env:LAUNCHER_RELEASE_API_TOKEN
foreach ($secretName in @(
    'MINISIGN_SECRET_KEY_BASE64',
    'MINISIGN_SECRET_KEY_PATH',
    'MINISIGN_PASSWORD',
    'GH_TOKEN',
    'LAUNCHER_RELEASE_API_TOKEN'
)) {
    Remove-Item "Env:$secretName" -ErrorAction SilentlyContinue
}
try {
    $npm = Get-RequiredCommand 'npm.cmd'
    $npx = Get-RequiredCommand 'npx.cmd'
    $cargo = Get-RequiredCommand 'cargo.exe'
    $git = Get-RequiredCommand 'git.exe'
    $minisign = Get-RequiredCommand 'minisign.exe'

    $minisignVersionOutput = @(& $minisign -v 2>&1)
    $minisignVersionExitCode = $LASTEXITCODE
    $minisignVersion = (($minisignVersionOutput | ForEach-Object { [string]$_ }) -join "`n").Trim()
    if ($minisignVersionExitCode -ne 0 -or $minisignVersion -cne 'minisign 0.12') {
        throw "Launcher releases require minisign 0.12; got '$minisignVersion'."
    }

    $package = Get-Content -LiteralPath (Join-Path $repoRoot 'package.json') -Raw | ConvertFrom-Json
    $packageLockText = Get-Content -LiteralPath (Join-Path $repoRoot 'package-lock.json') -Raw
    $packageLockMatch = [regex]::Match($packageLockText, '(?m)^\s*"version"\s*:\s*"([^"]+)"')
    if (-not $packageLockMatch.Success) {
        throw 'Unable to read root version from package-lock.json'
    }
    $tauriConfig = Get-Content -LiteralPath (Join-Path $repoRoot 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json
    $cargoToml = Get-Content -LiteralPath (Join-Path $repoRoot 'src-tauri\Cargo.toml') -Raw
    $cargoMatch = [regex]::Match($cargoToml, '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"')
    if (-not $cargoMatch.Success) {
        throw 'Unable to read [package].version from src-tauri/Cargo.toml'
    }

    Assert-Version 'package.json' $package.version $Version
    Assert-Version 'package-lock.json' $packageLockMatch.Groups[1].Value $Version
    Assert-Version 'src-tauri/Cargo.toml' $cargoMatch.Groups[1].Value $Version
    Assert-Version 'src-tauri/tauri.conf.json' $tauriConfig.version $Version

    $workingTreeStatus = @(& $git status --porcelain --untracked-files=normal)
    if ($LASTEXITCODE -ne 0) {
        throw 'Unable to inspect the Git working tree.'
    }
    if ($workingTreeStatus.Count -ne 0) {
        throw 'Release builds require a clean Git working tree.'
    }

    $expectedTag = "v$Version"
    $headTags = @(& $git tag --points-at HEAD)
    if ($LASTEXITCODE -ne 0) {
        throw 'Unable to read Git tags for the current commit.'
    }
    if ($headTags -notcontains $expectedTag) {
        throw "A release must be built from the exact Git tag $expectedTag"
    }
    if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_REF_NAME)) {
        Assert-Version 'GitHub tag' $env:GITHUB_REF_NAME $expectedTag
    }

    $defaultKeyDirectory = Join-Path $env:LOCALAPPDATA 'ZHEKARIKSTRIKE\release-keys'
    $secretKeyPath = $configuredSecretKeyPath
    if ([string]::IsNullOrWhiteSpace($secretKeyBase64) -and [string]::IsNullOrWhiteSpace($secretKeyPath)) {
        $defaultSecretKey = Join-Path $defaultKeyDirectory 'updater.key'
        if (Test-Path -LiteralPath $defaultSecretKey -PathType Leaf) {
            $secretKeyPath = $defaultSecretKey
        }
    }
    if ([string]::IsNullOrWhiteSpace($secretKeyBase64) -and
        ([string]::IsNullOrWhiteSpace($secretKeyPath) -or -not (Test-Path -LiteralPath $secretKeyPath -PathType Leaf))) {
        throw 'Set MINISIGN_SECRET_KEY_BASE64 or MINISIGN_SECRET_KEY_PATH to a private key outside Git.'
    }
    if ([string]::IsNullOrWhiteSpace($secretKeyBase64)) {
        $secretKeyPath = (Resolve-Path -LiteralPath $secretKeyPath).Path
        $repoPrefix = $repoRoot.TrimEnd('\') + '\'
        if ($secretKeyPath.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'The minisign private key must not be stored inside the repository.'
        }
    }
    $protectedPasswordPath = Join-Path $defaultKeyDirectory 'updater.password.dpapi'
    if ([string]::IsNullOrWhiteSpace($signingPassword) -and
        -not (Test-Path -LiteralPath $protectedPasswordPath -PathType Leaf)) {
        throw 'MINISIGN_PASSWORD must be set so minisign never prompts during a release.'
    }
    if ($Publish -and [string]::IsNullOrWhiteSpace($githubToken)) {
        throw 'GH_TOKEN is required with -Publish.'
    }
    if ($Publish -and [string]::IsNullOrWhiteSpace($releaseApiToken)) {
        throw 'LAUNCHER_RELEASE_API_TOKEN is required with -Publish.'
    }

    $publicKeyPath = Join-Path $repoRoot 'src-tauri\updater.pub'
    $publicKeyLines = @(Get-Content -LiteralPath $publicKeyPath | Where-Object {
        -not [string]::IsNullOrWhiteSpace($_) -and -not $_.StartsWith('untrusted comment:')
    })
    if ($publicKeyLines.Count -ne 1 -or $publicKeyLines[0] -notmatch '^RWQ[A-Za-z0-9+/=]+$') {
        throw 'src-tauri/updater.pub must contain a complete minisign public key.'
    }
    $publicKeyBase64 = $publicKeyLines[0]

    Invoke-NpmWithoutWorkspaces -Executable $npm -Arguments @('run', 'test:release-script')
    Invoke-NpmWithoutWorkspaces -Executable $npm -Arguments @('ci')
    Invoke-NpmWithoutWorkspaces -Executable $npm -Arguments @('run', 'lint')
    Invoke-NpmWithoutWorkspaces -Executable $npm -Arguments @('run', 'test:unit')
    Invoke-NpmWithoutWorkspaces -Executable $npm -Arguments @('run', 'test:e2e:browser')
    Invoke-NpmWithoutWorkspaces -Executable $npm -Arguments @('run', 'build:frontend')
    Invoke-Native $cargo @('fmt', '--manifest-path', 'src-tauri/Cargo.toml', '--', '--check')
    Invoke-Native $cargo @('clippy', '--manifest-path', 'src-tauri/Cargo.toml', '--all-targets', '--', '-D', 'warnings')
    Invoke-Native $cargo @('test', '--manifest-path', 'src-tauri/Cargo.toml')
    Invoke-NpmWithoutWorkspaces -Executable $npm -Arguments @('run', 'test:e2e:tauri')

    # Debug + WebDriver artifacts are large on Windows and are not release inputs.
    Invoke-Native $cargo @('clean', '--manifest-path', 'src-tauri/Cargo.toml', '--profile', 'dev')

    $targetTriple = 'x86_64-pc-windows-msvc'
    Invoke-NpmWithoutWorkspaces -Executable $npx -Arguments @(
        'tauri', 'build', '--target', $targetTriple, '--bundles', 'nsis'
    )

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
    Copy-Item -LiteralPath $installer.FullName -Destination $installerAsset

    Invoke-NpmWithoutWorkspaces -Executable $npx -Arguments @(
        'tauri', 'build', '--target', $targetTriple, '--no-bundle', '--features', 'portable'
    )
    Invoke-Native 'powershell.exe' @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass',
        '-File', (Join-Path $repoRoot 'scripts\build-portable.ps1'),
        '-Version', $Version,
        '-OutputDirectory', $artifactDirectory,
        '-TargetTriple', $targetTriple
    )

    if (-not [string]::IsNullOrWhiteSpace($secretKeyBase64)) {
        $temporarySecretKey = Join-Path ([System.IO.Path]::GetTempPath()) "zhekarik-minisign-$([guid]::NewGuid().ToString('N')).key"
        [System.IO.File]::WriteAllBytes(
            $temporarySecretKey,
            [Convert]::FromBase64String($secretKeyBase64)
        )
        $secretKeyPath = $temporarySecretKey
    }
    if ([string]::IsNullOrWhiteSpace($signingPassword)) {
        $signingPassword = Read-DpapiProtectedSecret -Path $protectedPasswordPath
    }

    $signaturePath = "$updateAsset.minisig"
    Invoke-MinisignWithPassword $minisign @('-S', '-s', $secretKeyPath, '-m', $updateAsset, '-x', $signaturePath) $signingPassword
    $signatureText = Read-ReleaseTextFile -Path $signaturePath
    Assert-StreamingMinisignSignature -SignatureText $signatureText
    Invoke-Native $minisign (Get-StreamingMinisignVerifyArguments `
        -MessagePath $updateAsset `
        -PublicKey $publicKeyBase64 `
        -SignaturePath $signaturePath)

    $repository = $env:GITHUB_REPOSITORY
    if ([string]::IsNullOrWhiteSpace($repository)) {
        $remoteOutput = @(& $git config --get remote.origin.url)
        $remoteExitCode = $LASTEXITCODE
        $remote = ($remoteOutput | Select-Object -First 1)
        if ($remoteExitCode -ne 0 -or [string]::IsNullOrWhiteSpace($remote) -or
            $remote.Trim() -notmatch 'github\.com[:/]([^/]+/[^/]+?)(?:\.git)?$') {
            throw 'Unable to determine GitHub repository from GITHUB_REPOSITORY or origin.'
        }
        $repository = $Matches[1]
    }
    $expectedRepository = 'd3affy/zhekarikstrike-launcher'
    if ($repository -ne $expectedRepository) {
        throw "Release repository mismatch: expected $expectedRepository, got $repository"
    }

    $sha256 = (Get-FileHash -LiteralPath $updateAsset -Algorithm SHA256).Hash.ToLowerInvariant()
    $signature = Read-ReleaseTextFile -Path $signaturePath
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
        $gh = Get-RequiredCommand 'gh.exe'
        $assets = Get-ChildItem -LiteralPath $artifactDirectory -File | ForEach-Object { $_.FullName }
        $apiBase = $env:LAUNCHER_RELEASE_API_BASE_URL
        if ([string]::IsNullOrWhiteSpace($apiBase)) {
            $apiBase = 'https://api.zhekarik.africa'
        }
        $apiBase = $apiBase.TrimEnd('/')
        $currentManifestUri = "$apiBase/launcher/update/windows/x86_64/$Version"
        $activeManifest = $null
        try {
            $activeManifest = Invoke-RestMethod -Method Get -Uri $currentManifestUri
        } catch {
            $response = $_.Exception.Response
            $statusCode = if ($null -eq $response) { 0 } else { [int]$response.StatusCode }
            if ($statusCode -ne 404) {
                throw
            }
        }

        $env:GH_TOKEN = $githubToken
        try {
            $previousErrorActionPreference = $ErrorActionPreference
            try {
                $ErrorActionPreference = 'SilentlyContinue'
                & $gh release view $expectedTag --repo $repository *> $null
                $releaseExists = $LASTEXITCODE -eq 0
            } finally {
                $ErrorActionPreference = $previousErrorActionPreference
            }

            $manifestObject = $manifestJson | ConvertFrom-Json
            $publicationAction = Resolve-LauncherPublicationAction `
                -CandidateManifest $manifestObject `
                -ActiveManifest $activeManifest `
                -ReleaseExists $releaseExists
            if ($publicationAction -eq 'CreateRelease') {
                Invoke-Native $gh (@('release', 'create', $expectedTag, '--repo', $repository, '--verify-tag', '--title', $expectedTag, '--generate-notes') + $assets)
            } else {
                $existingReleaseDirectory = Join-Path ([IO.Path]::GetTempPath()) "zhekarik-existing-release-$([guid]::NewGuid().ToString('N'))"
                try {
                    New-Item -ItemType Directory -Path $existingReleaseDirectory | Out-Null
                    $manifestAssetName = "launcher-update-${Version}.json"
                    Invoke-Native $gh @(
                        'release', 'download', $expectedTag,
                        '--repo', $repository,
                        '--pattern', $manifestAssetName,
                        '--dir', $existingReleaseDirectory
                    )
                    Invoke-Native $gh @(
                        'release', 'download', $expectedTag,
                        '--repo', $repository,
                        '--pattern', $updateAssetName,
                        '--dir', $existingReleaseDirectory
                    )
                    $existingManifestPath = Join-Path $existingReleaseDirectory $manifestAssetName
                    $existingUpdateAsset = Join-Path $existingReleaseDirectory $updateAssetName
                    $existingManifestJson = Read-ReleaseTextFile -Path $existingManifestPath
                    $existingManifest = $existingManifestJson | ConvertFrom-Json
                    if ([string]$existingManifest.version -ne $Version -or
                        -not (Test-LauncherManifestIdentity -Left $manifestObject -Right $existingManifest)) {
                        throw "GitHub release $expectedTag contains another updater artifact. Publish a new version."
                    }
                    $existingPlatform = Get-LauncherManifestPlatform -Manifest $existingManifest
                    $existingSize = (Get-Item -LiteralPath $existingUpdateAsset).Length
                    if ($existingSize -ne [int64]$existingPlatform.size) {
                        throw "Existing GitHub updater asset size does not match its manifest."
                    }
                    $existingHash = (Get-FileHash -LiteralPath $existingUpdateAsset -Algorithm SHA256).Hash.ToLowerInvariant()
                    if ($existingHash -ne [string]$existingPlatform.sha256) {
                        throw "Existing GitHub updater asset hash does not match its manifest."
                    }
                    $existingSignaturePath = Join-Path $existingReleaseDirectory "$updateAssetName.minisig"
                    Assert-StreamingMinisignSignature -SignatureText ([string]$existingPlatform.signature)
                    [IO.File]::WriteAllText(
                        $existingSignaturePath,
                        [string]$existingPlatform.signature,
                        [Text.UTF8Encoding]::new($false)
                    )
                    Invoke-Native $minisign (Get-StreamingMinisignVerifyArguments `
                        -MessagePath $existingUpdateAsset `
                        -PublicKey $publicKeyBase64 `
                        -SignaturePath $existingSignaturePath)
                    $manifestJson = $existingManifestJson
                } finally {
                    if (Test-Path -LiteralPath $existingReleaseDirectory) {
                        Remove-Item -LiteralPath $existingReleaseDirectory -Recurse -Force
                    }
                }
            }
        } finally {
            Remove-Item Env:GH_TOKEN -ErrorAction SilentlyContinue
        }

        $publishUri = "$apiBase/admin/launcher/releases/windows/x86_64/$Version"
        Invoke-RestMethod -Method Put -Uri $publishUri -Headers @{
            Authorization = "Bearer $releaseApiToken"
        } -ContentType 'application/json' -Body $manifestJson | Out-Null
        Write-Host "Published signed launcher manifest: $publishUri" -ForegroundColor Green
    }

    Write-Host "Signed release bundle: $artifactDirectory" -ForegroundColor Green
} finally {
    if ($null -ne $temporarySecretKey -and (Test-Path -LiteralPath $temporarySecretKey)) {
        Remove-Item -LiteralPath $temporarySecretKey -Force
    }
    $secretKeyBase64 = $null
    $signingPassword = $null
    $githubToken = $null
    $releaseApiToken = $null
    Pop-Location
}
