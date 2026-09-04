# Google Drive Pack Delivery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Выпустить launcher `1.6.16`, который устанавливает и ремонтирует game content `1.0.3.6` из 64 MiB Google Drive packs минимум вдвое быстрее зафиксированного v2 baseline, без длинной повторной проверки 19.6 GB при commit.

**Architecture:** Publisher детерминированно объединяет существующие независимые zstd frames в 136 content-addressed packs и публикует три Drive replica каждого pack. Backend выдаёт один самодостаточный fail-closed v3 manifest. Launcher планирует full-pack или coalesced Range-загрузки, адаптивно управляет двумя-шестью HTTP/2 workers и потоково передаёт проверенные файлы единственному transactional commit coordinator. Единственный runtime fallback — уже активный v1 ZIP, и он выбирается только по буквальному HTTP `404` от v3 endpoint.

**Tech Stack:** Python 3.12, `rfc8785==0.1.4`, rclone 1.75.0, FastAPI/Pydantic, Rust/Tauri 2, reqwest 0.12 with native TLS ALPN, Tokio, zstd, `serde_json_canonicalizer==0.3.2`, Vitest/WebDriverIO, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-02-google-drive-pack-delivery-design.md`

## Global Constraints

- Google Drive is the only v3 pack store. Do not add Oracle, CDN, loose-object, or v2 transport fallback.
- Runtime discovery is exactly `GET /launcher/game/v3/manifest`; only literal `404` selects v1 ZIP. Network errors, timeouts, `503`, malformed manifests, and exhausted pack replicas fail visibly.
- Pack maximum is exactly `67_108_864` bytes. Current content must produce exactly `136` packs from `7_191` unique v2 chunk objects and `8_757_634_312` compressed bytes.
- Preserve `content_sha256` by reconstructing the exact legacy schema-2 transport-neutral projection. Use RFC 8785 only for v3 `manifest_sha256`.
- Keep v1 active and immutable throughout. V2 may remain temporarily available to released old launchers during E2E, but launcher `1.6.16` must never request it.
- Do not force-push. Start every code branch from a freshly fetched target and merge fresh Seva changes normally before the complete local gate.
- Server source arrives only from GitHub at a recorded full commit SHA. Do not copy source files directly to NY.
- Never print, commit, or upload the rclone config, OAuth JSON, launcher signing key, or API tokens.
- On NY, build and upload one pack at a time. Maintain at least 5 GiB free and never materialize another full client copy.
- Do not delete any v2 pointers or blobs before the controlled E2E succeeds. Deletion commands must enumerate exact manifest-owned files and require their named confirmation guard.
- Do not remove or overwrite the user's existing installation or `D:\zhekarik-e2e-*` directories without first resolving the exact disposable target and proving it is not the configured user game path.
- Testing is stage-batched: one combined Python/deployment checkpoint, one focused Rust checkpoint, one complete local release gate, one real cold-cache E2E, and one tagged CI gate. Do not run tests after individual files, functions, or small commits.
- New-test budget is two publisher test functions, two backend test functions, and five Rust test functions. Add no tests for getters, individual serde fields, status text, logs, or thin delegation.

---

## Task 1: Establish Fresh Isolated Branches and Freeze the Shared Contract

**Files:**

- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\docs\superpowers\specs\2026-09-02-google-drive-pack-delivery-design.md`
- Create: `D:\projects\bots\SITE_SERVER\launcher_project\.worktrees\drive-pack-v3-1.0.3.6\tests\fixtures\content-v3-canonicalization.json`
- Create: `D:\projects\bots\zhekarik_site_react\worktrees\zhekarik_site_backend-drive-pack-v3-api\tests\fixtures\content-v3-canonicalization.json`
- Create: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\tests\fixtures\content-v3-canonicalization.json`

- [ ] In `D:\projects\bots\SITE_SERVER\launcher_project`, fetch GitHub and verify `origin/master` contains `12f3a0437f7beb6bb45320e6c1e8fb95b1ec45a8` before creating the publisher worktree.

```powershell
git fetch origin --prune
git merge-base --is-ancestor 12f3a0437f7beb6bb45320e6c1e8fb95b1ec45a8 origin/master
git worktree add .worktrees/drive-pack-v3-1.0.3.6 -b codex/drive-pack-v3-1.0.3.6 origin/master
```

- [ ] In the clean backend repository, fetch `origin/dev`, create `codex/drive-pack-v3-api`, and preserve the already-reviewed drive-only changes from `cf66c247` with a normal merge only when those changes are not ancestors of fresh `origin/dev`.

```powershell
Set-Location D:\projects\bots\zhekarik_site_react\zhekarik_site_backend
git fetch origin --prune
git worktree add ..\worktrees\zhekarik_site_backend-drive-pack-v3-api -b codex/drive-pack-v3-api origin/dev
Set-Location ..\worktrees\zhekarik_site_backend-drive-pack-v3-api
git merge-base --is-ancestor cf66c247 origin/dev
```

If the last command exits nonzero, resolve the full local object with `git rev-parse cf66c247`, inspect its diff, and merge that exact commit with `git merge --no-ff cf66c247`. Do not use the dirty backend checkout on `master`.

- [ ] Fetch the deployment repository and create `codex/drive-pack-v3-deploy` from fresh `origin/agent/restore-react-source`.

```powershell
Set-Location D:\projects\bots\zhekarik_site_react\zhekarik-site-react
git fetch origin --prune
git worktree add .worktrees\drive-pack-v3-deploy -b codex/drive-pack-v3-deploy origin/agent/restore-react-source
```

- [ ] In the existing launcher worktree, fetch `origin/tauri-rework`, inspect divergence, and merge remote changes normally before implementation if the target moved. Preserve the approved design commits.

```powershell
Set-Location D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14
git fetch origin --prune
git log --left-right --oneline HEAD...origin/tauri-rework
git merge --no-ff origin/tauri-rework
```

- [ ] Freeze one byte-identical fixture in all three code repositories. It must contain these two cases:

  - JCS input `{"z":"Жекарик","a":[3,{"b":true,"a":"é"}]}`, canonical UTF-8 base64 `eyJhIjpbMyx7ImEiOiLDqSIsImIiOnRydWV9XSwieiI6ItCW0LXQutCw0YDQuNC6In0=`, SHA-256 `a9eb440d609d1c200afdd060ccb1f7f8ed950cbe430ca507f34be14aea785a30`;
  - a complete legacy schema-2 projection using `release_id=1.0.3.6-r1`, one two-byte file `путь/é.bin`, one three-byte compressed chunk, and expected canonical SHA-256 `aff45b7ff48dbc87f7ae9a40c508e3b8284307a6431d60a709d7d31f94c51298`.

- [ ] Define the legacy projection as the exact existing `INTERNAL_FIELDS` document: `schema_version`, `release_id`, `game_version`, `generated_at`, `source_archive_sha256`, `chunking`, `compression`, `download_size`, `unpacked_size`, `chunks`, and `files`. Each chunk keeps only `uncompressed_size`, `compressed_size`, and `compressed_sha256`. Canonical bytes are compact recursively sorted UTF-8 JSON plus one trailing LF.

- [ ] Pin Python `rfc8785==0.1.4` with hashes in the publisher dependency lock and add the same exact version to backend production/test dependencies. Do not change the existing v2 `_canonical_bytes()` helper.

- [ ] Commit contract/fixture work in each repository, but do not run tests at this stage.

---

## Task 2: Build the Deterministic 64 MiB Pack Model

**Files:**

- Create: `D:\projects\bots\SITE_SERVER\launcher_project\.worktrees\drive-pack-v3-1.0.3.6\launcher_backend\content_v3.py`
- Modify: `D:\projects\bots\SITE_SERVER\launcher_project\.worktrees\drive-pack-v3-1.0.3.6\requirements-publisher.txt`
- Create: `D:\projects\bots\SITE_SERVER\launcher_project\.worktrees\drive-pack-v3-1.0.3.6\tests\test_drive_pack_publisher.py`

- [ ] Add the pure pack-domain API without network calls:

```python
PACK_MAX_SIZE = 64 * 1024 * 1024
PACK_REPLICA_COUNT = 3
PACK_PROFILE = "drive-pack-v1"

