use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sysinfo::System;
use tokio::task::JoinSet;
use tokio::time::{interval_at, Instant as TokioInstant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::models::GameFileManifestEntry;
use crate::utils::hash_utils::sha256_file_tracked;

const CONTROL_WINDOW: Duration = Duration::from_secs(5);
const PROGRESS_WINDOW: Duration = Duration::from_millis(250);
const MIN_SAMPLE_BYTES: u64 = 64 * 1024 * 1024;
const COOLDOWN_WINDOWS: u8 = 6;
const MIN_TRIAL_GAIN: f64 = 1.05;
const REGRESSION_THRESHOLD: f64 = 0.90;
const CPU_GROW_LIMIT: f32 = 80.0;
const CPU_SHRINK_LIMIT: f32 = 90.0;

pub(crate) struct VerifyHashTask {
    pub(crate) file: GameFileManifestEntry,
    pub(crate) local_path: PathBuf,
}

pub(crate) struct ContentHashTask {
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) expected_sha256: String,
    pub(crate) local_path: PathBuf,
}

struct HashTask {
    key: String,
    size: u64,
    expected_sha256: String,
    local_path: PathBuf,
}

pub(crate) struct VerifyHashProgress {
    pub(crate) completed_bytes: u64,
    pub(crate) total_bytes: u64,
    pub(crate) speed_bytes_per_sec: f64,
    pub(crate) time_remaining_sec: Option<f64>,
    pub(crate) current_file: Option<String>,
}

pub(crate) type VerifyProgressCallback = Arc<dyn Fn(VerifyHashProgress) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq)]
enum VerifyControllerDecision {
    Unchanged,
    TrialStarted {
        from: usize,
        to: usize,
    },
    TrialAccepted {
        workers: usize,
        gain_percent: f64,
        next_trial: Option<usize>,
    },
    TrialRolledBack {
        from: usize,
        to: usize,
        gain_percent: f64,
        regression: bool,
    },
    CpuDownshift {
        from: usize,
        to: usize,
    },
}

#[derive(Debug)]
struct AdaptiveVerifyController {
    current: usize,
    maximum: usize,
    trial_baseline: Option<f64>,
    cooldown_windows: u8,
}

impl AdaptiveVerifyController {
    fn new(maximum: usize) -> Self {
        Self {
            current: 1,
            maximum: maximum.max(1),
            trial_baseline: None,
            cooldown_windows: 0,
        }
    }

    fn current(&self) -> usize {
        self.current
    }

    fn observe_window(
        &mut self,
        sample_bytes: u64,
        throughput: f64,
        cpu_percent: f32,
        has_pending: bool,
    ) -> VerifyControllerDecision {
        if cpu_percent > CPU_SHRINK_LIMIT && self.current > 1 {
            let from = self.current;
            self.current -= 1;
            self.trial_baseline = None;
            self.cooldown_windows = COOLDOWN_WINDOWS;
            return VerifyControllerDecision::CpuDownshift {
                from,
                to: self.current,
            };
        }

        if self.cooldown_windows > 0 {
            self.cooldown_windows -= 1;
            return VerifyControllerDecision::Unchanged;
        }

        if sample_bytes < MIN_SAMPLE_BYTES || !has_pending {
            return VerifyControllerDecision::Unchanged;
        }

        if let Some(baseline) = self.trial_baseline {
            let gain_ratio = if baseline > f64::EPSILON {
                throughput / baseline
            } else {
                1.0
            };
            let gain_percent = (gain_ratio - 1.0) * 100.0;
            if gain_ratio < MIN_TRIAL_GAIN {
                let from = self.current;
                self.current = self.current.saturating_sub(1).max(1);
                self.trial_baseline = None;
                self.cooldown_windows = COOLDOWN_WINDOWS;
                return VerifyControllerDecision::TrialRolledBack {
                    from,
                    to: self.current,
                    gain_percent,
                    regression: gain_ratio < REGRESSION_THRESHOLD,
                };
            }

            self.trial_baseline = None;
            let accepted_workers = self.current;
            let next_trial = if cpu_percent < CPU_GROW_LIMIT && self.current < self.maximum {
                self.trial_baseline = Some(throughput);
                self.current += 1;
                Some(self.current)
            } else {
                None
            };
            return VerifyControllerDecision::TrialAccepted {
                workers: accepted_workers,
                gain_percent,
                next_trial,
            };
        }

        if cpu_percent < CPU_GROW_LIMIT && self.current < self.maximum {
            let from = self.current;
            self.trial_baseline = Some(throughput);
            self.current += 1;
            return VerifyControllerDecision::TrialStarted {
                from,
                to: self.current,
            };
        }

        VerifyControllerDecision::Unchanged
    }
}

