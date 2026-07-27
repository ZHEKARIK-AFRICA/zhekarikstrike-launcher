# Remote Game Patches and Single Portable Launcher Design

## Goal

Ship one Windows EXE that is sufficient to start the launcher while moving the
`game_files` and `game_files_pure` payloads out of the launcher bundle. The
launcher downloads those payloads from the website backend on first use and
checks their server-provided SHA-256 hashes before every game launch.

## Repository scope

- Launcher: `d3affy/zhekarikstrike-launcher`, branch `tauri-rework`.
- Backend: `d3affy/zhekarik_site_backend`, branch `dev`.
- Frontend/deployment: `d3affy/zhekarik-site-react`, branch
  `agent/restore-react-source`.
- Existing Electron source and the source copies under launcher `public/` stay
  available as rollback/reference material, but Tauri release artifacts do not
  bundle or copy those directories.

## Backend contract

The backend image owns the deployed patch payload under
`/app/launcher-assets/game_files` and
`/app/launcher-assets/game_files_pure`. The source payload is versioned in the
backend repository so deployment remains Git-only.

`GET /launcher/game-files/manifest` returns:

```json
{
  "files": [
    {
      "layer": "game_files",
      "path": "csgo/pak01_dir.vpk",
      "size": 6502226,
      "sha256": "lowercase hex"
    }
  ]
}
```

The manifest is generated from the files actually present in the backend
image. Entries are ordered by layer and normalized relative path. Only the two
known layers are exposed. SHA-256 is authoritative for the simplified trust
model approved for this release.

`GET /launcher/game-files/{layer}/{file_path:path}` streams a file only when:

- `layer` is exactly `game_files` or `game_files_pure`;
- the resolved file remains below the configured layer root;
- the target exists and is a regular file.

Traversal and symlink escapes return `404`. Manifest responses use
`Cache-Control: no-store`; download responses do not rely on client-side HTTP
caching for correctness.

## Launcher cache and launch lifecycle

The launcher stores remote patch files below:

```text
%LOCALAPPDATA%\ZHEKARIKSTRIKE\game-file-cache\
  game_files\...
  game_files_pure\...
```

Before every real game launch, the launcher:

1. fetches the current backend manifest;
2. validates every layer, relative path, size, and SHA-256 value;
3. hashes each corresponding local cache file;
4. downloads missing, wrong-sized, or hash-mismatched files to `.part` files;
5. verifies the downloaded size and SHA-256 before rename;
6. removes cache files that are not present in the current manifest;
7. only then copies `game_files_pure` into the selected game directory and
   starts `RevLoader.exe`.

The first launch therefore populates the cache. Later launches normally only
read the small manifest and hash the five local patch files. A backend or
download failure blocks the game launch rather than using an unverified or
incomplete cache.

When the game exits or the launcher closes, the existing cleanup lifecycle
deletes the tracked pure files and restores files from the cached
`game_files` layer. Cleanup skips restoration only when the cache has never
been initialized.

The existing main-game installation archive remains external and is still
downloaded into the directory chosen by the user. It is not part of the
portable launcher EXE.

## Release layout

The canonical release artifact
`ZHEKARIK-STRIKE_X.Y.Z_windows-x86_64.exe` is built with Cargo feature
`portable`. It is simultaneously:

- the file downloaded from the website;
- the signed launcher updater artifact;
- the executable that moves itself to the selected game directory using the
  existing portable-move behavior.

The NSIS installer remains in GitHub Releases as a fallback, but the website
continues to expose only the backend stable download route. The portable ZIP is
removed. Tauri `bundle.resources` no longer contains `game_files` or
`game_files_pure`, so the canonical EXE is self-contained apart from the
runtime downloads described above.

The existing signed updater manifest format and pinned minisign public key do
not change. Release publication still uploads all GitHub assets before the
backend atomically activates the manifest. Consequently the stable website
route continues to redirect to the same canonical signed EXE.

## Deployment and compatibility

The backend Dockerfile copies `launcher-assets` into the immutable image. The
production compose file explicitly sets
`LAUNCHER_GAME_FILES_PATH=/app/launcher-assets`, and deployment verification
requires the game-file manifest endpoint to return `200` in addition to the
existing health and launcher-update checks.

Frontend application code needs no new download URL: both visible download
links already use
`https://api.zhekarik.africa/launcher/download/windows/x86_64`.

The deployment must be performed from commits pushed to GitHub. No patch asset
is copied directly to the Oracle host outside the Git checkout/deployment
process.

## Tests and acceptance

- Backend tests cover deterministic manifests, exact hashes/sizes, successful
  downloads, unknown layers, traversal, symlink escape, and an empty/missing
  asset root.
- Rust tests cover manifest validation, cache path safety, first download,
  unchanged cache, corrupted-file repair, unexpected-file removal, and absent
  layers.
- Release-script tests prove that the canonical EXE comes from the portable
  build, no portable ZIP is produced, and Tauri has no bundled patch resources.
- Existing frontend, backend, Rust, browser E2E, Windows Tauri E2E, NSIS, and
  portable release gates remain required.