@dataclass(frozen=True)
class PackSpan:
    raw_sha256: str
    compressed_sha256: str
    compressed_size: int
    offset: int

@dataclass(frozen=True)
class PackPlan:
    ordinal: int
    size: int
    spans: tuple[PackSpan, ...]
```

Expose `ContentV3Error` and the exact functions `canonical_manifest_identity(document: dict[str, Any]) -> tuple[bytes, str]`, `legacy_content_projection(manifest: Mapping[str, Any]) -> dict[str, Any]`, `legacy_content_identity(manifest: Mapping[str, Any]) -> str`, `ordered_unique_chunks(v2_manifest: Mapping[str, Any]) -> Iterator[str]`, `build_pack_layout(v2_manifest: Mapping[str, Any], *, max_pack_size: int = PACK_MAX_SIZE) -> list[PackPlan]`, and `validate_v3_manifest(manifest: Mapping[str, Any], *, expected_v1: Mapping[str, Any]) -> dict[str, Any]`.

- [ ] Traverse v2 `files` and each file's raw chunk list in order. Add a raw chunk only at its first occurrence. Close the current pack before an append that would exceed 64 MiB. Reject a source object larger than 64 MiB rather than creating an invalid pack.

- [ ] Build the v3 document with all existing file records, a chunk map extended by `pack_sha256` and `offset`, a pack map, and `pack_profile`. Preserve `generated_at` so the legacy projection hashes to the already-published `content_sha256`.

- [ ] Validate safe case-insensitive paths, file/chunk closure, pack/chunk closure, contiguous offsets, checked additions, exact totals, three distinct Drive IDs, v1 version/source/files agreement, legacy `content_sha256`, and JCS `manifest_sha256` with the latter field omitted.

- [ ] Add exactly one table-driven test function named `test_v3_pack_layout_is_deterministic_and_preserves_content`. Cover first-use ordering, deduplication, an injected small boundary, contiguous spans, a zero-byte file, byte-for-byte zstd preservation, both canonical fixture cases, and the exact current-release count assertion through a manifest-only fixture.

- [ ] Do not execute the test yet. Commit the completed pure pack model as one publisher batch.

---

## Task 3: Implement Resumable Three-Replica Drive Publication

**Files:**

- Create: `D:\projects\bots\SITE_SERVER\launcher_project\.worktrees\drive-pack-v3-1.0.3.6\launcher_backend\drive_pack_publisher.py`
- Modify: `D:\projects\bots\SITE_SERVER\launcher_project\.worktrees\drive-pack-v3-1.0.3.6\launcher_backend\release_pipeline.py`
- Modify: `D:\projects\bots\SITE_SERVER\launcher_project\.worktrees\drive-pack-v3-1.0.3.6\tests\test_drive_pack_publisher.py`
- Modify: `D:\projects\bots\SITE_SERVER\launcher_project\.worktrees\drive-pack-v3-1.0.3.6\README.md`

- [ ] Add the publication API and CLI:

```python
@dataclass(frozen=True)
class PackPublisherConfig:
    storage_root: Path
    content_sha256: str
    rclone_bin: Path
    rclone_config: Path
    drive_root: str
    minimum_free_bytes: int = 5 * 1024 * 1024 * 1024
```

Expose `prepare_drive_packs(config: PackPublisherConfig) -> dict[str, Any]`, `verify_drive_packs(storage_root: Path, game_version: str, *, rclone_bin: Path, rclone_config: Path, drive_root: str) -> dict[str, Any]`, `activate_drive_packs(storage_root: Path, game_version: str) -> dict[str, Any]`, `deactivate_drive_packs(storage_root: Path, game_version: str) -> dict[str, Any]`, `deactivate_v2_transports(storage_root: Path, game_version: str) -> dict[str, Any]`, `prune_local_v2_after_v3(storage_root: Path, game_version: str, *, confirm_v3_repair_verified: bool) -> dict[str, Any]`, `prune_drive_v2_after_release_cycle(storage_root: Path, game_version: str, *, rclone_bin: Path, rclone_config: Path, drive_root: str, confirm_release_cycle_complete: bool) -> dict[str, Any]`, and `main(argv: Sequence[str] | None = None) -> int`.

- [ ] Reuse only the audited primitives from `content_publisher.py`: `_load_prepared_manifest(storage_root, content_sha256, verify_blobs=False)`, `_content_root`, `_publisher_lock`, `_run_rclone`, `_atomic_bytes`, `_safe_path`, `_require_sha256`, `deactivate_content`, and `deactivate_drive_mirror`. Keep old v2 publisher and mirror behavior byte-compatible.

- [ ] Store manifest/pointer/state under:

```text
/srv/zhekarik-game/content-v3/manifests/<manifest_sha256>.json
/srv/zhekarik-game/content-v3/current.json
/srv/zhekarik-game/content-v3/staging/
/srv/zhekarik-game/content-v3/disabled/
/srv/zhekarik-game/publications/1.0.3.6/v3-state.json
```

The pointer is exactly `{"schema_version":1,"release_id":"1.0.3.6-r1","manifest":"manifests/<manifest_sha256>.json","sha256":"<stored-file-sha256>"}` after canonical serialization. Its path stem and body field carry the RFC 8785 identity calculated with `manifest_sha256` omitted; pointer `sha256` separately authenticates the complete stored JSON bytes including that field.

- [ ] For each pack, write one temporary local file while checking every source `.zst` size and SHA-256 and calculating pack SHA-256 plus MD5. Flush and fsync before upload. Never have two complete local pack temporaries at once.

- [ ] Persist an attempt record before uploading. Upload three immutable objects to exact paths `content-v3/<content_sha256>/packs/<pack_sha256>/replica-{1,2,3}.pack` with separate `rclone copyto --immutable --drive-chunk-size 64M` calls.

- [ ] Read remote `ID`, `Size`, and checksum metadata through `rclone lsjson --files-only --hash`; require three distinct valid IDs, exact size, unique paths, no conflict, and exact MD5 whenever Drive returns complete trustworthy MD5 metadata. Missing/untrustworthy MD5 is allowed only because the mandatory anonymous full SHA-256 stream independently verifies the entire replica.

- [ ] Grant and read back anonymous access through rclone without emitting config contents. Probe exact `https://drive.usercontent.google.com/download?id=<id>&export=download&confirm=t` with redirects disabled, `Accept-Encoding: identity`, `Range: bytes=0-0`, exact `206`, and correct full size. Then stream every replica fully and compare its SHA-256 with the pack identity.