fn verify_worker_limit(logical_cpu_count: usize) -> usize {
    (logical_cpu_count / 2).clamp(1, 6)
}

struct VerifyHashResult {
    key: String,
    matches: bool,
}

pub(crate) async fn find_hash_mismatches(
    tasks: Vec<VerifyHashTask>,
    cancel: CancellationToken,
    on_progress: VerifyProgressCallback,
) -> Result<Vec<GameFileManifestEntry>, AppError> {
    let mut files = HashMap::new();
    let generic = tasks
        .into_iter()
        .map(|task| {
            let key = task.file.path.clone();
            let size = task.file.size;
            let expected_sha256 = task.file.sha256.clone();
            files.insert(key.clone(), task.file);
            HashTask {
                key,
                size,
                expected_sha256,
                local_path: task.local_path,
            }
        })
        .collect();
    let mismatches = run_hash_checks(generic, cancel, on_progress).await?;
    Ok(mismatches
        .into_iter()
        .filter_map(|path| files.remove(&path))
        .collect())
}

pub(crate) async fn find_content_hash_mismatches(
    tasks: Vec<ContentHashTask>,
    cancel: CancellationToken,
    on_progress: VerifyProgressCallback,
) -> Result<Vec<String>, AppError> {
    let generic = tasks
        .into_iter()
        .map(|task| HashTask {
            key: task.path,
            size: task.size,
            expected_sha256: task.expected_sha256,
            local_path: task.local_path,
        })
        .collect();
    run_hash_checks(generic, cancel, on_progress).await
}

async fn run_hash_checks(
    tasks: Vec<HashTask>,
    cancel: CancellationToken,
    on_progress: VerifyProgressCallback,
) -> Result<Vec<String>, AppError> {
    if cancel.is_cancelled() {
        return Err(AppError::Canceled);
    }

    let total_bytes = tasks.iter().try_fold(0_u64, |total, task| {
        total
            .checked_add(task.size)
            .ok_or_else(|| AppError::InvalidData("verification byte count overflow".to_string()))
    })?;
    let logical_cpus = std::thread::available_parallelism().map_or(1, usize::from);
    let maximum = verify_worker_limit(logical_cpus);
    let mut controller = AdaptiveVerifyController::new(maximum);
    crate::logger::info(&format!(
        "integrity verification concurrency initialized: current=1 maximum={maximum}"
    ));

    let completed_bytes = Arc::new(AtomicU64::new(0));
    let child_cancel = cancel.child_token();
    let mut pending = VecDeque::from(tasks);
    let mut running = JoinSet::new();
    let mut mismatches = Vec::new();
    let started = Instant::now();
    let mut control_sample_bytes = 0_u64;
    let mut last_file = None;
    let mut system = System::new_all();

    let mut progress_interval = interval_at(TokioInstant::now() + PROGRESS_WINDOW, PROGRESS_WINDOW);
    progress_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut control_interval = interval_at(TokioInstant::now() + CONTROL_WINDOW, CONTROL_WINDOW);
    control_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        while running.len() < controller.current() {
            let Some(task) = pending.pop_front() else {
                break;
            };
            let task_cancel = child_cancel.clone();
            let task_completed = completed_bytes.clone();
            running.spawn(async move {
                let actual_hash =
                    sha256_file_tracked(&task.local_path, &task_cancel, task_completed.as_ref())
                        .await?;
                Ok(VerifyHashResult {
                    matches: actual_hash == task.expected_sha256,
                    key: task.key,
                })
            });
        }

        if running.is_empty() {
            break;
        }

        tokio::select! {
            _ = cancel.cancelled() => {
                child_cancel.cancel();
                drain_workers(&mut running).await;
                return Err(AppError::Canceled);
            }
            joined = running.join_next() => {
                let Some(joined) = joined else {
                    continue;
                };
                match joined {
                    Ok(Ok(result)) => {
                        last_file = Some(result.key.clone());
                        if !result.matches {
                            mismatches.push(result.key);
                        }
                    }
                    Ok(Err(error)) => {
                        child_cancel.cancel();
                        drain_workers(&mut running).await;
                        return Err(error);
                    }
                    Err(error) => {
                        child_cancel.cancel();
                        drain_workers(&mut running).await;
                        return Err(AppError::Unknown(format!(
                            "verification worker failed: {error}"
                        )));
                    }
                }
            }
            _ = progress_interval.tick() => {
                emit_progress(
                    &on_progress,
                    completed_bytes.load(Ordering::Relaxed),
                    total_bytes,
                    started,
                    last_file.clone(),
                );
            }
            _ = control_interval.tick() => {
                let completed = completed_bytes.load(Ordering::Relaxed);
                let sample_bytes = completed.saturating_sub(control_sample_bytes);
                control_sample_bytes = completed;
                system.refresh_cpu_usage();
                let throughput = sample_bytes as f64 / CONTROL_WINDOW.as_secs_f64();
                let decision = controller.observe_window(
                    sample_bytes,
                    throughput,
                    system.global_cpu_usage(),
                    !pending.is_empty(),
                );
                log_controller_decision(decision);
            }
        }
    }

    let completed = completed_bytes.load(Ordering::Relaxed);
    emit_progress(&on_progress, completed, total_bytes, started, last_file);
    Ok(mismatches)
}

