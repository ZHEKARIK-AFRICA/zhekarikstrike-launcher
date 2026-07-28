# Adaptive Integrity Verification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the non-functional custom window controls and make SHA-256 integrity verification automatically scale from one to at most six workers using measured throughput and CPU load.

**Architecture:** Keep Tauri window actions in the existing shared renderer and grant only the two required capabilities. Split verification into manifest classification plus a bounded adaptive hashing scheduler; the scheduler owns concurrency decisions while `verify_service` retains API, repair, and configuration responsibilities.

**Tech Stack:** Tauri 2 capabilities and window API, Vitest/jsdom, Rust/Tokio `JoinSet`, `CancellationToken`, RustCrypto SHA-256, `sysinfo` CPU sampling.

## Global Constraints

- Do not classify disks as HDD, SSD, or NVMe.
- Start every hashing operation with one worker.
- Sample every 5 seconds and require at least 64 MiB before changing concurrency.
- Keep a trial only for at least 5% throughput gain; rollback otherwise.
- Downshift above 90% CPU and wait 30 seconds after rollback/downshift.
- Cap workers at `clamp(logical_cpu_count / 2, 1, 6)`.
- Preserve SHA-256 verification, manifests, command names, structured errors, cancellation, and repair behavior.
- Run tests in one focused batch, not after every source-file edit.
- Prepare version 1.6.8 but do not publish it.

---

### Task 1: Regression tests for window controls and controller boundaries

**Files:**
- Create: `tests/frontend/renderer-window.test.js`
- Create: `src-tauri/src/services/verify_hash_service.rs`
- Modify: `src-tauri/src/services/mod.rs`

**Interfaces:**
- Consumes: `getCurrentWindow()` from `@tauri-apps/api/window`.
- Produces: `AdaptiveVerifyController::new(maximum)`, `observe_window(sample_bytes, throughput, cpu_percent, has_pending)`, and `verify_worker_limit(logical_cpu_count)`.

- [ ] **Step 1: Add frontend regression tests**

Create a jsdom test that mocks `getCurrentWindow` with `close` and `minimize` spies, imports `src/renderer/renderer.js`, dispatches `DOMContentLoaded`, clicks both buttons, and asserts both promises are called. Read `src-tauri/capabilities/default.json` and assert it contains the two window permissions.

```js
expect(windowApi.close).toHaveBeenCalledOnce();
expect(windowApi.minimize).toHaveBeenCalledOnce();
expect(capability.permissions).toEqual(expect.arrayContaining([
  'core:window:allow-close',
  'core:window:allow-minimize'
]));
```

- [ ] **Step 2: Add controller tests before implementation**

Define focused Rust tests for CPU-derived worker limits, first baseline/trial, a 5% accepted trial, a less-than-5% rollback, a greater-than-10% regression, CPU downshift, the 64 MiB minimum sample, and six-window cooldown.

```rust
assert_eq!(verify_worker_limit(1), 1);
assert_eq!(verify_worker_limit(8), 4);
assert_eq!(verify_worker_limit(64), 6);
```

- [ ] **Step 3: Run one red batch**

```powershell
npx vitest run tests/frontend/renderer-window.test.js
cargo test --manifest-path src-tauri/Cargo.toml verify_controller_ --lib
```

Expected: frontend permissions test and Rust controller tests fail because the permissions/controller implementation do not exist yet.

### Task 2: Window control fix

**Files:**
- Modify: `src-tauri/capabilities/default.json`
- Modify: `src/renderer/renderer.js`

**Interfaces:**
- Consumes: Tauri `WebviewWindow.close(): Promise<void>` and `minimize(): Promise<void>`.
- Produces: working custom buttons with logged rejected promises.

- [ ] **Step 1: Grant the minimum window permissions**

Append only:

```json
"core:window:allow-close",
"core:window:allow-minimize"
```

- [ ] **Step 2: Await and log window action failures**

```js
async function runWindowAction(action, label) {
  try {
    await action();
  } catch (error) {
    console.error(`Failed to ${label} window:`, error);
  }
}
```

