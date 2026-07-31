# Universal prerequisite manager — launcher 1.6.13

## Global constraints

- Work from fresh `origin/tauri-rework` and publisher `origin/master`; preserve Seva's changes, no force-push.
- Windows x86_64 only. Backend and public game APIs do not change.
- Test in batches: focused tests for each task, then one full release gate.
- Use strict TDD for new behavior: add focused failing tests, confirm expected failure, then implement.
- User downloads one portable EXE. Never force a Windows reboot and never launch installers through `cmd`.

## Task 1 — Universal prerequisite service

Implement a reusable Rust prerequisite service and embedded allowlisted catalog.

- Scan PE imports for `.exe` and `.dll` files from the active content manifest, detect PE architecture, and map only known missing imports to catalog component IDs.
- Resolve app-local DLLs beside the importing module, in the game root, and in `bin` before requiring a system package.
- Cache analysis at `.zhekarik/prerequisites/analysis-v1.json`, keyed by active `content_sha256` and catalog version. Invalid/missing state or changed content invalidates the cache.
- First catalog entries:
  - `vc2010-sp1-x86`: Microsoft URL `https://download.microsoft.com/download/1/6/5/165255E7-1014-4D0A-B094-B6A430A6BFFC/vcredist_x86.exe`, size `8993744`, SHA-256 `99dce3c841cc6028560830f7866c9ce2928c98cf3256892ef8e6cf755147b0d8`, required runtime version `10.0.40219.325`, imports covering MSVCR/MSVCP/MFC/ATL 100.
  - `directx-june-2010`: game-local `directx_installer/directx_jun2010_redist.exe`, size `100271992`, SHA-256 `8746ee1a84a083a90e37899d71d50d5c7c015e69688a466aa80447f011780c0d`, imports covering D3DX9/10/11, XInput1_3, XAudio2_7, XAPOFX, and D3DCompiler_43.
- A current `bin/xinput1_3.dll` must satisfy the DirectX import without installing DirectX.
- VC++ download uses a dedicated no-redirect client, exact HTTPS URL/host, bounded `.part`, streaming hash, verified cache, and atomic rename.
- Before executing either installer require exact size, pinned hash, successful `WinVerifyTrust`, Microsoft signer, and expected architecture/source.
- Run VC++ silently without reboot. DirectX silently extracts into a unique temp directory, verifies the extracted `DXSETUP.exe`, runs `/silent`, and cleans temporary files.
- Handle installer exit codes 0, 3010, 1641, and other codes separately. Post-check actual DLL architecture/version after install. Never reboot automatically.
- Make downloader, trust verifier, runtime probe, and installer runner injectable so Windows behavior can be tested without installing packages in unit tests.
- Add focused tests for PE mapping, app-local satisfaction, cache invalidation, URL/redirect/hash/signature rejection, installer exits, post-check, and cancellation.

## Task 2 — Tauri lifecycle and frontend UX

Integrate Task 1 into install and play flows.

- Add `InstallingPrerequisites` operation state and expose commands `ensure_game_prerequisites` and `get_prerequisite_state`.
- Return `{ ready, installed, alreadyPresent, restartRecommended }` and emit `prerequisite-progress` with operation ID, stage, component ID, percentage, byte counts, and restart recommendation.
- Add structured errors: `prerequisite_download_failed`, `prerequisite_verification_failed`, `prerequisite_install_failed`, `prerequisite_restart_required`, `prerequisite_unsupported`.
- Run automatically after successful content installation. Dependency failure must not roll back or invalidate installed game content; show the exact error, navigate to main, and retry only prerequisites on Play.
- In Play, run after update/verify and before `rev.ini`, overlays, and game launch. `launch_game` performs a final internal fast prerequisite check so direct frontend command invocation cannot bypass it.
- Add RU/EN statuses for detecting, downloading, installing, and verifying components. Block Play/manual verify while running. Restore status after reload.
- Downloads are cancelable. Once a system installer starts, do not kill it; close lifecycle waits up to 10 seconds and a later launch re-checks the result.
- Retain the `RevLoader.exe` child handle. If it exits before the game appears, map `0xC0000135` to a targeted prerequisite/restart error; report other exit codes immediately rather than waiting 60 seconds.
- Add focused frontend and Rust tests for status flow, retry, blocking, reload, close/cancel, internal launch guard, and quick RevLoader exit.

## Task 3 — Transactional obsolete content deletion

Extend v2 content updates so files removed from the next immutable manifest are safely deleted.

- Compute obsolete paths only as `previous valid manifest - new manifest`. Never delete unknown/user files; if the previous manifest cannot be loaded, delete nothing.
- Introduce journal schema v2 with explicit replace/remove actions while retaining schema v1 recovery compatibility.
- During commit move obsolete files to transaction backup before removal. Rollback restores them; successful commit removes backup and only safe empty directories.
- Include obsolete-file sizes in disk preflight and preserve chunks/`.part` behavior.
- Add focused tests for deletion, user file preservation, interrupted commit rollback, completed recovery, malformed paths, missing previous manifest, and v1 journal compatibility.

## Task 4 — Publisher prepare/activate and game content 1.0.3.5

Implement in the `zs-updater` publisher repository from fresh `origin/master`.

- Split v1 publication into an immutable `prepare` and a validating atomic `activate`, retaining a compatibility entrypoint for the existing publish behavior.
- Add a reproducible derive/repack operation that creates `client-1.0.3.5-normalized.zip` from `1.0.3.4`, excluding exactly 157 files under `directx_installer` while retaining `directx_jun2010_redist.exe`.
- Validate all 3439 retained paths, sizes, and SHA-256 values against `1.0.3.4`; removed bytes must equal `102930955`. Refuse additions, wrapper directories, removal of `RevLoader.exe`, or incomplete removal lists.
- Preparation must not change `current`, `current-manifest.json`, v2 pointer, or Drive pointer. Activation validates the immutable archive/release/manifest immediately before changing v1 pointers.
- Preserve the old `1.0.3.4` archive, release, and manifest.
- Add publisher tests for dormant preparation, activation, exact retained/removed closure, rollback on failure, and compatibility behavior.

## Task 5 — Verification and rollout

- Run frontend lint/unit/browser tests, focused Rust prerequisite/content deletion tests, Python publisher tests, then the single full local `scripts/release.ps1 -Version 1.6.13` gate.
- On Oracle, deploy publisher code only through GitHub by exact commit SHA. Prepare v1 `1.0.3.5`, v2 `1.0.3.5-r1`, fully reconstruct it, and prepare its Drive mirror without changing active pointers.
- Real Windows E2E: missing VC++ installs automatically; no MSVCR100 dialog; second launch is fast; offline failure retries; update removes the 157 known files but preserves an injected user file; crash rollback works; game and overlays work.
- Publish signed launcher `1.6.13`, verify updater/minisign/SHA and public portable EXE, then activate v1 -> v2 -> Drive pointers. Roll back pointers to `1.0.3.4-r1` on failure.
