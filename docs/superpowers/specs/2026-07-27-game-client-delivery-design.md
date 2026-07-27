# Game Client Delivery Design

## Goal

Make the published Tauri launcher install and repair the complete ZHEKARIK STRIKE client from `api.zhekarik.africa`, while keeping the original Google Drive ZIP on the Oracle server and preserving the existing `game_files` / `game_files_pure` launch overlay.

## Constraints

- The 9,153,970,381-byte Drive archive must never be downloaded to the developer PC.
- The original archive must remain on the server after extraction.
- Server deployment inputs come from GitHub, except the game archive itself, which comes directly from the supplied Google Drive file.
- Existing frontend/backend work from other branches is preserved through ordinary merges; no force-push.
- Public game downloads use HTTPS.
- Game archive and file integrity use SHA-256. Launcher updates continue to use the existing minisign contract.
- Game manifests and game files are not cryptographically signed in this phase.
- The production launcher remains one portable EXE and contains no game client payload.

## Chosen Architecture

The existing `zs-updater` repository becomes the asset-publishing tool, not a second public API. Its legacy FastAPI service contains plaintext credentials, MD5-only verification, HTTP origins, and unsafe path construction, so it will not be exposed.

The Oracle host's unused 50 GB `/dev/sdb` volume is mounted at `/srv/zhekarik-game`. It stores:

- `archives/client-02-12.zip`: the unchanged Drive download;
- `releases/1.0.3.4/`: safely extracted client files;
- `manifests/1.0.3.4.json`: deterministic SHA-256 manifest;
- `current`: symlink to the active release;
- `current-manifest.json`: atomically replaced active manifest.

The production backend receives the storage root as a read-only bind mount. It loads and validates the precomputed manifest, streams the immutable ZIP, and serves individual repair files. It never rescans or rehashes the full client during an HTTP request.

## Public Contract

`GET /launcher/game/manifest` returns:

```json
{
  "game_version": "1.0.3.4",
  "generated_at": "RFC3339",
  "files": [
    {
      "path": "RevLoader.exe",
      "size": 123,
      "sha256": "lowercase hex",
      "url": "https://api.zhekarik.africa/launcher/game/files/RevLoader.exe",
      "excluded_from_hash_check": false,
      "temporary": false
    }
  ],
  "archive": {
    "url": "https://api.zhekarik.africa/launcher/game/archive",
    "size": 9153970381,
    "sha256": "lowercase hex",
    "unpacked_size": 123456789
  }
}
```

Additional routes:

- `GET /launcher/game/additional`: manifest containing only the configured additional-check paths.
- `GET /launcher/game/updates?from_version=X`: empty file list when `X` is current, otherwise the full current manifest.
- `GET /launcher/game/excludes`: normalized paths that retain user settings and are not hash-repaired.
- `GET /launcher/game/files/{path}`: one validated regular file below the active release root.
- `GET /launcher/game/archive`: the preserved immutable ZIP.

Every manifest path is forward-slash relative, contains no empty, dot, parent, drive-prefix, or absolute component, and is unique case-insensitively. Archive metadata and every non-excluded file have an exact size and lowercase SHA-256.

## Publisher

The new Python publisher in `zs-updater`:

1. reads the archive without extracting and rejects unsafe entries, duplicate normalized paths, symlink entries, or a release too large for the target filesystem;
2. permits either files at ZIP root or exactly one wrapper directory and strips that wrapper consistently;
3. computes the archive SHA-256;
4. extracts into a unique staging directory without deleting or modifying the archive;
5. verifies the presence of `RevLoader.exe`;
6. computes deterministic per-file SHA-256 records and the total unpacked size;
7. marks configured exclude paths and produces the additional subset;
8. atomically renames staging to the versioned release and atomically replaces the current symlink/manifest.

Publishing is idempotent for the same version and same bytes. A conflicting artifact for an existing version fails without changing `current`.

## Launcher Flow

The Tauri client stops using `http://80.85.247.83`. All game metadata and downloads use `https://api.zhekarik.africa`.

Install:

1. fetch and validate the full manifest;
2. check free space against compressed plus unpacked sizes with safety overhead;
3. download the ZIP into a unique `.part`, validate byte count and SHA-256, then rename;
4. safely extract into the selected directory;
5. delete only the local downloaded ZIP after successful extraction;
6. run a full manifest verification and repair;
7. persist path/version and create shortcuts.

Repair:

- Manual verification checks the full manifest.
- Pre-launch verification checks the additional manifest.
- Game update returns no work for the active version and a full repair manifest for an older/unknown version.
- Missing or corrupt files are downloaded individually through the modern HTTPS file endpoint and hash-verified before replacement.

Launch overlay:

1. sync both patch layers from their existing backend manifest;
2. place `game_files_pure` over the installed client before launch;
3. run `RevLoader.exe` and observe the game process;
4. remove tracked pure files and restore `game_files` after normal exit, failed launch, stop, or launcher shutdown.

## Failure Handling

- Network interruption leaves only a local `.part`; retry never treats it as a valid archive.
- Bad archive size/hash aborts before extraction.
- Unsafe ZIP or manifest paths fail closed.
- Cancellation releases the operation lease and leaves configuration unchanged.
- A failed server publication leaves the prior `current` release active.
- Backend startup/health fails when the manifest, archive, active release, or `RevLoader.exe` is missing or inconsistent.
- Download responses use `Accept-Ranges` where supported by the file server; the first launcher version may restart a partial archive rather than resume it.

## Testing

- Publisher unit tests use small real ZIP fixtures for wrapper stripping, traversal, duplicate/case collision, preserved archive, deterministic SHA-256 manifest, idempotence, and failed-publication rollback.
- Backend tests exercise real manifest loading and file/archive responses, malformed metadata, traversal/symlink rejection, additional/update semantics, and health failure.
- Launcher Rust tests cover modern URL construction, manifest validation, disk-space calculation, size/hash rejection, safe extraction, repair planning, and overlay restore.
- Browser/native Tauri E2E continues to cover install/cancel/verify/launch/error UI.
- The real 9 GB archive is downloaded, hashed, listed, extracted, and fully rehashed only on the Oracle host.
- A local tiny fixture exercises download, install, corruption repair, fake-process launch lifecycle, and overlay replacement without downloading the real client to the developer PC.

## Release and Deployment

After all repositories pass their gates:

1. merge feature commits into `zs-updater/master`, backend `dev`, frontend `agent/restore-react-source`, and launcher `tauri-rework` without force;
2. format/mount the verified blank `/dev/sdb` and persist its UUID in `/etc/fstab`;
3. download the Drive archive directly on the server;
4. run the publisher checked out from GitHub;
5. deploy frontend/backend by full GitHub commit SHA;
6. build, sign, publish, and activate launcher `1.6.3`;
7. verify all public manifests, range/download paths, SHA-256 values, minisign, container health, and disk headroom.
