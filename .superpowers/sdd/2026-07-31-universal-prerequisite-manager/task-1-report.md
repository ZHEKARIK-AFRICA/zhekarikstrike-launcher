# Task 1 report — Universal prerequisite service

## Status

DONE_WITH_CONCERNS. The reusable Rust service and focused tests are implemented on `codex/launcher-1.6.13` in commit `822186b`. Task 2 command/frontend integration and Task 3 obsolete-file deletion were intentionally not touched.

## Changed files

- `src-tauri/src/services/prerequisite_service.rs` — embedded catalog, PE import parser, active-manifest analysis/cache, secure acquisition and verification pipeline, Windows adapters, injectable boundaries, and 12 focused tests.
- `src-tauri/src/services/mod.rs` — exports the service; temporary `dead_code` allowance documents that Task 2 is the first consumer.
- `src-tauri/Cargo.toml` — enables the Windows WinTrust and Cryptography bindings used by `WinVerifyTrust`.
- `src-tauri/Cargo.lock` — unchanged; no new crate dependency was added.

## Architecture and API

- `PrerequisiteService::windows()` assembles production adapters; `PrerequisiteService::new(...)` accepts injected `InstallerDownloader`, `TrustVerifier`, `RuntimeProbe`, and `InstallerRunner` implementations for unit tests.
- `analyze_active(game_root, cancel)` loads and validates `.zhekarik/content/state.json` plus its referenced manifest, scans only manifest `.exe`/`.dll` files, parses PE machine/import tables, resolves matching app-local DLLs beside the module, at the game root, and under `bin`, then probes the Windows runtime.
- Analysis is atomically cached at `.zhekarik/prerequisites/analysis-v1.json`. The cache requires schema 1, the active `content_sha256`, and catalog version 1. Missing/invalid active state removes the old cache; changed content/catalog bypasses and replaces it.
- The allowlist maps only VC++ 2010 (`MSVCR/MSVCP/MFC/ATL 100`) and DirectX June 2010 (`D3DX9/10/11`, `XInput1_3`, `XAudio2_7`, `XAPOFX`, `D3DCompiler_43`) import families. A same-architecture `bin/xinput1_3.dll` is treated as satisfied.
- `ensure_active(game_root, cancel)` rechecks the runtime, installs only required allowlisted components, distinguishes exit codes 0/3010/1641/other, performs the architecture/version post-check, and returns `ready`, `installed`, `alreadyPresent`, and `restartRecommended` without initiating a launcher-controlled reboot.
- The VC++ adapter uses a dedicated no-redirect HTTPS client, exact pinned Microsoft URL/host, bounded `.part`, streaming SHA-256, verified cache reuse, and same-directory atomic rename. It runs `/quiet /norestart`.
- The DirectX adapter accepts only the exact active-manifest local source, checks pinned size/hash, Authenticode/Microsoft signer, and x86 PE architecture, extracts with `/Q /T:<unique-dir>`, verifies the extracted `DXSETUP.exe`, runs `/silent`, and removes the unique extraction directory.
- Every executable is rechecked immediately before execution for expected size (where pinned), SHA-256 (where pinned), PE architecture, successful `WinVerifyTrust`, and an exact Microsoft certificate common name. Runtime post-checks inspect the actual DLL PE architecture; VC++ also requires file version `>= 10.0.40219.325`.
- `PrerequisiteError::code()` supplies the Task 2 structured codes without adding commands or frontend state in this task.

## TDD evidence

All Cargo commands used the required shared cache:

`CARGO_TARGET_DIR=D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.12\src-tauri\target`

1. Initial RED, before production implementation:
   - Command: `cargo test prerequisite_service::tests --lib`
   - Result: expected compile failure, exit 1, 62 unresolved new-service symbols (for example `PeArchitecture`, `AnalysisCache`, `PrerequisiteService`, and `parse_pe_image`).
2. First GREEN:
   - Command: `cargo test prerequisite_service::tests --lib`
   - Result: 10 passed, 0 failed, 92 filtered out.
3. Active-state invalidation regression RED:
   - Command: `cargo test missing_active_content_state_invalidates_an_existing_analysis_cache --lib`
   - Result: expected compile failure, exit 1, unresolved `load_active_manifest`.