The click handlers pass thunks such as `() => getCurrentWindow().close()` and
`() => getCurrentWindow().minimize()` to this helper so synchronous and asynchronous
failures are both logged.

### Task 3: Adaptive hashing scheduler

**Files:**
- Modify: `src-tauri/src/services/verify_hash_service.rs`
- Modify: `src-tauri/src/utils/hash_utils.rs`
- Modify: `src-tauri/src/services/verify_service.rs`
- Modify: `src-tauri/src/services/mod.rs`

**Interfaces:**
- Consumes: `Vec<VerifyHashTask>`, `CancellationToken`, and `Arc<dyn Fn(VerifyHashProgress) + Send + Sync>`.
- Produces: `find_hash_mismatches(...) -> Result<Vec<GameFileManifestEntry>, AppError>`.

- [ ] **Step 1: Add tracked cancelable hashing**

Add `sha256_file_tracked(path, cancel, completed_bytes)`. Read sequentially with a reusable 1 MiB buffer, check cancellation before every read, increment an `AtomicU64`, and return `AppError::Canceled` immediately when requested. Keep `sha256_file` unchanged for existing callers.

- [ ] **Step 2: Implement the pure controller**

```rust
const CONTROL_WINDOW: Duration = Duration::from_secs(5);
const MIN_SAMPLE_BYTES: u64 = 64 * 1024 * 1024;
const COOLDOWN_WINDOWS: u8 = 6;
const MIN_TRIAL_GAIN: f64 = 1.05;
const CPU_GROW_LIMIT: f32 = 80.0;
const CPU_SHRINK_LIMIT: f32 = 90.0;
```

`observe_window` decrements cooldown on elapsed windows, ignores undersized samples, rolls back an active trial below 5% gain, accepts and chains the next trial when gain is sufficient, and stays within `1..=maximum`.

- [ ] **Step 3: Implement the bounded scheduler**

Use `VecDeque<VerifyHashTask>` and `JoinSet<Result<VerifyHashResult, AppError>>`. Launch while `join_set.len() < controller.current()`. Select across completion, a 250 ms progress interval, the 5-second control interval, and cancellation. A downshift does not abort active files. On error, cancel the child token and drain every task before returning.

Refresh `sysinfo::System` CPU usage at control intervals. Log initial/max concurrency and every controller decision without telemetry.

- [ ] **Step 4: Emit byte-based progress**

Report `VerifyHashProgress { completed_bytes, total_bytes, speed_bytes_per_sec, time_remaining_sec, current_file }`. `verify_service` converts this into the existing `ProgressPayload`; network byte fields remain unset.

- [ ] **Step 5: Refactor manifest verification**

Missing entries and wrong-size entries become download tasks, existing excluded entries are skipped, and remaining entries become `VerifyHashTask`s. Emit classification progress from 0–5%, run adaptive hashing from 5–100%, append mismatches to the existing repair list, and preserve version/completion behavior.

### Task 4: Prepare 1.6.8 and verify the batch

**Files:**
- Modify: `package.json`
- Modify: `package-lock.json`
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/Cargo.lock`
- Modify: `src-tauri/tauri.conf.json`

**Interfaces:**
- Produces: consistent unreleased version `1.6.8` across all release metadata.

- [ ] **Step 1: Update all version fields**

Replace only launcher version `1.6.7` with `1.6.8` in the five files. Do not create or push a tag.

- [ ] **Step 2: Run the single focused green batch**

```powershell
npm run lint
npx vitest run tests/frontend/renderer-window.test.js tests/frontend/renderer-index.test.js
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml verify_ --lib
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Expected: all commands exit 0, controller/window regression tests pass, and clippy emits no warnings.

- [ ] **Step 3: Review and commit**

Run `git diff --check`, inspect the complete diff, and confirm no generated build artifacts are staged. Commit source, tests, and version metadata:

```powershell
git commit -m "feat: add adaptive integrity verification"
```

Do not push or publish without an explicit user request.
