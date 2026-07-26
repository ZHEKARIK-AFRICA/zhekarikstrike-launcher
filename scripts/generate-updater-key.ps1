param(
    [string]$KeyDirectory = (Join-Path $env:LOCALAPPDATA 'ZHEKARIKSTRIKE\release-keys')
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$minisign = Get-Command minisign.exe -ErrorAction SilentlyContinue
if ($null -eq $minisign) {
    throw 'minisign is required. On Windows: winget install --id jedisct1.minisign --exact --scope user'
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$KeyDirectory = [System.IO.Path]::GetFullPath($KeyDirectory)
$repoPrefix = $repoRoot.TrimEnd('\') + '\'
if ($KeyDirectory.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'The updater private key directory must be outside the repository.'
}

$secretKeyPath = Join-Path $KeyDirectory 'updater.key'
$generatedPublicKeyPath = Join-Path $KeyDirectory 'updater.pub'
$protectedPasswordPath = Join-Path $KeyDirectory 'updater.password.dpapi'
$repositoryPublicKeyPath = Join-Path $repoRoot 'src-tauri\updater.pub'

foreach ($path in @($secretKeyPath, $generatedPublicKeyPath)) {
    if (Test-Path -LiteralPath $path) {
        throw "Refusing to overwrite an existing updater key file: $path"
    }
}

New-Item -ItemType Directory -Path $KeyDirectory -Force | Out-Null

$originalPassword = $env:MINISIGN_PASSWORD
$generatedPassword = [string]::IsNullOrWhiteSpace($originalPassword)
try {
    if ($generatedPassword) {
        # Minisign accepts passwords shorter than its 64-byte input buffer.
        $randomBytes = [byte[]]::new(32)
        $randomNumberGenerator = [Security.Cryptography.RandomNumberGenerator]::Create()
        try {
            $randomNumberGenerator.GetBytes($randomBytes)
        } finally {
            $randomNumberGenerator.Dispose()
        }
        $env:MINISIGN_PASSWORD = [Convert]::ToBase64String($randomBytes)
    }

    $passwordFile = Join-Path ([System.IO.Path]::GetTempPath()) "zhekarik-minisign-password-$([guid]::NewGuid().ToString('N')).tmp"
    try {
        $passwordInput = "$($env:MINISIGN_PASSWORD)`r`n$($env:MINISIGN_PASSWORD)`r`n"
        [System.IO.File]::WriteAllText($passwordFile, $passwordInput, [Text.Encoding]::ASCII)
        $commandLine = "`"$($minisign.Source)`" -G -p `"$generatedPublicKeyPath`" -s `"$secretKeyPath`" < `"$passwordFile`""
        & cmd.exe /d /s /c $commandLine
        if ($LASTEXITCODE -ne 0) {
            throw "minisign key generation failed with exit code $LASTEXITCODE"
        }
    } finally {
        if (Test-Path -LiteralPath $passwordFile) {
            Remove-Item -LiteralPath $passwordFile -Force
        }
    }

    if ($generatedPassword) {
        ConvertTo-SecureString $env:MINISIGN_PASSWORD -AsPlainText -Force |
            ConvertFrom-SecureString |
            Set-Content -LiteralPath $protectedPasswordPath -Encoding ASCII
    }

    $identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetAccessRuleProtection($true, $false)
    $rule = New-Object Security.AccessControl.FileSystemAccessRule(
        $identity,
        [Security.AccessControl.FileSystemRights]::FullControl,
        [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit',
        [Security.AccessControl.PropagationFlags]::None,
        [Security.AccessControl.AccessControlType]::Allow
    )
    $acl.AddAccessRule($rule)
    Set-Acl -LiteralPath $KeyDirectory -AclObject $acl

    Copy-Item -LiteralPath $generatedPublicKeyPath -Destination $repositoryPublicKeyPath -Force
    Write-Host "Private updater key created outside Git: $secretKeyPath" -ForegroundColor Green
    Write-Host "Public updater key copied to: $repositoryPublicKeyPath" -ForegroundColor Green
    if ($generatedPassword) {
        Write-Host "The key password is protected for this Windows account with DPAPI: $protectedPasswordPath"
    }
} finally {
    if ($null -eq $originalPassword) {
        Remove-Item Env:MINISIGN_PASSWORD -ErrorAction SilentlyContinue
    } else {
        $env:MINISIGN_PASSWORD = $originalPassword
    }
}
