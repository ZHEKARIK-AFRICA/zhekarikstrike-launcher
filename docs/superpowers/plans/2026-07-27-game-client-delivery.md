# Game Client Delivery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver the complete game client through production so the one-file Tauri launcher can install, verify, repair, update, launch, and restore patch overlays.

**Architecture:** A GitHub-delivered Python publisher safely extracts the immutable Drive ZIP on a dedicated Oracle volume and precomputes a SHA-256 manifest. The existing FastAPI backend serves the manifest, archive, and repair files from a read-only mount; Tauri consumes only this modern HTTPS contract.

**Tech Stack:** Python 3.12, pytest, FastAPI, Pydantic, Docker Compose, Rust/Tokio/Reqwest/Tauri 2, Vitest, WebDriverIO, PowerShell, GitHub Actions, minisign.

## Global Constraints

- Never download the 9,153,970,381-byte game archive to the developer PC.
- Preserve the original server archive after extraction.
- Use GitHub for code delivery; only the supplied game archive comes directly from Drive.
- Preserve concurrent work through fetch/merge/push; never force-push.
- Use SHA-256 for game archive/files and the existing minisign contract for launcher updates.
- Keep `game_files` and `game_files_pure` external to the launcher and archive.
- Never enable Cargo feature `e2e` in production/release builds.
- Never expose the legacy updater API, plaintext key, MD5-only verification, or HTTP origins.

---

### Task 1: Safe game-client publisher

**Files:**
- Create: `launcher_backend/client_publisher.py`
- Create: `tests/test_client_publisher.py`
- Create: `requirements-dev.txt`
- Modify: `README.md`

**Interfaces:**
- `publish_client(archive: Path, storage_root: Path, version: str, excludes: list[str], additional: list[str]) -> dict`
- CLI: `python -m launcher_backend.client_publisher --archive ... --storage-root ... --version 1.0.3.4 --exclude-file ... --additional-file ...`

- [ ] Write real-ZIP tests for wrapper stripping, SHA-256, archive preservation, deterministic order, required `RevLoader.exe`, traversal/absolute/duplicate-case/symlink rejection, idempotence, and rollback.
- [ ] Run `python -m pytest tests/test_client_publisher.py -q`; confirm RED because the module is missing.
- [ ] Implement chunked hashing, safe inspection/extraction, unique staging, versioned release, relative current symlink, and atomic manifest activation. Never alter the archive.
- [ ] Run `python -m pytest -q`; require GREEN.
- [ ] Commit as `feat: publish immutable game client releases`.

### Task 2: Backend game-client store and API

**Files:**
- Create: `app/launcher_game_client.py`
- Create: `tests/test_launcher_game_client.py`
- Modify: `app/models.py`, `app/config.py`, `app/main.py`, `tests/test_api_contracts.py`, `.env.example`

**Interfaces:**
- Consume `/data/game-client/current-manifest.json`, `archives/<name>`, and `current/<path>`.
- Produce `manifest()`, `additional_manifest()`, `updates_from(version)`, `resolve_file(path)`, and `archive_path()`.
- Produce GET routes `/launcher/game/manifest`, `additional`, `updates`, `excludes`, `files/{path}`, and `archive`.

- [ ] Write store tests with literal JSON for current/old versions, additional subset, malformed metadata, traversal, symlink escape, bad hashes, and missing archive.
- [ ] Run focused pytest; confirm RED on missing module.
- [ ] Implement a bounded, strict, read-only store that requires `RevLoader.exe`.
- [ ] Write API tests for responses, cache headers, traversal, missing storage, and health readiness.
- [ ] Run API tests; confirm RED on 404 routes.
- [ ] Register routes and `LAUNCHER_GAME_CLIENT_ROOT=/data/game-client`; use `FileResponse` for large files.
- [ ] Run full backend pytest; require all non-DB tests green.
- [ ] Commit as `feat: serve complete launcher game client`.

### Task 3: Production mount and health gates

**Files:**
- Modify: `deploy/compose.production.yml`, `deploy/env.example`, `deploy/scripts/preflight.sh`, `deploy/scripts/verify.sh`
- Modify: `tests/site-contract.test.mjs`, `tests/data-platform-deploy.test.mjs`

**Interfaces:**
- Bind host path (default `/srv/zhekarik-game`) to backend `/data/game-client:ro`.
- Deployment verification requires manifest, archive metadata, and one bounded file probe.

- [ ] Write failing deployment tests for the read-only bind, required host path, and public probes.
- [ ] Run `npm --workspaces=false test`; confirm RED.
- [ ] Add compose/env/preflight/verify wiring without changing database volumes.
- [ ] Run full frontend tests, `lint:data-platform`, production build, and `audit --omit=dev`.
- [ ] Commit as `feat: deploy complete game client storage`.

