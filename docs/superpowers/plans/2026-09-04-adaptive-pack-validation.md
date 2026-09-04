# Adaptive Google Drive pack validation

Approved scope: diagnose and verify the existing controller, without changing its policy, the installed game, server, public APIs, or publishing a launcher.

## Constraints

- Start at two download jobs, cap six, keep the 256 MiB ready-backlog threshold and existing trial/pressure decisions.
- Production and test probe share downloader, integrity checks, materializer, staging and commit.
- Diagnostics every ten seconds and immediately on decisions changing concurrency. Separate jobs, HTTP requests, received bytes, verified bytes, materialization and commit.
- Test-only headless probe under cfg(test); unique owned directories on D:, no config setters, UAC, prerequisites or game launch.
- One focused release-profile drive_pack_ test batch after implementation. No full suite, cargo clean, or per-function builds.
- Real ABBA series: fixed2, adaptive, adaptive, fixed2, identical whole-file/pack subset, fresh caches, fixed replica selection seed. At most 512 MiB planned per run and 2 GiB received bodies across the series, retries included.
- Preserve full server manifest identity; select only whole-file closure and only needed readiness chunks. Remove probe-only state/data after awaited completion, retain reports.
- Compare pipeline-to-commit completion time. Claim a measured gain only when adaptive is faster in both paired comparisons and mean gain >=10%; correct behavior can retain two workers.
- All subagents explicitly use gpt-5.6-luna/low, otherwise primary works itself.

## Tasks

- [x] 1. Persist global subagent preference and verify isolated worktree.
- [x] 2. Add decision reasons, low-overhead shared metrics and test-only scheduler controls.
- [x] 3. Add headless common-pipeline probe, real local-transport behavior tests and safe script.
- [x] 4. Run one focused release test batch, review and correct affected scopes only.
- [x] 5. Run bounded Drive ABBA, retain report and remove only owned probe data.

## Execution notes

Baseline: 6d8efed on codex/drive-pack-v3-1.6.16, existing linked worktree with warm build caches.
User explicitly overrides skill per-function red/green test runs: assertions are prepared with implementation, verification runs once after the complete batch.
No existing test installation is a probe root. Completion state created by subset commit is always disposable probe state.

2026-09-04 focused gate: formatting passed; 11 tests passed, real-Drive test intentionally ignored. Initial compile found two misplaced/missing diagnostic arguments on the Range path; corrected together before rerunning this same scope. No full suite, cargo clean or launcher rebuild/release was run.
Controlled transport tested real 2-to-3 jobs and HTTP requests, acceptance/rejection, backlog hold/release, HTTP 429 decrease, and zero jobs/requests after cancellation. Cheap independent review checked the probe boundary; root path is rechecked after creation. Exact Range Content-Length is validated before body accounting; concurrent request reservations include retry bodies.
Real ABBA started with 359 whole files, 8 packs and 453.5 MiB planned per run. Full immutable server manifest is retained unchanged in the report directory. Fresh owned work data is removed only after all pipeline tasks finish. Unexpected process termination deliberately leaves marked data for inspection, never deletes arbitrary stale directories.

Real ABBA completed successfully: A1 72.8577291 s, B1 53.6390186 s, B2 48.7325424 s, A2 54.395247 s. Both adaptive runs reached three actual jobs/HTTP requests; both fixed runs stayed at two. Paired gains 26.38% and 10.41%, combined elapsed-time gain 19.55%. Total received pack bodies 1,902,129,268 bytes, no retry overhead. All work directories removed; protected installation state/config SHA-256 remained unchanged. Raw report: `D:\zhekarik-adaptive-pack-probe\series-442710e6-dffe-4da2-971f-58e9008523da\report.json`.
Post-measurement telemetry-only corrections refresh final queue gauges, log materializer target changes immediately, and suppress artificial "changed to 2" messages from the fixed baseline's unused adaptive target. These do not change admission, transfer, verification, materialization or commit policy. Only the affected metrics test scope is rerun; no additional Drive comparison or full suite.
