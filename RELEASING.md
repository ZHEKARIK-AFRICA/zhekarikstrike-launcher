# Signed Windows releases

`scripts/release.ps1` is the only release entrypoint. It requires an exact
`vX.Y.Z` tag whose version matches `package.json`, `package-lock.json`,
`src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.

## One-time key setup

Install minisign and generate the project key once:

```powershell
winget install --id jedisct1.minisign --exact --scope user
./scripts/generate-updater-key.ps1
```

The private key is stored outside Git in
`%LOCALAPPDATA%\ZHEKARIKSTRIKE\release-keys\updater.key`. Its generated
password is protected for the current Windows account with DPAPI. Only the
public key in `src-tauri/updater.pub` belongs in the repository.

Configure these GitHub Actions secrets before publishing:

- `MINISIGN_SECRET_KEY_BASE64`: base64 of the complete `updater.key` file;
- `MINISIGN_PASSWORD`: the key password;
- `LAUNCHER_RELEASE_API_TOKEN`: bearer token for manifest publication.

## Build and publish

```powershell
./scripts/release.ps1 -Version X.Y.Z
./scripts/release.ps1 -Version X.Y.Z -Publish
```

Without `-Publish`, the signed artifacts remain under `release/X.Y.Z`.
With `-Publish`, the script uploads all artifacts to the matching GitHub
Release and only then calls:

```text
PUT /admin/launcher/releases/windows/x86_64/{version}
Authorization: Bearer <LAUNCHER_RELEASE_API_TOKEN>
Content-Type: application/json
```

The request body is the same signed manifest written into the local bundle.
The API must validate it completely and atomically replace the active public
manifest served by `GET /launcher/update/windows/x86_64/{currentVersion}`.
