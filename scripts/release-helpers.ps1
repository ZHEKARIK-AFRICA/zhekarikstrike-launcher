function Read-DpapiProtectedSecret {
    param([Parameter(Mandatory = $true)][string]$Path)

    $cipherText = (Get-Content -LiteralPath $Path -Raw).Trim()
    $secure = ConvertTo-SecureString $cipherText
    return ([PSCredential]::new('protected-secret', $secure)).GetNetworkCredential().Password
}

function Read-ReleaseTextFile {
    param([Parameter(Mandatory = $true)][string]$Path)

    $resolvedPath = (Resolve-Path -LiteralPath $Path).Path
    return [IO.File]::ReadAllText($resolvedPath)
}

function Get-MinisignSignatureAlgorithm {
    param([Parameter(Mandatory = $true)][string]$SignatureText)

    $lines = @($SignatureText -split "`r?`n")
    if ($lines.Count -lt 2) {
        throw 'Minisign signature is incomplete.'
    }
    try {
        $record = [Convert]::FromBase64String($lines[1])
    } catch {
        throw 'Minisign signature record is not valid base64.'
    }
    if ($record.Length -ne 74) {
        throw 'Minisign signature record must be 74 bytes.'
    }
    return [Text.Encoding]::ASCII.GetString($record, 0, 2)
}

function Assert-StreamingMinisignSignature {
    param([Parameter(Mandatory = $true)][string]$SignatureText)

    $algorithm = Get-MinisignSignatureAlgorithm -SignatureText $SignatureText
    if ($algorithm -cne 'ED') {
        throw "Launcher updates require a streaming ED minisign signature; got $algorithm."
    }
}

function Get-StreamingMinisignVerifyArguments {
    param(
        [Parameter(Mandatory = $true)][string]$MessagePath,
        [Parameter(Mandatory = $true)][string]$PublicKey,
        [Parameter(Mandatory = $true)][string]$SignaturePath
    )

    return @(
        '-V', '-H', '-m', $MessagePath,
        '-P', $PublicKey,
        '-x', $SignaturePath
    )
}

function Invoke-NpmWithoutWorkspaces {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )

    $effectiveArguments = @('--workspaces=false') + $Arguments
    Write-Host "> $Executable $($effectiveArguments -join ' ')" -ForegroundColor Cyan
    & $Executable @effectiveArguments
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code ${LASTEXITCODE}: $Executable $($effectiveArguments -join ' ')"
    }
}

function Get-LauncherVersionParts {
    param([Parameter(Mandatory = $true)][string]$Version)

    if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)$') {
        throw "Launcher version must be X.Y.Z: $Version"
    }
    return @([long]$Matches[1], [long]$Matches[2], [long]$Matches[3])
}

function Compare-LauncherVersions {
    param(
        [Parameter(Mandatory = $true)][string]$Left,
        [Parameter(Mandatory = $true)][string]$Right
    )

    $leftParts = Get-LauncherVersionParts -Version $Left
    $rightParts = Get-LauncherVersionParts -Version $Right
    for ($index = 0; $index -lt 3; $index += 1) {
        if ($leftParts[$index] -lt $rightParts[$index]) { return -1 }
        if ($leftParts[$index] -gt $rightParts[$index]) { return 1 }
    }
    return 0
}

function Get-LauncherManifestPlatform {
    param([Parameter(Mandatory = $true)][object]$Manifest)

    if ($null -eq $Manifest.platforms) {
        throw 'Launcher manifest is missing platforms.'
    }
    $platform = $Manifest.platforms.'windows-x86_64'
    if ($null -eq $platform) {
        throw 'Launcher manifest is missing windows-x86_64 data.'
    }
    foreach ($name in @('url', 'sha256', 'signature', 'size')) {
        if ($null -eq $platform.$name) {
            throw "Launcher manifest platform is missing $name."
        }
    }
    return $platform
}

function Test-LauncherManifestIdentity {
    param(
        [Parameter(Mandatory = $true)][object]$Left,
        [Parameter(Mandatory = $true)][object]$Right
    )

    $leftPlatform = Get-LauncherManifestPlatform -Manifest $Left
    $rightPlatform = Get-LauncherManifestPlatform -Manifest $Right
    return (
        [string]::Equals([string]$Left.version, [string]$Right.version, [StringComparison]::Ordinal) -and
        [string]::Equals([string]$leftPlatform.url, [string]$rightPlatform.url, [StringComparison]::Ordinal) -and
        [string]::Equals([string]$leftPlatform.sha256, [string]$rightPlatform.sha256, [StringComparison]::Ordinal) -and
        [string]::Equals([string]$leftPlatform.signature, [string]$rightPlatform.signature, [StringComparison]::Ordinal) -and
        [int64]$leftPlatform.size -eq [int64]$rightPlatform.size
    )
}

function Resolve-LauncherPublicationAction {
    param(
        [Parameter(Mandatory = $true)][object]$CandidateManifest,
        [object]$ActiveManifest,
        [Parameter(Mandatory = $true)][bool]$ReleaseExists
    )

    Get-LauncherVersionParts -Version ([string]$CandidateManifest.version) | Out-Null
    if ($null -ne $ActiveManifest) {
        $comparison = Compare-LauncherVersions `
            -Left ([string]$CandidateManifest.version) `
            -Right ([string]$ActiveManifest.version)
        if ($comparison -lt 0) {
            throw "Cannot publish launcher $($CandidateManifest.version): active version $($ActiveManifest.version) is newer."
        }
        if ($comparison -eq 0 -and
            -not (Test-LauncherManifestIdentity -Left $CandidateManifest -Right $ActiveManifest)) {
            throw "Launcher version $($CandidateManifest.version) is already active with another artifact. Publish a new version."
        }
    }

    if ($ReleaseExists) {
        return 'VerifyExistingRelease'
    }
    return 'CreateRelease'
}
