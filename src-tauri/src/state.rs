use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard};

use discord_rich_presence::DiscordIpcClient;
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, FrontendError};
use crate::models::{GameProcessState, VerifiedLauncherUpdate};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum OperationKind {
    #[default]
    Idle,
    Installing,
    InstallingPrerequisites,
    Verifying,
    UpdatingGame,
    LaunchingGame,
    UpdatingLauncher,
    RecoveringContent,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct OperationState {
    pub kind: OperationKind,
}

#[derive(Default)]
pub struct DiscordRpcState {
    pub client: Option<DiscordIpcClient>,
    pub started_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CurrentState {
    #[serde(rename = "ProcessInProgress")]
    pub process_in_progress_legacy: bool,
    #[serde(rename = "processInProgress")]
    pub process_in_progress: bool,
    #[serde(rename = "verificationInProgress")]
    pub verification_in_progress: bool,
    pub operation: OperationKind,
    #[serde(rename = "launcherUpdateReady")]
    pub launcher_update_ready: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PrerequisiteOutcome {
    #[default]
    None,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrerequisiteTerminalResult {
    pub ready: bool,
    pub installed: Vec<String>,
    pub already_present: Vec<String>,
    pub restart_recommended: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PrerequisiteOperationState {
    pub active: bool,
    pub operation_id: Option<String>,
    pub stage: Option<String>,
    pub component_id: Option<String>,
    pub progress: Option<f64>,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub restart_recommended: bool,
    pub outcome: PrerequisiteOutcome,
    pub result: Option<PrerequisiteTerminalResult>,
    pub error: Option<FrontendError>,
}

#[derive(Clone)]
pub struct AppState {
    install_cancel_token: Arc<StdMutex<Option<CancellationToken>>>,
    verify_cancel_token: Arc<StdMutex<Option<CancellationToken>>>,
    launcher_update_cancel_token: Arc<StdMutex<Option<CancellationToken>>>,
    prerequisite_cancel_token: Arc<StdMutex<Option<CancellationToken>>>,
    prerequisite_state: Arc<StdMutex<PrerequisiteOperationState>>,
    launcher_update: Arc<StdMutex<Option<VerifiedLauncherUpdate>>>,
    pub file_patch_cancel_token: Arc<Mutex<Option<CancellationToken>>>,
    pub process_state: Arc<RwLock<GameProcessState>>,
    pub discord_rpc_state: Arc<Mutex<DiscordRpcState>>,
    pub copied_pure_files: Arc<Mutex<Vec<PathBuf>>>,
    pub copied_game_files: Arc<Mutex<Vec<PathBuf>>>,
    operation_lock: Arc<StdMutex<OperationState>>,
    pub cleanup_lock: Arc<Mutex<()>>,
    shutdown_started: Arc<AtomicBool>,
    close_confirmation_pending: Arc<AtomicBool>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            install_cancel_token: Arc::new(StdMutex::new(None)),
            verify_cancel_token: Arc::new(StdMutex::new(None)),
            launcher_update_cancel_token: Arc::new(StdMutex::new(None)),
            prerequisite_cancel_token: Arc::new(StdMutex::new(None)),
            prerequisite_state: Arc::new(StdMutex::new(PrerequisiteOperationState::default())),
            launcher_update: Arc::new(StdMutex::new(None)),
            file_patch_cancel_token: Arc::new(Mutex::new(None)),
            process_state: Arc::new(RwLock::new(GameProcessState::default())),
            discord_rpc_state: Arc::new(Mutex::new(DiscordRpcState::default())),
            copied_pure_files: Arc::new(Mutex::new(Vec::new())),
            copied_game_files: Arc::new(Mutex::new(Vec::new())),
            operation_lock: Arc::new(StdMutex::new(OperationState::default())),
            cleanup_lock: Arc::new(Mutex::new(())),
            shutdown_started: Arc::new(AtomicBool::new(false)),
            close_confirmation_pending: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn begin_operation(
        &self,
        kind: OperationKind,
        cancellation_slot: Option<CancellationSlot>,
    ) -> Result<OperationLease, AppError> {
        let mut state = lock_unpoisoned(&self.operation_lock);
        if state.kind != OperationKind::Idle {
            return Err(AppError::OperationInProgress(format!("{:?}", state.kind)));
        }
        state.kind = kind;
        drop(state);

        let cancel_slot = cancellation_slot.map(|slot| match slot {
            CancellationSlot::Install => self.install_cancel_token.clone(),
            CancellationSlot::Verify => self.verify_cancel_token.clone(),
            CancellationSlot::LauncherUpdate => self.launcher_update_cancel_token.clone(),
            CancellationSlot::Prerequisite => self.prerequisite_cancel_token.clone(),
        });
        let cancellation_token = cancel_slot.as_ref().map(|slot| {
            let token = CancellationToken::new();
            *lock_unpoisoned(slot) = Some(token.clone());
            token
        });

        Ok(OperationLease {
            operation_state: self.operation_lock.clone(),
            cancel_slot,
            cancellation_token,
        })
    }

    pub fn cancel_install(&self) -> bool {
        cancel_slot(&self.install_cancel_token)
    }

    pub fn cancel_verify(&self) -> bool {
        cancel_slot(&self.verify_cancel_token)
    }

    pub fn cancel_launcher_update(&self) -> bool {
        cancel_slot(&self.launcher_update_cancel_token)
    }

    pub fn cancel_prerequisites(&self) -> bool {
        cancel_slot(&self.prerequisite_cancel_token)
    }

    pub fn consume_prerequisite_state(&self) -> PrerequisiteOperationState {
        let mut state = lock_unpoisoned(&self.prerequisite_state);
        let snapshot = state.clone();
        if matches!(
            snapshot.outcome,
            PrerequisiteOutcome::Succeeded
                | PrerequisiteOutcome::Failed
                | PrerequisiteOutcome::Canceled
        ) {
            *state = PrerequisiteOperationState::default();
        }
        snapshot
    }

    pub fn begin_prerequisite_operation(&self, operation_id: &str) {
        *lock_unpoisoned(&self.prerequisite_state) = PrerequisiteOperationState {
            active: true,
            operation_id: Some(operation_id.to_string()),
            stage: Some("detecting".into()),
            progress: Some(0.0),
            outcome: PrerequisiteOutcome::Running,
            ..PrerequisiteOperationState::default()
        };
    }

    pub fn update_prerequisite_state(&self, snapshot: PrerequisiteOperationState) {
        let mut state = lock_unpoisoned(&self.prerequisite_state);
        if state.outcome == PrerequisiteOutcome::Running
            && state.operation_id == snapshot.operation_id
        {
            *state = snapshot;
        }
    }

    pub fn finish_prerequisite_success(
        &self,
        operation_id: &str,
        result: PrerequisiteTerminalResult,
    ) {
        self.finish_prerequisite(
            operation_id,
            PrerequisiteOutcome::Succeeded,
            Some(result),
            None,
        );
    }

    pub fn finish_prerequisite_failure(&self, operation_id: &str, error: FrontendError) {
        self.finish_prerequisite(operation_id, PrerequisiteOutcome::Failed, None, Some(error));
    }

    pub fn finish_prerequisite_canceled(&self, operation_id: &str, error: FrontendError) {
        self.finish_prerequisite(
            operation_id,
            PrerequisiteOutcome::Canceled,
            None,
            Some(error),
        );
    }

    fn finish_prerequisite(
        &self,
        operation_id: &str,
        outcome: PrerequisiteOutcome,
        result: Option<PrerequisiteTerminalResult>,
        error: Option<FrontendError>,
    ) {
        let mut state = lock_unpoisoned(&self.prerequisite_state);
        if state.operation_id.as_deref() != Some(operation_id)
            || state.outcome != PrerequisiteOutcome::Running
        {
            return;
        }
        state.active = false;
        state.outcome = outcome;
        state.result = result;
        state.error = error;
    }

    pub fn cancel_active_operation(&self) -> bool {
        match self.current_state().operation {
            OperationKind::Installing => self.cancel_install(),
            OperationKind::Verifying | OperationKind::UpdatingGame => self.cancel_verify(),
            OperationKind::UpdatingLauncher => self.cancel_launcher_update(),
            OperationKind::InstallingPrerequisites => self.cancel_prerequisites(),
            OperationKind::Idle
            | OperationKind::LaunchingGame
            | OperationKind::RecoveringContent => false,
        }
    }

    pub fn current_state(&self) -> CurrentState {
        let operation = lock_unpoisoned(&self.operation_lock).kind;
        let process_in_progress = operation != OperationKind::Idle;
        let launcher_update_ready = lock_unpoisoned(&self.launcher_update).is_some();
        CurrentState {
            process_in_progress_legacy: process_in_progress,
            process_in_progress,
            verification_in_progress: matches!(
                operation,
                OperationKind::Verifying | OperationKind::UpdatingGame
            ),
            operation,
            launcher_update_ready,
        }
    }

    pub fn set_launcher_update(&self, update: VerifiedLauncherUpdate) {
        *lock_unpoisoned(&self.launcher_update) = Some(update);
    }

    pub fn launcher_update(&self) -> Option<VerifiedLauncherUpdate> {
        lock_unpoisoned(&self.launcher_update).clone()
    }

    pub fn clear_launcher_update(&self) {
        *lock_unpoisoned(&self.launcher_update) = None;
    }

    pub fn begin_shutdown(&self) -> bool {
        self.shutdown_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn shutdown_started(&self) -> bool {
        self.shutdown_started.load(Ordering::Acquire)
    }

    pub fn begin_close_confirmation(&self) -> bool {
        if self.shutdown_started() {
            return false;
        }
        self.close_confirmation_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn cancel_close_confirmation(&self) -> bool {
        self.close_confirmation_pending
            .swap(false, Ordering::AcqRel)
    }

    pub fn close_confirmation_pending(&self) -> bool {
        self.close_confirmation_pending.load(Ordering::Acquire)
    }

    pub fn confirm_close(&self) -> bool {
        if self
            .close_confirmation_pending
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        self.begin_shutdown()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CancellationSlot {
    Install,
    Verify,
    LauncherUpdate,
    Prerequisite,
}

#[derive(Debug)]
pub struct OperationLease {
    operation_state: Arc<StdMutex<OperationState>>,
    cancel_slot: Option<Arc<StdMutex<Option<CancellationToken>>>>,
    cancellation_token: Option<CancellationToken>,
}

impl OperationLease {
    pub fn cancellation_token(&self) -> Option<CancellationToken> {
        self.cancellation_token.clone()
    }
}

impl Drop for OperationLease {
    fn drop(&mut self) {
        if let Some(slot) = self.cancel_slot.as_ref() {
            *lock_unpoisoned(slot) = None;
        }
        lock_unpoisoned(&self.operation_state).kind = OperationKind::Idle;
    }
}

fn cancel_slot(slot: &StdMutex<Option<CancellationToken>>) -> bool {
    if let Some(token) = lock_unpoisoned(slot).take() {
        token.cancel();
        return true;
    }
    false
}

fn lock_unpoisoned<T>(mutex: &StdMutex<T>) -> StdMutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{
        AppState, CancellationSlot, OperationKind, PrerequisiteOutcome, PrerequisiteTerminalResult,
    };
    use crate::error::AppError;

    #[test]
    fn operation_lease_releases_state_when_dropped() {
        let state = AppState::new();
        {
            let _lease = state
                .begin_operation(OperationKind::Verifying, Some(CancellationSlot::Verify))
                .expect("first operation should start");
            assert_eq!(state.current_state().operation, OperationKind::Verifying);
        }
        assert_eq!(state.current_state().operation, OperationKind::Idle);
    }

    #[test]
    fn operation_lease_rejects_a_concurrent_operation() {
        let state = AppState::new();
        let _lease = state
            .begin_operation(OperationKind::Installing, Some(CancellationSlot::Install))
            .expect("first operation should start");

        let error = state
            .begin_operation(OperationKind::Verifying, Some(CancellationSlot::Verify))
            .expect_err("second operation must be rejected");

        assert!(matches!(error, AppError::OperationInProgress(_)));
    }

    #[test]
    fn cancel_method_cancels_the_lease_token() {
        let state = AppState::new();
        let lease = state
            .begin_operation(OperationKind::Verifying, Some(CancellationSlot::Verify))
            .expect("verification should start");
        let token = lease
            .cancellation_token()
            .expect("verification must expose a cancellation token");

        assert!(state.cancel_verify());
        assert!(token.is_cancelled());
        assert!(!state.cancel_verify());
    }

    #[test]
    fn early_return_still_releases_the_operation() {
        fn fail_after_acquire(state: &AppState) -> Result<(), AppError> {
            let _lease = state.begin_operation(OperationKind::LaunchingGame, None)?;
            Err(AppError::GamePathNotSet)
        }

        let state = AppState::new();
        assert!(matches!(
            fail_after_acquire(&state),
            Err(AppError::GamePathNotSet)
        ));
        assert_eq!(state.current_state().operation, OperationKind::Idle);
    }

    #[test]
    fn release_1_6_11_shutdown_can_only_begin_once() {
        let state = AppState::new();
        assert!(state.begin_shutdown());
        assert!(!state.begin_shutdown());
    }

    #[test]
    fn release_1_6_12_close_confirmation_is_one_shot_and_cancelable() {
        let state = AppState::new();
        assert!(state.begin_close_confirmation());
        assert!(!state.begin_close_confirmation());
        assert!(state.cancel_close_confirmation());
        assert!(state.begin_close_confirmation());
        assert!(state.close_confirmation_pending());
        assert!(state.confirm_close());
        assert!(!state.close_confirmation_pending());
        assert!(!state.confirm_close());
    }

    #[test]
    fn release_1_6_12_shutdown_cancels_launcher_updates() {
        let state = AppState::new();
        let lease = state
            .begin_operation(
                OperationKind::UpdatingLauncher,
                Some(CancellationSlot::LauncherUpdate),
            )
            .expect("launcher update should start");
        let token = lease
            .cancellation_token()
            .expect("launcher update should expose a token");

        assert!(state.cancel_active_operation());
        assert!(token.is_cancelled());
    }

    #[test]
    fn release_1_6_13_prerequisite_operation_is_exclusive_and_cancelable() {
        let state = AppState::new();
        let lease = state
            .begin_operation(
                OperationKind::InstallingPrerequisites,
                Some(CancellationSlot::Prerequisite),
            )
            .expect("prerequisite operation should start");
        let token = lease
            .cancellation_token()
            .expect("prerequisite downloads expose cancellation");

        assert_eq!(
            state.current_state().operation,
            OperationKind::InstallingPrerequisites
        );
        assert!(state.cancel_active_operation());
        assert!(token.is_cancelled());
    }

    #[test]
    fn prerequisite_terminal_outcome_is_keyed_and_consumed_once() {
        let state = AppState::new();
        state.begin_prerequisite_operation("operation-1");
        state.finish_prerequisite_failure(
            "operation-1",
            AppError::PrerequisiteInstall("exit 1603".into()).frontend_error(),
        );

        let terminal = state.consume_prerequisite_state();
        assert_eq!(terminal.operation_id.as_deref(), Some("operation-1"));
        assert_eq!(terminal.outcome, PrerequisiteOutcome::Failed);
        assert_eq!(
            terminal.error.as_ref().map(|error| error.code.as_str()),
            Some("prerequisite_install_failed")
        );
        assert_eq!(
            state.consume_prerequisite_state().outcome,
            PrerequisiteOutcome::None
        );
    }

    #[test]
    fn prerequisite_success_and_cancel_are_distinct_terminal_outcomes() {
        let state = AppState::new();
        state.begin_prerequisite_operation("success");
        state.finish_prerequisite_success(
            "success",
            PrerequisiteTerminalResult {
                ready: true,
                installed: vec!["vc2010-sp1-x86".into()],
                already_present: Vec::new(),
                restart_recommended: false,
            },
        );
        assert_eq!(
            state.consume_prerequisite_state().outcome,
            PrerequisiteOutcome::Succeeded
        );

        state.begin_prerequisite_operation("cancel");
        state.finish_prerequisite_canceled("cancel", AppError::Canceled.frontend_error());
        assert_eq!(
            state.consume_prerequisite_state().outcome,
            PrerequisiteOutcome::Canceled
        );
    }
}