async fn drain_workers(running: &mut JoinSet<Result<VerifyHashResult, AppError>>) {
    while running.join_next().await.is_some() {}
}

fn emit_progress(
    on_progress: &VerifyProgressCallback,
    completed_bytes: u64,
    total_bytes: u64,
    started: Instant,
    current_file: Option<String>,
) {
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    let speed_bytes_per_sec = completed_bytes as f64 / elapsed;
    let remaining = total_bytes.saturating_sub(completed_bytes);
    let time_remaining_sec =
        (speed_bytes_per_sec > f64::EPSILON).then_some(remaining as f64 / speed_bytes_per_sec);
    on_progress(VerifyHashProgress {
        completed_bytes: completed_bytes.min(total_bytes),
        total_bytes,
        speed_bytes_per_sec,
        time_remaining_sec,
        current_file,
    });
}

fn log_controller_decision(decision: VerifyControllerDecision) {
    match decision {
        VerifyControllerDecision::Unchanged => {}
        VerifyControllerDecision::TrialStarted { from, to } => crate::logger::info(&format!(
            "integrity verification concurrency trial started: {from} -> {to}"
        )),
        VerifyControllerDecision::TrialAccepted {
            workers,
            gain_percent,
            next_trial,
        } => crate::logger::info(&format!(
            "integrity verification concurrency trial accepted: workers={workers} gain={gain_percent:.1}% next_trial={next_trial:?}"
        )),
        VerifyControllerDecision::TrialRolledBack {
            from,
            to,
            gain_percent,
            regression,
        } => crate::logger::info(&format!(
            "integrity verification concurrency trial rolled back: {from} -> {to} gain={gain_percent:.1}% regression_over_10_percent={regression} cooldown=30s"
        )),
        VerifyControllerDecision::CpuDownshift { from, to } => crate::logger::info(&format!(
            "integrity verification concurrency reduced for CPU load: {from} -> {to} cooldown=30s"
        )),
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};
    use std::sync::Arc;

    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    use super::{
        find_content_hash_mismatches, find_hash_mismatches, verify_worker_limit,
        AdaptiveVerifyController, ContentHashTask, VerifyHashTask,
    };
    use crate::error::AppError;
    use crate::models::GameFileManifestEntry;

    const VALID_WINDOW: u64 = 64 * 1024 * 1024;

    #[tokio::test]
    async fn release_1_6_11_content_hashing_reports_progress_and_only_returns_mismatches() {
        let directory = tempdir().expect("temporary directory should be created");
        let correct_path = directory.path().join("correct.bin");
        let corrupt_path = directory.path().join("corrupt.bin");
        tokio::fs::write(&correct_path, b"correct").await.unwrap();
        tokio::fs::write(&corrupt_path, b"corrupt").await.unwrap();
        let correct_hash = hex::encode(Sha256::digest(b"correct"));
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = updates.clone();

        let mismatches = find_content_hash_mismatches(
            vec![
                ContentHashTask {
                    path: "correct.bin".to_string(),
                    size: 7,
                    expected_sha256: correct_hash.clone(),
                    local_path: correct_path,
                },
                ContentHashTask {
                    path: "corrupt.bin".to_string(),
                    size: 7,
                    expected_sha256: correct_hash,
                    local_path: corrupt_path,
                },
            ],
            CancellationToken::new(),
            Arc::new(move |progress| captured.lock().unwrap().push(progress.completed_bytes)),
        )
        .await
        .expect("content hashes should be checked");

        assert_eq!(mismatches, vec!["corrupt.bin"]);
        assert_eq!(updates.lock().unwrap().last().copied(), Some(14));
    }

    fn manifest_file(path: &str, size: u64, sha256: &str) -> GameFileManifestEntry {
        GameFileManifestEntry {
            path: path.to_string(),
            size,
            sha256: sha256.to_string(),
            url: "https://example.invalid/file".to_string(),
            excluded_from_hash_check: false,
            temporary: false,
        }
    }

    #[test]
    fn verify_controller_limits_workers_from_logical_cpu_count() {
        assert_eq!(verify_worker_limit(1), 1);
        assert_eq!(verify_worker_limit(8), 4);
        assert_eq!(verify_worker_limit(64), 6);
    }

    #[test]
    fn verify_controller_starts_a_trial_after_the_first_valid_baseline() {
        let mut controller = AdaptiveVerifyController::new(6);

        controller.observe_window(VALID_WINDOW, 100.0, 50.0, true);

        assert_eq!(controller.current(), 2);
    }

    #[test]
    fn verify_controller_accepts_a_five_percent_gain_and_chains_the_next_trial() {
        let mut controller = AdaptiveVerifyController::new(6);
        controller.observe_window(VALID_WINDOW, 100.0, 50.0, true);

        controller.observe_window(VALID_WINDOW, 105.0, 50.0, true);

        assert_eq!(controller.current(), 3);
    }

    #[test]
    fn verify_controller_rolls_back_a_trial_below_five_percent_gain() {
        let mut controller = AdaptiveVerifyController::new(6);
        controller.observe_window(VALID_WINDOW, 100.0, 50.0, true);

        controller.observe_window(VALID_WINDOW, 104.9, 50.0, true);

        assert_eq!(controller.current(), 1);
    }

    #[test]
    fn verify_controller_rolls_back_a_trial_regressing_more_than_ten_percent() {
        let mut controller = AdaptiveVerifyController::new(6);
        controller.observe_window(VALID_WINDOW, 100.0, 50.0, true);

        controller.observe_window(VALID_WINDOW, 89.0, 50.0, true);

        assert_eq!(controller.current(), 1);
    }

    #[test]
    fn verify_controller_downshifts_when_cpu_is_over_ninety_percent() {
        let mut controller = AdaptiveVerifyController::new(6);
        controller.observe_window(VALID_WINDOW, 100.0, 50.0, true);
        controller.observe_window(VALID_WINDOW, 105.0, 50.0, true);

        controller.observe_window(VALID_WINDOW, 105.0, 91.0, true);

        assert_eq!(controller.current(), 2);
    }

    #[test]
    fn verify_controller_ignores_samples_smaller_than_sixty_four_mebibytes() {
        let mut controller = AdaptiveVerifyController::new(6);

        controller.observe_window(VALID_WINDOW - 1, 500.0, 50.0, true);

        assert_eq!(controller.current(), 1);
    }

    #[test]
    fn verify_controller_waits_six_windows_after_a_rollback() {
        let mut controller = AdaptiveVerifyController::new(6);
        controller.observe_window(VALID_WINDOW, 100.0, 50.0, true);
        controller.observe_window(VALID_WINDOW, 90.0, 50.0, true);

        for _ in 0..6 {
            controller.observe_window(VALID_WINDOW, 200.0, 50.0, true);
            assert_eq!(controller.current(), 1);
        }
        controller.observe_window(VALID_WINDOW, 200.0, 50.0, true);

        assert_eq!(controller.current(), 2);
    }

    #[tokio::test]
    async fn verify_scheduler_collects_hash_mismatches() {
        let directory = tempdir().expect("temporary directory");
        let local_path = directory.path().join("client.bin");
        tokio::fs::write(&local_path, b"tampered")
            .await
            .expect("write fixture");
        let task = VerifyHashTask {
            file: manifest_file("client.bin", 8, &"0".repeat(64)),
            local_path,
        };

        let mismatches =
            find_hash_mismatches(vec![task], CancellationToken::new(), Arc::new(|_| {}))
                .await
                .expect("hashing should complete");

        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].path, "client.bin");
    }

    #[tokio::test]
    async fn verify_scheduler_honors_precancelled_operations() {
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = find_hash_mismatches(Vec::new(), cancel, Arc::new(|_| {})).await;

        assert!(matches!(result, Err(AppError::Canceled)));
    }
}