- [ ] Atomically mark a pack verified in `v3-state.json`, delete its temporary local file, and continue. On failure remove only replica objects first created by the current attempt. Preserve previously verified packs for idempotent resume.

- [ ] `activate` must rerun local contract closure, require all 136 packs and 408 verified replica objects, and atomically replace only `content-v3/current.json`. `deactivate` atomically moves the pointer into `disabled/` without deleting data.

- [ ] `deactivate-v2` must call the existing tracked v2 content and mirror deactivation implementations, remove content first and mirror second, archive both exact pointers recoverably, and leave v1 and v3 untouched.

- [ ] `prune-local-v2` must require active exact v3, completed v3 verification, absent v2 content and mirror pointers, accessible v1 pointer/ZIP, and `--confirm-v3-repair-verified`. Delete only `.zst` paths enumerated by the source v2 manifest.

- [ ] `prune-drive-v2` must additionally require `--confirm-release-cycle-complete` and delete exact loose paths via a temporary `--files-from-raw`; never purge a broad Drive directory.

- [ ] Add these CLI forms and document their order:

```text
python -m launcher_backend.drive_pack_publisher prepare --storage-root /srv/zhekarik-game --content-sha256 01a13dfb3448ce6c55ec2051d70ad61775cbe1c2fa322330542d3b879d9675db --rclone-bin /usr/local/bin/rclone --rclone-config /var/lib/zhekarik-strike/launcher-publisher/config/rclone.conf --drive-root "zhekarik-launcher-drive:Zhekarik Strike Launcher Releases"
python -m launcher_backend.drive_pack_publisher verify --storage-root /srv/zhekarik-game --game-version 1.0.3.6 --rclone-bin /usr/local/bin/rclone --rclone-config /var/lib/zhekarik-strike/launcher-publisher/config/rclone.conf --drive-root "zhekarik-launcher-drive:Zhekarik Strike Launcher Releases"
python -m launcher_backend.drive_pack_publisher activate --storage-root /srv/zhekarik-game --game-version 1.0.3.6
python -m launcher_backend.drive_pack_publisher deactivate --storage-root /srv/zhekarik-game --game-version 1.0.3.6
python -m launcher_backend.drive_pack_publisher deactivate-v2 --storage-root /srv/zhekarik-game --game-version 1.0.3.6
python -m launcher_backend.drive_pack_publisher prune-local-v2 --storage-root /srv/zhekarik-game --game-version 1.0.3.6 --confirm-v3-repair-verified
python -m launcher_backend.drive_pack_publisher prune-drive-v2 --storage-root /srv/zhekarik-game --game-version 1.0.3.6 --rclone-bin /usr/local/bin/rclone --rclone-config /var/lib/zhekarik-strike/launcher-publisher/config/rclone.conf --drive-root "zhekarik-launcher-drive:Zhekarik Strike Launcher Releases" --confirm-release-cycle-complete
```

- [ ] Add exactly one more table-driven publisher test function named `test_v3_drive_publication_lifecycle_is_fail_closed`. Cover replica metadata, anonymous range/full stream checks, interrupted resume, conflicts, prepare-without-activation, atomic activate/deactivate, one-pack disk bound, cleanup ownership, and both prune guards.

- [ ] Do not execute tests yet. Commit publisher network/state/CLI changes as one batch and push the feature branch for review.

---

## Task 4: Add the Fail-Closed Backend V3 Contract

**Files:**

- Modify: `D:\projects\bots\zhekarik_site_react\worktrees\zhekarik_site_backend-drive-pack-v3-api\app\models.py`
- Create: `D:\projects\bots\zhekarik_site_react\worktrees\zhekarik_site_backend-drive-pack-v3-api\app\launcher_content_v3.py`
- Modify: `D:\projects\bots\zhekarik_site_react\worktrees\zhekarik_site_backend-drive-pack-v3-api\app\main.py`
- Modify: `D:\projects\bots\zhekarik_site_react\worktrees\zhekarik_site_backend-drive-pack-v3-api\app\config.py`
- Modify: `D:\projects\bots\zhekarik_site_react\worktrees\zhekarik_site_backend-drive-pack-v3-api\requirements.txt`
- Create: `D:\projects\bots\zhekarik_site_react\worktrees\zhekarik_site_backend-drive-pack-v3-api\tests\test_launcher_content_v3.py`
- Modify: `D:\projects\bots\zhekarik_site_react\worktrees\zhekarik_site_backend-drive-pack-v3-api\tests\test_api_contracts.py`

- [ ] Add strict Pydantic v3 models with `extra="forbid"`: `LauncherContentV3Manifest`, `DrivePackProfile`, `DrivePackRecord`, `PackedChunkRecord`, and the existing-compatible file record. Enforce lowercase SHA-256, exactly three distinct Drive IDs, exact profile constants, and `0 < pack.size <= 67_108_864`.

- [ ] Implement a store with this public surface:

```python
class LauncherContentV3Store:
    storage_root: Path
```

Expose `PackContentNotActive`, `PackContentInvalid`, `LauncherContentV3Store.__init__(storage_root: Path)`, `LauncherContentV3Store.load_active() -> tuple[dict[str, Any], str]`, and `LauncherContentV3Store.validate_if_active() -> None`.

