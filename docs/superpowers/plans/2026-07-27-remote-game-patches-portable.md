# Remote Game Patches and Single Portable Launcher Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver launcher v1.6.2 as one portable EXE and make it download, verify, cache, apply, and restore `game_files` and `game_files_pure` through the production backend.

**Architecture:** FastAPI exposes a deterministic SHA-256 manifest and traversal-safe file downloads for two Git-versioned patch layers. Rust synchronizes those layers into `%LOCALAPPDATA%\ZHEKARIKSTRIKE\game-file-cache` before every game launch and existing copy/cleanup code consumes the cache. The release pipeline signs the portable build as the canonical updater/site artifact and retains NSIS only as a release fallback.

**Tech Stack:** Rust 2021, Tokio, Reqwest, Tauri 2, FastAPI, Pydantic 2, pytest, PowerShell 5, GitHub Actions, Docker Compose.

## Global Constraints

- Launcher work is on `tauri-rework`; backend work is on `dev`; deployment/frontend work is on `agent/restore-react-source`.
- Fetch and fast-forward before each repository's first commit; never force-push or overwrite unrelated remote changes.
- Deployment is Git-only. Do not SCP patch assets or source archives to the Oracle host.
- The public download remains `GET https://api.zhekarik.africa/launcher/download/windows/x86_64`.
- The canonical `ZHEKARIK-STRIKE_1.6.2_windows-x86_64.exe` is built with feature `portable` and remains the signed updater artifact.
- The main game archive remains an external installation download and is not embedded in the launcher.
- Release builds must not contain Cargo feature `e2e`.
- Every behavior change follows red-green-refactor and every repository is clean before publishing.

---

### Task 1: Backend patch manifest and safe file service

**Files:**
- Create: backend `app/launcher_game_files.py`
- Create: backend `tests/test_launcher_game_files.py`
- Modify: backend `app/models.py`
- Modify: backend `app/config.py`

**Interfaces:**
- Produces: `LauncherGameFile(layer: Literal["game_files", "game_files_pure"], path: str, size: int, sha256: str)`.
- Produces: `LauncherGameFilesManifest(files: list[LauncherGameFile])`.
- Produces: `LauncherGameFileStore(root: Path)`.
- Produces: `LauncherGameFileStore.manifest() -> LauncherGameFilesManifest`.
- Produces: `LauncherGameFileStore.resolve_download(layer: str, relative_path: str) -> Path`.

- [ ] **Step 1: Write failing store tests**

Create fixtures with one file in each layer and assert that `manifest()` returns
sorted normalized paths, exact byte sizes, and lowercase SHA-256. Assert that
unknown layers, `..`, absolute paths, missing files, directories, and a symlink
escaping the root raise `FileNotFoundError`.

```python
def test_manifest_hashes_both_layers_in_stable_order(tmp_path: Path):
    (tmp_path / "game_files/csgo").mkdir(parents=True)
    (tmp_path / "game_files_pure/csgo").mkdir(parents=True)
    (tmp_path / "game_files/csgo/base.bin").write_bytes(b"base")
    (tmp_path / "game_files_pure/csgo/pure.bin").write_bytes(b"pure")

    manifest = LauncherGameFileStore(tmp_path).manifest()

    assert [(item.layer, item.path) for item in manifest.files] == [
        ("game_files", "csgo/base.bin"),
        ("game_files_pure", "csgo/pure.bin"),
    ]
```

- [ ] **Step 2: Verify the tests fail for the missing service**

Run: `.\.venv\Scripts\python -m pytest tests/test_launcher_game_files.py -q`

Expected: collection fails because `app.launcher_game_files` does not exist.

- [ ] **Step 3: Implement the strict models and store**

Use `hashlib.sha256`, `Path.rglob`, `Path.resolve`, and `Path.relative_to`.
Reject any layer outside the two literal values and any resolved target outside
its layer root. Do not follow a file symlink outside the configured root.

Add `launcher_game_files_path: Path = Path("/app/launcher-assets")` to
`Settings`.

- [ ] **Step 4: Run focused backend tests**

Run: `.\.venv\Scripts\python -m pytest tests/test_launcher_game_files.py -q`

Expected: all tests pass.

- [ ] **Step 5: Commit the backend service**

```powershell
git add app/config.py app/models.py app/launcher_game_files.py tests/test_launcher_game_files.py
git commit -m "feat: add launcher game file store"
```

### Task 2: Public backend patch API and deployable assets