4. Active-state GREEN:
   - Command: `cargo test prerequisite_service::tests --lib`
   - Result: 11 passed, 0 failed, 92 filtered out.
5. Exact Microsoft signer regression RED:
   - Command: `cargo test microsoft_signer_check_requires_the_exact_certificate_common_name --lib`
   - Result: expected compile failure, exit 1, unresolved `is_microsoft_signer`.
6. Current GREEN before report:
   - Command: `cargo test prerequisite_service::tests --lib`
   - Result: 12 passed, 0 failed, 92 filtered out; no warnings.

The first attempt after enabling the additional Windows bindings timed out while compiling `windows`; rerunning against the same shared target completed normally. This was a build-cache timing event, not a test failure.

## Focused test coverage

- PE x86 architecture detection, case-insensitive import parsing, allowlisted mapping, and unknown-import exclusion.
- App-local `bin/xinput1_3.dll` satisfaction with matching PE architecture.
- Cache schema/content/catalog matching and missing active-state invalidation.
- Exact URL/HTTPS/Microsoft host checks, redirect rejection, status and Content-Length rejection.
- Exact artifact size and SHA-256 rejection.
- Signature rejection and exact Microsoft certificate CN allowlist.
- Separate installer exit outcomes for 0, 3010, 1641, and other codes.
- Runtime post-check failure after a nominal installer success.
- Pre-cancellation before downloader or runner side effects.
- Exact pinned catalog values for both initial components.

## Self-review

- Re-read `task-1-brief.md` line by line and mapped every Task 1 requirement to code/tests above.
- Confirmed no Tauri command, frontend, app state, content-install hook, Play flow, or obsolete deletion was introduced.
- Confirmed the remote downloader cannot follow redirects and cannot write beyond the pinned size; failure/cancellation removes `.part`.
- Confirmed local DirectX source metadata must match both the active manifest and embedded catalog before disk verification/execution.
- Confirmed installer execution always follows integrity, architecture, WinTrust, and signer checks, and post-checks use actual runtime DLLs.
- `cargo fmt --all -- --check` and `git diff --check` passed.
- Per task instruction, the full Rust test suite was not run.

## Risks / concerns

- Production `WinVerifyTrust`, Microsoft signer extraction, Windows file-version probing, Microsoft download, and the two real installers were compiled on Windows but deliberately not executed by unit tests. The signer/version adapters use built-in Windows PowerShell after `WinVerifyTrust`; systems where Windows PowerShell is removed will receive a verification error instead of executing an unverified installer.
- The service module is intentionally not yet consumed, so `services/mod.rs` temporarily allows dead code until Task 2 wires commands/startup/Play integration.
- Exit code 1641 is reported separately as restart-initiated/recommended state; the launcher never calls a reboot API, and VC++ is invoked with `/norestart`, but an installer that ignores its documented no-reboot contract is outside launcher control.

## Fix round 1 — 2026-08-01

### Status

DONE_WITH_CONCERNS. All three Important findings from `task-1-review.md` are closed in the Task 1 service and focused tests. Task 2/3 remain untouched.

### Changes

- VC100 satisfaction during analysis now resolves the concrete system or app-local DLL, verifies its PE architecture, and asks the injected runtime probe for the DLL file version. A VC100 DLL satisfies the import only at `>= 10.0.40219.325`.
- App-local analysis evaluates every allowed location (beside the importing module, game root, then `bin`) rather than allowing an outdated or wrong-architecture earlier candidate to hide a valid later candidate.
- `PrerequisiteRequirement` now serializes `architecture`; aggregation uses `(component_id, importer_architecture)` and analysis cache schema is 2 so schema-1 entries cannot be reused without the new identity field.
- An unsatisfied x64 or mixed x86/x64 import set returns `PrerequisiteError::Unsupported` when the embedded catalog contains only the x86 component. A satisfied same-architecture app-local/system DLL still avoids an unnecessary package requirement.
- `ensure_manifest` rejects a cached requirement whose architecture does not match its catalog component, and runtime pre/post-checks receive the saved requirement architecture explicitly.
- `EnsurePrerequisitesResult` now exposes `restartStatus` with serialized values `none`, `recommended`, and `initiated`. Installer exits 3010 and 1641 remain distinct through `complete_install` and the public result; aggregation gives `initiated` precedence over `recommended`.