- [ ] Treat only a genuinely absent `content-v3/current.json` as not active. Reject a symlink, broken symlink, directory, duplicate JSON key, malformed pointer, path escape, symlinked manifest, wrong exact-file SHA, path stem/body `manifest_sha256` mismatch, malformed body, or failed closure as invalid active state.

- [ ] Validate `manifest_sha256` with pinned RFC 8785 after removing that top-level member. Independently build the exact schema-2 legacy projection and require its legacy canonical SHA-256 to match `content_sha256` without reading a v2 pointer or endpoint.

- [ ] Validate current v1 through authoritative `current-manifest.json`: same `game_version`, source archive SHA, files and flags, additional-check semantics, unpacked total, and `RevLoader.exe`. Do not require the compatibility `current` symlink and do not consult v2.

- [ ] Register `GET /launcher/game/v3/manifest`. Return the immutable manifest body on `200`; absent pointer maps to `404`; any active-state defect maps to `503`. Add current ETag plus `Cache-Control: no-cache` to all three outcomes so activation and rollback are immediately revalidated.

- [ ] Add v3 validation to `/health/ready`: absent pointer is healthy/dormant, valid active pointer is healthy, invalid active pointer fails readiness.

- [ ] Change the default launcher repository in `app/config.py` from the old personal owner to `ZHEKARIK-AFRICA/zhekarikstrike-launcher`.

- [ ] Add exactly one table-driven `test_launcher_content_v3.py` test function covering valid data plus JCS/content/v1/path/span/total/replica mutations.

- [ ] Add exactly one v3 route matrix to the existing API contract test: `404`, `200`, `503`, ETag/no-cache, unchanged v1/v2 payloads, v2 pointer removal, and v3 remaining valid.

- [ ] Do not run backend tests yet. Commit and push backend v3 as one batch.

---

## Task 5: Update Deployment and Run the Single Python/Deployment Checkpoint

**Files:**

- Modify: `D:\projects\bots\zhekarik_site_react\zhekarik-site-react\.worktrees\drive-pack-v3-deploy\deploy\scripts\verify.sh`
- Modify: `D:\projects\bots\zhekarik_site_react\zhekarik-site-react\.worktrees\drive-pack-v3-deploy\deploy\scripts\preflight.sh`
- Modify: `D:\projects\bots\zhekarik_site_react\zhekarik-site-react\.worktrees\drive-pack-v3-deploy\deploy\scripts\git-release.sh`
- Create: `D:\projects\bots\zhekarik_site_react\zhekarik-site-react\.worktrees\drive-pack-v3-deploy\deploy\scripts\publisher-release.sh`
- Modify: `D:\projects\bots\zhekarik_site_react\zhekarik-site-react\.worktrees\drive-pack-v3-deploy\deploy\compose.production.yml`
- Modify: `D:\projects\bots\zhekarik_site_react\zhekarik-site-react\.worktrees\drive-pack-v3-deploy\tests\data-platform-deploy.test.mjs`
- Modify: `D:\projects\bots\zhekarik_site_react\zhekarik-site-react\.worktrees\drive-pack-v3-deploy\tests\deploy-scripts.test.mjs`

- [ ] Keep the existing read-only `/srv/zhekarik-game:/data/game-client` mount and generic API proxy. Add no secret, OAuth file, or Google API call to the backend container.

- [ ] Make deployment verification accept only `200` or `404` for both v2 and v3 endpoints and reject `503`; keep v1 manifest/archive/loader requirements strict at `200/206`.

- [ ] Make preflight read the release version from authoritative `current-manifest.json` and verify `releases/<version>/RevLoader.exe`. Treat the `current` symlink as optional compatibility state.

- [ ] Switch tracked GitHub references in `git-release.sh`, `compose.production.yml`, and deployment tests from `d3affy` to `ZHEKARIK-AFRICA`, paired with the backend default changed in Task 4.

- [ ] Add `publisher-release.sh <publisher-40-char-sha>`. It must fetch `git@github-zhekarik-publisher:ZHEKARIK-AFRICA/zs-updater.git` through `/opt/zhekarik-africa/.ssh/config`, verify `${sha}^{commit}` in `/opt/zhekarik-africa/git/zs-updater.git`, archive that exact object into a mode-700 staging directory, write `.publisher-sha`, atomically rename to `/opt/zhekarik-africa/publisher-releases/<sha>`, create `.venv` from root `requirements-publisher.txt`, and verify the recorded SHA. Existing matching immutable checkouts are reused; conflicting ones fail closed.

- [ ] Fold assertions into the two existing deployment test files instead of creating more test files.

- [ ] Run the first implementation checkpoint only after Tasks 2–5 are complete:

```powershell
$ErrorActionPreference = 'Stop'
Set-Location D:\projects\bots\SITE_SERVER\launcher_project\.worktrees\drive-pack-v3-1.0.3.6
python -m pytest tests/test_drive_pack_publisher.py tests/test_content_publisher.py tests/test_release_pipeline.py
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Set-Location D:\projects\bots\zhekarik_site_react\worktrees\zhekarik_site_backend-drive-pack-v3-api
python -m pytest tests/test_launcher_content_v3.py tests/test_api_contracts.py
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Set-Location D:\projects\bots\zhekarik_site_react\zhekarik-site-react\.worktrees\drive-pack-v3-deploy
npm ci
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
npm test
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

- [ ] If a scope fails, fix the whole related group and rerun only that failed command. Record counts and final exit codes in the branch handoff.

- [ ] Commit deployment changes and push all three non-launcher feature branches. Open ordinary PRs or merge normally; never force-push.

---

## Task 6: Deploy the Backend Dormant by Exact GitHub SHA

**Files:**

- Server application checkout convention: `/opt/zhekarik-africa/releases/${FRONTEND_SHA:0:7}-${BACKEND_SHA:0:7}/`, containing the exact frontend/deployment and backend commits
- Server publisher checkout convention: `/opt/zhekarik-africa/publisher-releases/$PUBLISHER_SHA/`, where `PUBLISHER_SHA` is the recorded 40-character GitHub commit from this task
- Existing game storage: `/srv/zhekarik-game`

- [ ] Resolve and record full GitHub commit SHAs after push. In the NY root shell set `FRONTEND_SHA`, `BACKEND_SHA`, and `PUBLISHER_SHA` to those recorded 40-character values without abbreviating them, then validate and deploy exactly:

```bash
set -eu
for sha in "$FRONTEND_SHA" "$BACKEND_SHA" "$PUBLISHER_SHA"; do
  printf '%s\n' "$sha" | grep -Eq '^[0-9a-f]{40}$'