**Files:**
- Modify: backend `app/main.py`
- Modify: backend `tests/test_api_contracts.py`
- Modify: backend `Dockerfile`
- Modify: backend `.env.example`
- Modify: backend `README.md`
- Create: backend `launcher-assets/game_files/csgo/pak01_dir.vpk`
- Create: backend `launcher-assets/game_files/csgo/scripts/items/items_game.txt`
- Create: backend `launcher-assets/game_files_pure/!test.txt`
- Create: backend `launcher-assets/game_files_pure/csgo/pak01_dir.vpk`
- Create: backend `launcher-assets/game_files_pure/csgo/scripts/items/items_game.txt`

**Interfaces:**
- Consumes: `LauncherGameFileStore` from Task 1.
- Produces: `GET /launcher/game-files/manifest`.
- Produces: `GET /launcher/game-files/{layer}/{file_path:path}`.

- [ ] **Step 1: Write failing API contract tests**

Extend `make_client` to populate a temporary patch root and inject
`launcher_game_files_path`. Assert the manifest response body and
`Cache-Control: no-store`; assert exact file bytes from the download endpoint;
assert `404` for an unknown layer, traversal encoding, a directory, and a
missing file.

- [ ] **Step 2: Verify the endpoints fail before registration**

Run: `.\.venv\Scripts\python -m pytest tests/test_api_contracts.py -q`

Expected: new endpoint tests receive `404`.

- [ ] **Step 3: Register the store and routes**

Initialize one `LauncherGameFileStore` in `create_app`, attach it to
`app.state`, return the Pydantic manifest with `Cache-Control: no-store`, and
return `FileResponse` only for `resolve_download` results. Translate
`FileNotFoundError` to `HTTP 404`.

- [ ] **Step 4: Version the five existing patch files in backend Git**

Copy the exact bytes from launcher `public/game_files` and
`public/game_files_pure` into backend `launcher-assets`. Add
`COPY launcher-assets ./launcher-assets` to the backend image before changing
to the unprivileged user.

- [ ] **Step 5: Run backend tests and image build**

```powershell
.\.venv\Scripts\python -m pytest -q
docker build -t zhekarik-site-backend:game-files-test .
```

Expected: tests and Docker build pass.

- [ ] **Step 6: Commit backend API and assets**

```powershell
git add app/main.py tests/test_api_contracts.py Dockerfile .env.example README.md launcher-assets
git commit -m "feat: serve launcher game patches"
```

### Task 3: Production compose and health contract

**Files:**
- Modify: frontend `deploy/compose.production.yml`
- Modify: frontend `deploy/scripts/verify.sh`
- Modify: frontend `deploy/env.example`
- Modify: frontend `tests/site-contract.test.mjs`
- Modify: frontend `docs/runbooks/operations.md`

**Interfaces:**
- Consumes: backend manifest endpoint from Task 2.
- Produces: backend environment value
  `LAUNCHER_GAME_FILES_PATH=/app/launcher-assets`.
- Produces: production verification that requires manifest HTTP `200`.

- [ ] **Step 1: Add failing deployment contract assertions**

Assert the compose environment contains the exact path and `verify.sh` probes
`/launcher/game-files/manifest`, accepting only `200` for that probe.

- [ ] **Step 2: Verify the frontend contract test fails**

Run: `npm test`

Expected: the new environment and probe assertions fail.

- [ ] **Step 3: Update compose, verification, example env, and runbook**

Keep the existing stable launcher download constant unchanged. Update the
success message so it names the game-file API alongside frontend, backend, and
launcher release API.

- [ ] **Step 4: Run frontend gates**

```powershell
npm ci
npm test
npm run lint
npm run build
```

Expected: all commands pass.

- [ ] **Step 5: Commit deployment contract**

```powershell
git add deploy tests/site-contract.test.mjs docs/runbooks/operations.md
git commit -m "feat: deploy launcher game patches"
```

### Task 4: Launcher patch manifest validation and API client

**Files:**
- Modify: launcher `src-tauri/src/models/manifest.rs`
- Modify: launcher `src-tauri/src/services/api_client.rs`
- Modify: launcher `src-tauri/src/services/mod.rs`
- Create: launcher `src-tauri/src/services/game_patch_service.rs`

**Interfaces:**
- Produces: `GamePatchLayer::{GameFiles, GameFilesPure}` with strict serde names.
- Produces: `GamePatchManifest { files: Vec<GamePatchManifestEntry> }`.
- Produces: `ApiClient::get_game_patch_manifest() -> Result<GamePatchManifest, AppError>`.
- Produces: `ApiClient::game_patch_download_url(&GamePatchManifestEntry) -> Result<String, AppError>`.
- Produces: `validate_manifest(&GamePatchManifest) -> Result<(), AppError>`.

- [ ] **Step 1: Write failing Rust tests for manifest validation**

Tests must reject unknown layers, empty/absolute/traversing paths, invalid
lowercase SHA-256, duplicate `(layer, path)` entries, and incomplete layer
sets. A valid manifest with an empty file (`size == 0`) must pass.

