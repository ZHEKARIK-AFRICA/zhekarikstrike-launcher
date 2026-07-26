function Read-DpapiProtectedSecret {
    param([Parameter(Mandatory = $true)][string]$Path)

    $cipherText = (Get-Content -LiteralPath $Path -Raw).Trim()
    $secure = ConvertTo-SecureString $cipherText
    return ([PSCredential]::new('protected-secret', $secure)).GetNetworkCredential().Password
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