done
/opt/zhekarik-africa/current/frontend/deploy/scripts/git-release.sh "$FRONTEND_SHA" "$BACKEND_SHA"
release_id="$(printf '%.7s-%.7s' "$FRONTEND_SHA" "$BACKEND_SHA")"
test "$(cat "/opt/zhekarik-africa/releases/$release_id/.frontend-sha")" = "$FRONTEND_SHA"
test "$(cat "/opt/zhekarik-africa/releases/$release_id/.backend-sha")" = "$BACKEND_SHA"
test "$(readlink -f /opt/zhekarik-africa/current)" = "/opt/zhekarik-africa/releases/$release_id"
/opt/zhekarik-africa/current/frontend/deploy/scripts/publisher-release.sh "$PUBLISHER_SHA"
test "$(cat "/opt/zhekarik-africa/publisher-releases/$PUBLISHER_SHA/.publisher-sha")" = "$PUBLISHER_SHA"
```

The two tracked scripts perform the GitHub fetch through repository-specific deploy keys, verify each exact commit object, materialize immutable checkouts, and fail before switching production on any mismatch.

- [ ] Deploy the backend before creating `content-v3/current.json`. Verify production frontend is unchanged, v1 returns `200`, v1 archive Range returns `206`, v3 returns `404`, and readiness remains healthy.

- [ ] Verify publisher runtime permissions without exposing secrets:

```bash
chown root:root /usr/local/bin/rclone
chmod 0755 /usr/local/bin/rclone
chown zs-launcher-publisher:zs-launcher-publisher /var/lib/zhekarik-strike/launcher-publisher/config/rclone.conf
chmod 0600 /var/lib/zhekarik-strike/launcher-publisher/config/rclone.conf
sudo -u zs-launcher-publisher test -r /var/lib/zhekarik-strike/launcher-publisher/config/rclone.conf
```

- [ ] Create the publisher venv inside the immutable publisher release directory from its hash-pinned requirements. Run no publication command as root; use `zs-launcher-publisher` and the exact rclone/config paths.

- [ ] Confirm `content-v3/current.json` is absent and save a read-only snapshot of current v1/v2 pointer identities before continuing.

---

## Task 7: Add Launcher V3 Models, Content Inventory, and Discovery

**Files:**

- Create: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\models\content_pack.rs`
- Create: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\models\content_inventory.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\models\mod.rs`
- Create: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\services\content_inventory_service.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\services\api_client.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\services\mod.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\Cargo.toml`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\Cargo.lock`

- [ ] Add strict Rust types mirroring the public schema:

```rust
pub struct DrivePackManifest {
    pub schema_version: u8,
    pub manifest_sha256: String,
    pub content_sha256: String,
    pub release_id: String,
    pub game_version: String,
    pub generated_at: String,
    pub source_archive_sha256: String,
    pub download_size: u64,
    pub unpacked_size: u64,
    pub chunking: ContentChunking,
    pub compression: ContentCompression,
    pub pack_profile: DrivePackProfile,
    pub packs: BTreeMap<String, DrivePack>,
    pub chunks: BTreeMap<String, PackedContentChunk>,
    pub files: Vec<ContentFile>,
}
pub struct DrivePackProfile { pub name: String, pub max_pack_size: u64, pub replica_count: u8 }
pub struct DrivePack { pub size: u64, pub replica_file_ids: Vec<String> }
pub struct PackedContentChunk {
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub compressed_sha256: String,
    pub pack_sha256: String,
    pub offset: u64,
}

impl DrivePackManifest {
    pub fn validate(&self) -> Result<(), AppError>;
    pub fn legacy_content_projection(&self) -> Result<serde_json::Value, AppError>;
    pub fn drive_url(file_id: &str) -> Result<Url, AppError>;
}
```

- [ ] Add transport-neutral `ContentInventory` schema 1 with `from_v3` and `from_v2`. Store it atomically at `.zhekarik/content/inventories/<content_sha256>.json`; validate on load. Migrate a persisted v2 manifest locally and leave the original file untouched.

- [ ] Verify `manifest_sha256` with `serde_json_canonicalizer = "0.3.2"`. Verify `content_sha256` through a separate recursive key-sort/compact UTF-8/trailing-LF legacy serializer matching the shared fixture.

- [ ] Change reqwest features to include `native-tls-alpn` while retaining `stream` and `json`. Log the negotiated HTTP version per pack attempt, without URLs containing file IDs.

- [ ] Add `ApiClient::get_compatible_pack_manifest() -> Result<Option<DrivePackManifest>, AppError>`. `Ok(None)` is produced only by literal HTTP `404`; every other HTTP/network/body/validation failure is `Err`. Do not call the v2 endpoint from this method or any new runtime path.

- [ ] Keep canonical v1-version comparison fail-closed when v3 is active. A v1 metadata failure is not permission to choose ZIP.

- [ ] Do not run Rust tests yet. Commit models/inventory/discovery as one launcher batch.

---

## Task 8: Implement Pack Planning, Verified Cache, HTTP/2 Download, and Adaptive Control

**Files:**

- Create: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\services\content_pack_plan_service.rs`
- Create: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\services\content_pack_cache_service.rs`
- Create: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\services\content_pack_controller.rs`
- Create: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\services\content_pack_download_service.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\services\mod.rs`

- [ ] Define the planner interface and inclusive range semantics:

```rust
pub enum PackTransferMode { Full, Ranges(Vec<ByteRange>) }
pub struct ByteRange { pub start: u64, pub end_inclusive: u64 }
pub struct PackFetchPlan { pub pack_sha256: String, pub mode: PackTransferMode, pub required_chunks: Vec<String> }