- [ ] **Step 2: Verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml game_patch_service -- --nocapture`

Expected: compilation fails because the service and models do not exist.

- [ ] **Step 3: Implement models, validation, and fixed-origin URLs**

Construct download URLs only from `MODERN_API_BASE_URL`, the validated layer,
and percent-encoded path segments. Do not accept an arbitrary download URL from
the manifest.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml game_patch_service -- --nocapture`

Expected: focused tests pass.

### Task 5: Launcher first-download and per-launch cache synchronization

**Files:**
- Modify: launcher `src-tauri/src/services/game_patch_service.rs`
- Modify: launcher `src-tauri/src/services/config_service.rs`
- Modify: launcher `src-tauri/src/services/download_service.rs`

**Interfaces:**
- Produces: `GamePatchRoots { game_files: PathBuf, game_files_pure: PathBuf }`.
- Produces: `game_patch_roots() -> Result<GamePatchRoots, AppError>`.
- Produces: `sync_game_patch_cache(app, cancel, event_name, operation_id) -> Result<GamePatchRoots, AppError>`.
- Uses existing `.part` download and SHA-256 verification behavior.

- [ ] **Step 1: Write failing cache planning tests**

Using `tempfile`, cover missing first-run files, exact unchanged files,
wrong-sized files, same-sized corrupt files, and unexpected files. The planner
must return download tasks only for missing/corrupt files and prune only files
inside the two cache roots.

- [ ] **Step 2: Verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml game_patch_service -- --nocapture`

Expected: tests fail because cache planning/synchronization is missing.

- [ ] **Step 3: Implement cache synchronization**

Resolve the root from `config_service::get_config_dir()`, create both layer
directories, fetch and validate the manifest, hash local files, download needed
entries with the existing concurrency limit, verify sizes as well as hashes,
remove unexpected regular files, and return roots only after a complete pass.

- [ ] **Step 4: Verify focused and complete Rust tests**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml game_patch_service -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: all tests pass.

- [ ] **Step 5: Commit manifest and cache implementation**

```powershell
git add src-tauri/src/models/manifest.rs src-tauri/src/services src-tauri/src/services/config_service.rs
git commit -m "feat: cache remote game patches"
```

### Task 6: Enforce patch synchronization in the game lifecycle

**Files:**
- Modify: launcher `src-tauri/src/services/game_process_service.rs`
- Modify: launcher `src-tauri/src/services/shutdown_service.rs`
- Modify: launcher `src-tauri/src/services/file_patch_service.rs`
- Modify: launcher `src-tauri/src/commands/game_commands.rs`
- Modify: launcher Windows E2E fixtures only if their mocked launch contract needs the new progress stage

**Interfaces:**
- Consumes: `sync_game_patch_cache` and `GamePatchRoots` from Task 5.
- Preserves: `launch_game` Tauri command name and zero-argument frontend call.
- Preserves: `game-started`, `game-closed`, and `verify-progress` payload contracts.

- [ ] **Step 1: Write failing lifecycle unit tests**

Extract a small source-selection helper and test that launch always selects
cached `game_files_pure`, cleanup selects cached `game_files`, and cleanup does
nothing when the cache has never initialized. The production change that makes
these tests pass must remove all `resource_path` lookup from patch lifecycle.

- [ ] **Step 2: Verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml file_patch_service -- --nocapture`

Expected: the cache-root lifecycle helpers do not exist.

- [ ] **Step 3: Synchronize immediately before applying pure files**

Call `sync_game_patch_cache` before `game-starting` and before deleting or
copying tracked patch files. Use `verify-progress` so the existing main page
shows activity. If synchronization fails, return the structured command error
and do not spawn the game.

- [ ] **Step 4: Restore from the verified cache during cleanup**

Remove `bundled_game_files` and `bundled_game_files_pure`. Cleanup must check
that the cache layer exists before walking it, retain the existing cancellation
and tracked-file semantics, and never recreate patch data from launcher
resources.