### Fix-round TDD evidence

Every Cargo command below used:

`CARGO_TARGET_DIR=D:\projects\bots\zhekarikstrike-launcher\.worktrees\launcher-1.6.12\src-tauri\target`

`CARGO_BUILD_JOBS=1`

1. Finding 1 inherited RED (captured before the inherited production diff):
   - Command: `cargo test outdated_ --lib`
   - Result: exit 1 with expected `E0407` errors because `RuntimeProbe::system_dependency_path` and `RuntimeProbe::dependency_file_version` did not yet exist.
2. Finding 1 resumed GREEN:
   - Command: `cargo test outdated_ --lib`
   - Result: 2 passed, 0 failed, 104 filtered out.
3. Finding 2 RED:
   - Command: `cargo test importer_ --lib`
   - Result: exit 1 with expected `E0609`: `PrerequisiteRequirement` had no `architecture` field.
4. Finding 2 GREEN:
   - Command: `cargo test importer_ --lib`
   - Result: 2 passed, 0 failed, 107 filtered out (x86 cache identity and explicit x64 rejection).
   - Command: `cargo test mixed_x86_and_x64_importers_are_explicitly_unsupported --lib`
   - Result: 1 passed, 0 failed, 108 filtered out.
5. Finding 3 RED:
   - Command: `cargo test statuses --lib`
   - Result: exit 1 with nine expected compiler errors: `E0560` for missing public `restart_status`, `E0433` for missing `RestartStatus`, and `E0610` because `complete_install` still returned `bool`.
6. Finding 3 GREEN:
   - Command: `cargo test statuses --lib`
   - Result: 2 passed, 0 failed, 109 filtered out; both the completion outcome and serialized public result distinguish 3010 from 1641.
7. Self-review regression RED:
   - Command: `cargo test analysis_uses_a_later_current_app_local_vc100_candidate --lib`
   - Result: expected assertion failure, 0 passed and 1 failed, because the outdated adjacent DLL hid the current `bin` DLL.
8. Self-review regression GREEN:
   - Command: `cargo test analysis_uses_a_later_current_app_local_vc100_candidate --lib`
   - Result: 1 passed, 0 failed, 111 filtered out.
9. Fresh final focused verification after formatting:
   - Command: `cargo test prerequisite_service::tests --lib`
   - Result: 20 passed, 0 failed, 92 filtered out.
   - Command: `cargo fmt --all -- --check`
   - Result: exit 0.
   - Command: `git diff --check`
   - Result: exit 0 (Git emitted only the repository's LF-to-CRLF checkout warning).

Several compile/link attempts exceeded the command wrapper's 124/244/604-second limits while the Windows test binary continued building as a child process. No timeout was counted as test evidence. The recorded RED/GREEN results above come only from commands that returned complete compiler/test output and explicit exit codes.

### Fix-round self-review

- Re-read all three Important findings against the final data flow, not just the new tests.
- Finding 1: both app-local and system candidates go through the same architecture/version predicate; missing version metadata is unsatisfied, and the version reader remains injected through `RuntimeProbe`.
- Finding 2: architecture is present in requirement identity and JSON, schema-1 cache data is invalidated, unsupported architectures cannot select the x86 installer, and post-check uses the saved requirement architecture.
- Finding 3: 3010 maps to `recommended`, 1641 maps to `initiated`, the distinction survives `complete_install` and public serialization, and no reboot API was added.
- Mutation check: removing the version comparison, dropping the requirement architecture, allowing x64 to reuse the x86 entry, collapsing either restart status, or restoring first-existing app-local behavior makes at least one focused regression test fail.
- Confirmed the diff remains limited to the Task 1 service/tests and this Task 1 report; no Tauri command, frontend integration, Task 2 wiring, or Task 3 deletion was added.

### Remaining concerns

- Real `WinVerifyTrust`, signer extraction, Windows file-version probing, Microsoft download behavior, and the installers themselves remain deliberately unexecuted; focused tests cover the injected boundaries and policy logic only.
- The full Rust suite was not run, per the task instruction. Final verification was limited to the 20 Task 1 service tests.