pub fn plan_pack_fetches(manifest: &DrivePackManifest, required_raw_chunks_in_first_use_order: &[String]) -> Result<Vec<PackFetchPlan>, AppError>;
```

- [ ] Select every required pack in full for a fresh installation. For update/repair, select a full pack when unique required spans contain at least 25% of pack bytes; otherwise sort spans and coalesce only when gap is at most 64 KiB and combined inclusive range is at most 16 MiB. Emit explicit verified empty-file artifacts outside the chunk plan.

- [ ] Implement content-addressed full-pack/range cache, atomic verified promotion, transaction-owned exclusive claim files, and `.part` rules. Preserve only verified entries and cleanly interrupted full-pack partials; discard incomplete ranges and structurally/hash-invalid partials.

- [ ] Implement exact Drive requests with `Accept-Encoding: identity`, redirects disabled, exact final URL, and bounded header/body timeouts. A range requires exact `206`, single exact `Content-Range`, exact `Content-Length`, no content encoding, and exact EOF. Accept `200` only for a fresh full pack.

- [ ] Resume a full pack by checking local length, feeding its existing prefix into the continuing SHA-256 state, and requesting the inclusive suffix from any valid replica. If local length equals pack size, verify/promote without network; if larger, delete it.

- [ ] Verify full pack size/SHA before publication to materializers. Verify every compressed chunk SHA before exposing a cached range. Rotate immediately on `403`, `404`, bad range/size/hash; bound retryable `408`, `429`, `5xx`, header timeout, and idle timeout to two attempts per replica with 1s/2s backoff and capped `Retry-After`.

- [ ] Implement the controller as a pure decision core driven by a real independent two-second Tokio interval:

```rust
pub enum PackSource { GoogleDrive }
pub struct AttemptProgress {
    pub source: PackSource,
    pub pack_sha256: String,
    pub replica_index: usize,
    pub current_offset: u64,
    pub useful_bytes: u64,
    pub header_latency: Option<Duration>,
    pub last_progress_at: Instant,
}
pub struct ControllerSample {
    pub useful_bytes: u64,
    pub ready_backlog_bytes: u64, // verified compressed bytes not yet consumed by materializers
    pub pressure: PressureWindow,
    pub active_attempts: Vec<AttemptProgress>,
}
pub struct AdaptivePackController {
    target: usize,
    maximum: usize,
    ewma_bytes_per_second: Option<f64>,
    accepted_baselines: BTreeMap<usize, f64>,
    trial: Option<ControllerTrial>,
    cooldown_until: Option<Instant>,
    pressure_events: VecDeque<PressureEvent>,
    sample_ticks: u8,
    sample_useful_bytes: u64,
}
impl AdaptivePackController {
    pub fn observe(&mut self, now: Instant, sample: ControllerSample) -> ControllerDecision;
}
```

- [ ] Start at 2 workers, cap at 6, use EWMA alpha 0.30, require at least three ticks and 64 MiB useful bytes per judgment, trial `+1` only below 256 MiB backlog, retain it only for at least 5% gain, and use 20s cooldown. Halve on `429` or three timeout/5xx pressure events within 30s.

- [ ] Keep user cancellation distinct from per-attempt adaptive preemption. Lowering concurrency stops admission immediately; rotate a 20s outlier only when another active transfer advances at least 512 KiB/s. A child token must never cancel the installation lease.

- [ ] Select each pack's initial replica with a stable hash of `operation_id` and `pack_sha256` so traffic is distributed. Once a replica has a permanent protocol/integrity failure, disable it for the rest of that operation; retryable failures still obey the two-attempt bound before rotation.

- [ ] Emit live unique-useful-byte events, excluding retries/gaps/duplicates, and ensure progress remains monotonic.

- [ ] Do not run tests yet. Commit planner/cache/downloader/controller as one launcher batch.

---

## Task 9: Stream Materialization into Transactional Commit and Integrate Every Runtime Path

**Files:**

- Create: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\services\content_commit_service.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\services\content_install_service.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\services\content_journal_service.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\services\content_download_service.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\services\install_service.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\commands\verify_commands.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\commands\utility_commands.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\services\prerequisite_service.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\app.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src\renderer\renderer_index.js`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src\renderer\renderer_install.js`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src\localization\locales\ru.json`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src\localization\locales\en.json`
- Create: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\drive_pack_tests.rs`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\src\lib.rs`

- [ ] Introduce the coordinator contract:

```rust
pub struct VerifiedArtifact {
    pub relative_path: PathBuf,
    pub temporary_path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

pub async fn run_streaming_commit(
    context: CommitContext,
    artifacts: mpsc::Receiver<VerifiedArtifact>,
    cancellation: CancellationToken,
) -> Result<ContentCompletionState, AppError>;
```

- [ ] Before journal activation, calculate disk preflight with checked arithmetic as missing pack/range bytes + managed backups + bounded staging + 2 GiB. Reserve staged bytes and cap concurrent staging at `max(largest_required_file, 1 GiB)`. A reservation failure must leave no active journal.

- [ ] Prevent local-source reuse races with one chosen mechanism: before journal activation, open each reused installed-file source once using Windows share flags `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`, wrap it in `Arc<File>`, clone handles for consumers, and retain the registry until every dependent materializer is drained. A lazy `PathBuf + offset` is not a safe source once streaming commit begins.

- [ ] Before any final-path mutation, validate targets and current managed identities, write the complete replace/remove journal, fsync it, and durably switch once to streaming commit phase.

- [ ] Materialize directly from verified pack spans while checking compressed SHA, bounded zstd output, raw SHA, whole-file size/SHA, flush, and fsync in one pass. Send the closed verified artifact through a bounded channel. The coordinator must not hash the new file again.

- [ ] Immediately before replacing an original managed target, recapture its full identity. If it differs from the snapshot, durably amend and fsync the journal before moving the original to backup. Then atomically rename the verified artifact. Only one coordinator mutates final paths.

- [ ] On error/cancel stop admission, cancel and drain every worker, reverse the journal, remove only exact transaction-introduced targets, preserve unknown files, and retain verified cache plus clean full-pack partials.

- [ ] After every artifact commit, process obsolete managed files through the same journal. Then atomically write `ContentInventory`; write and fsync `state.json` strictly last within the content transaction with the same transaction/content/release binding; update configured game version only after that durable state. Matching durable state performs forward cleanup; missing/nonmatching state rolls back in reverse order. Ambiguous target/backup/state identity preserves evidence and fails closed.

- [ ] After durable state atomically rename completed cache into `.zhekarik/content/cleanup/<uuid>`, resolve success immediately, and delete in background. At startup retry leftover cleanup and migrate legacy `content/chunks` only after valid v3 completion.

- [ ] Add `IntegrityMode::{FastUpdate, FullIntegrity}`. Install/update uses adaptive existing-file selection and repairs only missing/bad chunks. Manual `checkAllFiles=true` must use v3 `FullIntegrity`, hash every non-excluded managed file instead of trusting a matching previous manifest, and repair failures with pack ranges. Pre-launch additional-check remains the established v1-compatible narrow check.

- [ ] Replace runtime entry points in `install_service`, `verify_commands`, and disk preflight with v3; call v1 code only after `Ok(None)` from literal v3 `404`. Leave legacy v2 code readable for recovery/downgrade but unreachable from discovery/download/repair.

- [ ] Make prerequisite analysis read `ContentInventory` first and migrate local v2 state when needed. Preserve its existing v1 sidecar behavior and do not create a network v2 dependency.

- [ ] Reuse the existing status controller. Show independent `0–100%` verification and installation stages, explicit resume text, network/effective throughput, and ETA; never display internal filenames.

- [ ] Add exactly five table-driven Rust test functions under the common prefix `drive_pack_`:

  1. manifest JCS, legacy projection, v1 closure, exact URL, and v3-404-to-v1-with-no-v2-request;
  2. full/range planning, exact HTTP validation, fresh/resumed hash, replica rotation, and corrupt-partial deletion;
  3. independent controller ticks, trial accept/reject, cooldown, pressure downshift, cancellation, and adaptive preemption;
  4. pack materialization, explicit empty files, four integrity layers, inventory creation, and local v2 migration;
  5. stable local reuse, streaming commit, rollback/forward recovery, identity ambiguity, unknown-file preservation, and cache cleanup.

- [ ] Run the second implementation checkpoint once after the entire launcher batch is complete:

```powershell
$ErrorActionPreference = 'Stop'
Set-Location D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo test --manifest-path src-tauri/Cargo.toml drive_pack_ --lib
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

- [ ] If it fails, fix the related group and rerun only the failed command. Do not separately run full `cargo check`, full `cargo test`, or clippy at this checkpoint.

- [ ] Commit the entire runtime integration only after the focused scope passes.

---

## Task 10: Freeze Final Source, Then Prepare and Fully Verify Dormant Packs on NY

**Files:**

- Server state: `/srv/zhekarik-game/publications/1.0.3.6/v3-state.json`
- Server manifests: `/srv/zhekarik-game/content-v3/manifests/`
- Drive root: `zhekarik-launcher-drive:Zhekarik Strike Launcher Releases`

- [ ] Fetch all four target branches again. Merge fresh Seva changes normally into each feature branch, resolve deliberately, and rerun only a checkpoint whose covered files changed.

- [ ] Merge/push the final publisher, backend, and deployment revisions without force. Deploy backend and publisher from their final exact GitHub SHAs while v3 remains dormant. From this point through activation, do not change publisher/backend v3 code or dependencies; a required change invalidates this task's preparation evidence and requires repeating `prepare`/`verify` for the affected publication.

- [ ] From the immutable publisher checkout, run `prepare` as `zs-launcher-publisher` with exact paths:

```bash
sudo -u zs-launcher-publisher /opt/zhekarik-africa/publisher-releases/$PUBLISHER_SHA/.venv/bin/python \
  -m launcher_backend.drive_pack_publisher prepare \
  --storage-root /srv/zhekarik-game \
  --content-sha256 01a13dfb3448ce6c55ec2051d70ad61775cbe1c2fa322330542d3b879d9675db \
  --rclone-bin /usr/local/bin/rclone \
  --rclone-config /var/lib/zhekarik-strike/launcher-publisher/config/rclone.conf \
  --drive-root "zhekarik-launcher-drive:Zhekarik Strike Launcher Releases"
```

- [ ] Let the idempotent command finish all packs; do not activate. Monitor useful byte rate, current ordinal, free space, and failures without printing the rclone configuration or OAuth source.

- [ ] Run the remote-aware `verify` once:

```bash
sudo -u zs-launcher-publisher /opt/zhekarik-africa/publisher-releases/$PUBLISHER_SHA/.venv/bin/python \
  -m launcher_backend.drive_pack_publisher verify \
  --storage-root /srv/zhekarik-game \
  --game-version 1.0.3.6 \
  --rclone-bin /usr/local/bin/rclone \
  --rclone-config /var/lib/zhekarik-strike/launcher-publisher/config/rclone.conf \
  --drive-root "zhekarik-launcher-drive:Zhekarik Strike Launcher Releases"
```

Require exact content SHA `01a13dfb3448ce6c55ec2051d70ad61775cbe1c2fa322330542d3b879d9675db`, 5,688 files, 7,191 unique chunks, 8,757,634,312 pack bytes, 19,607,923,525 unpacked bytes, 136 packs, 408 distinct Drive IDs, exact IDs/sizes, trustworthy MD5 where Drive exposes it, 408 anonymous Range probes, 408 mandatory full streamed SHA checks, and successful file reconstruction hashes without a full output tree.

- [ ] Confirm v3 endpoint still returns `404`, v1 remains `200/206`, no pointer changed, and at least 5 GiB remains free.

- [ ] Save the verified `manifest_sha256` and publication-state SHA in the release record. Commit no generated server state to Git.

---

## Task 11: Version Launcher 1.6.16 and Run One Complete Local Gate

**Files:**

- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\package.json`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\package-lock.json`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\Cargo.toml`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\Cargo.lock`
- Modify: `D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.14\src-tauri\tauri.conf.json`

- [ ] Set version `1.6.16` consistently in package, lockfile top-level entries, Cargo package/lock, and Tauri config. Verify `v1.6.15` remains untouched.

- [ ] Validate the candidate `LAUNCHER_RELEASE_API_TOKEN` without publishing: load the current public launcher manifest, send that unchanged valid body to the deliberately mismatching admin route version `0.0.0`, require authenticated handler response `422`, and prove the active public manifest is byte-identical before/after. A `401` means the candidate is wrong. Then pipe the same nonempty environment value to the GitHub secret and clear the local variable; never echo it.

```powershell
$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($env:LAUNCHER_RELEASE_API_TOKEN)) { throw 'release token is not loaded' }
$publicUrl = 'https://api.zhekarik.africa/launcher/update/windows/x86_64/0.0.0'
$adminUrl = 'https://api.zhekarik.africa/admin/launcher/releases/windows/x86_64/0.0.0'
$before = (Invoke-WebRequest -UseBasicParsing -Uri $publicUrl).Content
$probe = Invoke-WebRequest -UseBasicParsing -SkipHttpErrorCheck -Method Put -Uri $adminUrl -Headers @{ Authorization = "Bearer $env:LAUNCHER_RELEASE_API_TOKEN" } -ContentType 'application/json' -Body $before
if ($probe.StatusCode -ne 422) { throw "safe release-token probe returned HTTP $($probe.StatusCode)" }
$after = (Invoke-WebRequest -UseBasicParsing -Uri $publicUrl).Content
if ($before -cne $after) { throw 'safe release-token probe changed the active manifest' }
$env:LAUNCHER_RELEASE_API_TOKEN | gh secret set LAUNCHER_RELEASE_API_TOKEN --repo ZHEKARIK-AFRICA/zhekarikstrike-launcher
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Remove-Item Env:LAUNCHER_RELEASE_API_TOKEN
```

- [ ] Free space only from confirmed generated launcher build directories if necessary. Preserve dependencies unless the cache is corrupt.

- [ ] Commit the final integrated launcher tree. Create a local, unpushed `v1.6.16` tag on that exact commit and run the third checkpoint once:

```powershell
scripts/release.ps1 -Version 1.6.16
```

- [ ] On failure, delete only the unpushed local tag, fix the grouped cause, rerun only the failed scope for diagnosis, create the local tag on the corrected commit, then rerun the complete release gate once because the release commit changed. Do not publish or reuse a failed remote tag.

- [ ] Record the signed portable path, size, SHA-256, minisign verification result, and exact Git commit. Do not run the full gate separately before or after this script.

---

## Task 12: Controlled Activation and the Single Real Windows E2E

**Files:**

- Disposable Windows workspace chosen after checking configured `gamePath`
- Server pointer: `/srv/zhekarik-game/content-v3/current.json`

- [ ] Record the existing v1 and v2 pointers, then activate only the already verified v3 manifest atomically. Immediately verify v3 `200`, exact ETag/manifest SHA, v1 `200/206`, and health readiness. Keep v2 unchanged until this E2E ends.

- [ ] Prove the chosen Windows E2E directory is disposable and differs from the user's configured game path. Clear only its game/state/cache content, restart the launcher process, and start timing immediately before the frontend invokes installation.

- [ ] Perform the one cold-cache E2E with the locally gated signed `1.6.16` portable:

  - fresh `1.0.3.6` install from v3 Drive packs;
  - observed HTTP/2 plus controller movement between worker levels when justified;
  - cancel/resume with a different replica and preserved verified cache;
  - overlapping download/materialization/commit with no final 19.6 GB reread;
  - manual full verify and deliberate corruption repair through ranges;
  - forced crash, journal rollback, and restart recovery;
  - prerequisites without visible console, UAC `RevLoader.exe`, overlays, game close, and cleanup;
  - a separately controlled v3-pointer absence proving literal `404` selects v1 ZIP;
  - request logs proving launcher `1.6.16` never called v2.

- [ ] Require total time at most `1,812.5` seconds from install invocation through durable matching `state.json` and resolved success. Treat `1,631.25–1,812.5` seconds as the prohibited 10% margin: it is not publishable and requires a concrete tuning change. Publication therefore requires at most `1,631.25` seconds, at most 120 seconds from final network byte to durable success, and no active 8.16 GiB loose cache afterward.

- [ ] If any integrity, recovery, fallback, or timing requirement fails, atomically deactivate v3 immediately and leave v1 active. Revalidate until v3 returns literal `404`, v1 manifest returns `200`, v1 archive Range returns `206`, readiness is healthy, and the archived v3 pointer identity matches the rejected activation. Repeat performance E2E only after a concrete code/config fix or confirmed environment correction; do not rerun merely for a better sample.

- [ ] After success, reactivate the same verified v3 pointer if the 404 fallback drill cleared it and repeat only short manifest/repair/launch smoke checks.

---

## Task 13: Publish Launcher, Retire V2 Safely, and Prove Rollback

**Files:**

- GitHub release/tag: `v1.6.16`
- Server v2 pointers: `/srv/zhekarik-game/content-v2/current.json` and `/srv/zhekarik-game/content-v2/mirrors/google-drive/current.json`
- Server v3 pointer: `/srv/zhekarik-game/content-v3/current.json`

- [ ] Push the already-gated exact launcher commit and the immutable `v1.6.16` tag. Let GitHub Actions run the second and final complete release gate and publish through the corrected admin token.

- [ ] Verify the public portable EXE size/SHA/minisign, GitHub release assets, updater manifest, website download, and self-update from `1.6.13` to `1.6.16`.

- [ ] With v3 active and verified, deactivate v2 content first and v2 Drive mirror second through the single tracked command:

```bash
sudo -u zs-launcher-publisher /opt/zhekarik-africa/publisher-releases/$PUBLISHER_SHA/.venv/bin/python \
  -m launcher_backend.drive_pack_publisher deactivate-v2 \
  --storage-root /srv/zhekarik-game \
  --game-version 1.0.3.6
```

Verify both v2 endpoints return `404`, an old launcher selects v1 ZIP, launcher `1.6.16` continues to use v3, and v1 remains `200/206`.

- [ ] Repair one controlled managed file through v3 while v2 is disabled. Only after that succeeds, run guarded local prune:

```bash
sudo -u zs-launcher-publisher /opt/zhekarik-africa/publisher-releases/$PUBLISHER_SHA/.venv/bin/python \
  -m launcher_backend.drive_pack_publisher prune-local-v2 \
  --storage-root /srv/zhekarik-game \
  --game-version 1.0.3.6 \
  --confirm-v3-repair-verified
```

- [ ] Confirm local v2 chunk route is `404`, v3 and v1 remain healthy, exact v1 archive Range stays `206`, and NY retains at least 5 GiB free.

- [ ] Run the rollback drill by atomically deactivating only v3. Verify v3 `404`, launcher `1.6.16` chooses v1 ZIP, and v2 remains disabled. Reactivate the same verified v3 pointer and perform one short repair smoke.

- [ ] Keep disabled loose Drive v2 objects and manifests for one successful release cycle. After that cycle, run the complete guarded command, verify its exact deletion list, and retain v1 plus all v3 replicas:

```bash
sudo -u zs-launcher-publisher /opt/zhekarik-africa/publisher-releases/$PUBLISHER_SHA/.venv/bin/python \
  -m launcher_backend.drive_pack_publisher prune-drive-v2 \
  --storage-root /srv/zhekarik-game \
  --game-version 1.0.3.6 \
  --rclone-bin /usr/local/bin/rclone \
  --rclone-config /var/lib/zhekarik-strike/launcher-publisher/config/rclone.conf \
  --drive-root "zhekarik-launcher-drive:Zhekarik Strike Launcher Releases" \
  --confirm-release-cycle-complete
```

- [ ] Record final branch SHAs, server checkout SHAs, v1/content/manifest identities, release artifact hashes, E2E timings, pointer states, and remaining disk space in the release handoff.

---

## Completion Criteria

- Launcher `1.6.16` is publicly signed and installs `1.0.3.6` from v3 Google Drive packs.
- The production v3 manifest has 136 packs and exactly three verified anonymous replicas per pack.
- Launcher runtime never queries v2; only v3 `404` selects active v1 ZIP.
- Manual full verification repairs corruption from v3 Range data.
- Streaming commit has no second read of newly materialized files and retains crash-safe rollback.
- The single valid cold-cache E2E completes in at most 1,631.25 seconds, stays outside the prohibited 10% margin, and has no final tail longer than 120 seconds.
- V2 pointers and local blobs are disabled/pruned only after verified v3 repair, while rollback through v1 remains proven.
- All repository changes are on fresh-target feature branches, merged without force-push, and deployed from GitHub by exact SHA.