- [ ] **Step 5: Run Rust and frontend flow tests**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml
npm run test:unit
npm run test:e2e:browser
```

Expected: all commands pass.

- [ ] **Step 6: Commit lifecycle integration**

```powershell
git add src-tauri/src/services src-tauri/src/commands tests src
git commit -m "feat: verify patches before game launch"
```

### Task 7: One portable EXE release and v1.6.2

**Files:**
- Modify: launcher `src-tauri/tauri.conf.json`
- Modify: launcher `scripts/build-portable.ps1`
- Modify: launcher `scripts/release.ps1`
- Modify: launcher `tests/release-publication.ps1`
- Modify: launcher `package.json`
- Modify: launcher `package-lock.json`
- Modify: launcher `src-tauri/Cargo.toml`
- Modify: launcher `src-tauri/Cargo.lock`
- Modify: launcher `RELEASING.md`

**Interfaces:**
- Produces: canonical portable and updater asset
  `ZHEKARIK-STRIKE_1.6.2_windows-x86_64.exe`.
- Produces: NSIS fallback
  `ZHEKARIK-STRIKE_1.6.2_windows-x86_64-setup.exe`.
- Removes: `*-portable.zip` release artifact.
- Preserves: existing signed launcher manifest JSON contract.

- [ ] **Step 1: Add failing release source assertions**

Assert that `tauri.conf.json` has no `game_files` resources, that
`build-portable.ps1` creates the canonical `.exe` without `Compress-Archive`,
and that `release.ps1` copies the updater asset only after the portable-feature
build. Assert no release script copies the two patch directories.

- [ ] **Step 2: Verify RED**

Run: `npm run test:release-script`

Expected: source assertions fail against the ZIP/resource implementation.

- [ ] **Step 3: Change the release build order**

Build and retain NSIS first. Then build `--no-bundle --features portable`, copy
that raw EXE to the canonical updater name, sign it, generate the unchanged
manifest, and upload both EXEs plus signature/manifest. Remove ZIP staging and
all resource copies.

- [ ] **Step 4: Bump all launcher versions to 1.6.2**

Update package, lockfile, Cargo, Cargo lock root package, and Tauri config.
Confirm the API client user-agent derives or states `1.6.2` consistently.

- [ ] **Step 5: Run release-focused gates**

```powershell
npm run test:release-script
npm run build:frontend
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: all commands pass with no warnings.

- [ ] **Step 6: Commit release layout and version**

```powershell
git add package.json package-lock.json src-tauri scripts tests RELEASING.md
git commit -m "release: prepare portable launcher 1.6.2"
```

### Task 8: Full verification, review, publication, and Git-only deployment

**Files:**
- No planned production code; fix only defects proven by a failing gate or review.

**Interfaces:**
- Produces: pushed backend `dev`, frontend `agent/restore-react-source`, and launcher `tauri-rework` commits.
- Produces: signed GitHub release/tag `v1.6.2` and active backend updater manifest.
- Produces: production backend serving patch manifest/files and website serving the canonical EXE.

- [ ] **Step 1: Fetch and reconcile remote work without force**

For each repository, run `git fetch --prune`, inspect
`git rev-list --left-right --count HEAD...@{u}`, and rebase only local commits
onto the current upstream. Resolve conflicts by preserving upstream behavior
and reapplying only this feature.

- [ ] **Step 2: Run complete backend and frontend gates**

```powershell
# backend
.\.venv\Scripts\python -m pytest -q
docker build -t zhekarik-site-backend:game-files-release .

# frontend
npm ci
npm test
npm run lint
npm run build
```

- [ ] **Step 3: Run complete launcher gates**

```powershell
npm ci
npm run lint
npm run test:unit
npm run test:e2e:browser
npm run build:frontend
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
npm run test:e2e:tauri
```

- [ ] **Step 4: Build release artifacts locally**

After committing and tagging the exact clean launcher commit, run
`scripts/release.ps1 -Version 1.6.2` with the existing local minisign key. Verify
the output contains the canonical EXE, its minisig, NSIS fallback, and manifest,
and contains no portable ZIP or external patch directory.

- [ ] **Step 5: Review the complete diffs and fix findings with tests first**

Inspect each upstream diff and run a focused security review of manifest path
handling, file replacement boundaries, download hashes, and release build
ordering. Re-run every affected gate after fixes.

- [ ] **Step 6: Push branches and publish v1.6.2**

Push each target branch normally. Push annotated tag `v1.6.2` only after all
branch pushes succeed. Let the GitHub Action invoke the same release script with
`-Publish`; verify the Release assets and active backend manifest.

- [ ] **Step 7: Deploy from Git commits on the Oracle Windows server**

Connect over the configured SSH key. Update the server's Git checkouts to the
exact pushed frontend/backend commits and invoke the repository deployment
scripts there. Do not transfer local source or patch files.

- [ ] **Step 8: Verify production end to end**

Check:

```text
GET https://api.zhekarik.africa/health/ready
GET https://api.zhekarik.africa/launcher/game-files/manifest
GET https://api.zhekarik.africa/launcher/download/windows/x86_64
GET https://api.zhekarik.africa/launcher/update/windows/x86_64/1.6.1
```

Download the canonical EXE, compare it with the GitHub Release SHA-256, launch
it in a clean test profile, verify first-run patch downloads, deliberately
corrupt one cached file, and verify the next game-launch attempt repairs it.

