# Adaptive integrity verification design

## Goal

Speed up the explicit full integrity check for an already installed game without weakening its SHA-256 guarantee. The launcher must still read and authenticate every non-excluded byte during a full check, but it should use the storage device and available CPU efficiently.

This change does not add a metadata cache or a new quick-check mode. Pre-launch verification remains `AdditionalOnly`; the manual check remains a full cryptographic verification.

## Current bottleneck

`verify_service` currently walks the manifest and awaits `sha256_file` for every file in sequence. The current client is roughly 18.2 GiB across 3,596 manifest files; about 17.3 GiB is concentrated in 203 files of at least 8 MiB. Sequential hashing is appropriate for an HDD, but leaves SSD/NVMe queue depth and multiple CPU cores unused.

## Worker limits

The launcher does not classify the storage device. USB enclosures, RAID, virtual disks, and some drivers report media type unreliably, so concurrency is learned from measured throughput instead.

- Every verification starts with one worker.
- Maximum concurrency is `clamp(logical_cpu_count / 2, 1, 6)`.
- The controller trials one additional worker at a time and keeps it only when measured throughput improves enough to justify the additional disk and CPU load.

Every trial, accepted limit, rollback, and CPU-driven downshift is written to the local launcher log.

## Verification pipeline

Full verification is split into two phases:

1. Walk the manifest once, validate safe paths, and classify entries as missing, excluded, or requiring hashing.
2. Hash candidates through a bounded scheduler and collect mismatches for the existing download/repair stage.

Each hashing worker:

- opens one file and reads it sequentially with a larger reusable buffer;
- checks the shared cancellation token between reads;
- increments a shared byte counter while feeding SHA-256;
- returns the manifest entry and whether size/hash matched.

The scheduler owns the pending queue and a `JoinSet`. It starts jobs only while the active job count is below the controller limit. Reducing the limit never interrupts an active file; it only delays new jobs. On cancellation or an error, pending work is abandoned, active workers observe cancellation, and every task is drained before the command returns.

Missing files and hash mismatches continue through the existing verified download path. Public command names, manifest formats, error payloads, and repair behavior do not change.

## Adaptive controller

The controller samples every 5 seconds using aggregate verified bytes and `sysinfo` CPU usage. A window must process at least 64 MiB before it can change the limit; smaller windows are ignored so file-open noise does not drive decisions.

- With CPU below 80% and pending work, trial one additional worker up to the CPU cap.
- Accept a trial only when throughput improves by at least 5% over the preceding valid window.
- If a trial improves throughput by less than 5% or reduces it, return to the previous limit. A reduction greater than 10% is logged as a regression.
- If CPU exceeds 90%, decrease concurrency by one.
- Every rollback or CPU-driven downshift starts a 30-second cooldown before another upward trial.
- During cooldown no new trial increase is attempted.
- Tiny checks that never produce two valid observation windows retain one worker.

The controller compares only adjacent valid windows. It remains a pure state machine so its boundaries can be tested without reading the full client. It deliberately does not persist a learned value between operations because storage load and available CPU can change.

## Progress

Verification progress becomes byte-based for hash candidates and remains monotonic. The UI continues to receive the existing `verify-progress` payload and `ProgressStage::Verify`; no frontend contract changes are required. Missing/excluded-file classification occupies 0–5%, while hashing occupies 5–100% of the verification phase.

ETA uses aggregate verified bytes per second. Download/repair progress continues to use the existing download service after the hash phase.

## Error handling and compatibility

- Metadata/read/hash failures preserve the existing structured `AppError` behavior.
- Excluded paths remain existence-checked but are not hashed, matching current behavior.
- `AdditionalOnly` uses the same scheduler but normally finishes with its small manifest before adaptation matters.
- Because `1.6.7` is already published, the implementation prepares launcher version `1.6.8`; publication is a separate explicit action.

## Tests and verification

Keep Rust coverage focused:

- CPU-derived maximum worker limits;
- successful trial acceptance, insufficient-gain rollback, regression rollback, CPU downshift, minimum sample size, and cooldown;
- bounded scheduler cancellation and mismatch collection using small temporary files.

Run tests once after the implementation batch:

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml verify_ --lib
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Do not run a full release gate or publish `1.6.8` unless explicitly requested.