### Task 4: Tauri modern game API

**Files:**
- Modify: `src-tauri/src/constants.rs`, `models/manifest.rs`, `services/api_client.rs`, `services/manifest_service.rs`

**Interfaces:**
- `GameArchiveManifest` gains required `unpacked_size: u64`.
- `ApiClient` uses only the six modern `/launcher/game/*` routes.
- No runtime request uses `80.85.247.83`.

- [ ] Write failing tests for exact HTTPS URLs, strict archive fields, unsafe/duplicate paths, and update behavior.
- [ ] Run focused Cargo tests; confirm expected failures.
- [ ] Remove compatibility maps and fallback archive metadata; add strict manifest validation.
- [ ] Run full Cargo tests and clippy `-D warnings`.
- [ ] Commit as `feat: consume modern game client API`.

### Task 5: Verified install, repair, and overlay lifecycle

**Files:**
- Modify: `services/install_service.rs`, `archive_service.rs`, `verify_service.rs`, `download_service.rs`, and `game_process_service.rs` only if a cleanup gap is proven.

**Interfaces:**
- `required_install_bytes(archive_size, unpacked_size) -> Result<u64, AppError>` uses checked arithmetic and overhead.
- Download must match manifest size and SHA-256 before extraction.
- All launch terminal paths restore regular overlays.

- [ ] Write failing tiny-file/ZIP tests for space overflow, short download, hash mismatch, cancellation, traversal, and failed-extraction archive retention.
- [ ] Run focused tests; confirm RED for missing behavior.
- [ ] Implement checked space, exact byte/hash validation, safe extraction, full repair, and config update only after success.
- [ ] Write failing overlay tests for success, timeout, launch error, stop, and shutdown.
- [ ] Implement only proven cleanup gaps and run full Cargo tests.
- [ ] Run launcher lint, Vitest, browser E2E, build, fmt, clippy, Rust tests, and Windows Tauri E2E.
- [ ] Commit as `feat: verify complete game installation lifecycle`.

### Task 6: Provision and publish the real client

**Server state:**
- Exact device: `/dev/sdb`
- Mount: `/srv/zhekarik-game`
- Archive: `/srv/zhekarik-game/archives/client-02-12.zip`
- Drive ID: `1fIOIJwCWCXKSC1UKxvKIaxzVHIs1KXQ8`

- [ ] Reconfirm `/dev/sdb` is blank/unmounted with `lsblk -f`, `wipefs -n`, and `findmnt`.
- [ ] Create ext4 filesystem, mount by UUID, persist in `/etc/fstab`, and verify writable capacity.
- [ ] Push updater code and fetch it on the server from GitHub; do not use SCP.
- [ ] Download on Oracle with server-side `gdown` into `.part`; require exact 9,153,970,381 bytes before rename.
- [ ] Inspect ZIP safety and uncompressed size before extraction; require 5 GB post-publication headroom.
- [ ] Publish `1.0.3.4` and independently hash every extracted file against the manifest.
- [ ] Record archive SHA-256/size, unpacked size/file count, active symlink, `RevLoader.exe`, and free space.

### Task 7: Merge, deploy, and publish launcher 1.6.3

**Files:**
- Modify launcher version fields in `package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, `Cargo.lock`, and `tauri.conf.json`.

- [ ] Fetch target/feature branches and merge any newer remote work; rerun affected tests.
- [ ] Merge into updater `master`, backend `dev`, frontend `agent/restore-react-source`, and launcher `tauri-rework` without force.
- [ ] Deploy frontend/backend by full GitHub SHA through `git-release.sh`.
- [ ] Add a failing version-consistency test, then bump launcher to 1.6.3 and create annotated `v1.6.3`.
- [ ] Run `scripts/release.ps1 -Version 1.6.3`; require signed portable EXE and NSIS fallback.
- [ ] Push tag, publish through the existing GitHub/admin API pipeline, and verify the active updater manifest.

### Task 8: End-to-end verification

- [ ] From an independent client, validate manifest/additional/exclude/update responses, sample repairs, and archive range/metadata.
- [ ] On Oracle, rehash the preserved ZIP and every extracted file against the active manifest.
- [ ] Locally run a tiny-fixture download/install/corruption-repair/fake-launch/overlay-restore lifecycle without the real client.
- [ ] Download public 1.6.3 EXE; verify size, SHA-256, minisign, stable redirect, and absence of `e2e`.
- [ ] Confirm target branches clean/zero-divergence, active server SHAs, healthy containers/DB, persistent `/dev/sdb`, and safe free space.
