use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard};

use discord_rich_presence::DiscordIpcClient;
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::models::GameProcessState;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum OperationKind {
    #[default]
    Idle,
    Installing,
    Verifying,
    UpdatingGame,
    LaunchingGame,
    UpdatingLauncher,
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
}

#[derive(Clone)]
pub struct AppState {
    install_cancel_token: Arc<StdMutex<Option<CancellationToken>>>,
    verify_cancel_token: Arc<StdMutex<Option<CancellationToken>>>,
    launcher_update_path: Arc<StdMutex<Option<PathBuf>>>,
    pub file_patch_cancel_token: Arc<Mutex<Option<CancellationToken>>>,
    pub process_state: Arc<RwLock<GameProcessState>>,
    pub discord_rpc_state: Arc<Mutex<DiscordRpcState>>,
    pub copied_pure_files: Arc<Mutex<Vec<PathBuf>>>,
    pub copied_game_files: Arc<Mutex<Vec<PathBuf>>>,
    operation_lock: Arc<StdMutex<OperationState>>,
    pub cleanup_lock: Arc<Mutex<()>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            install_cancel_token: Arc::new(StdMutex::new(None)),
            verify_cancel_token: Arc::new(StdMutex::new(None)),
            launcher_update_path: Arc::new(StdMutex::new(None)),
            file_patch_cancel_token: Arc::new(Mutex::new(None)),
            process_state: Arc::new(RwLock::new(GameProcessState::default())),
            discord_rpc_state: Arc::new(Mutex::new(DiscordRpcState::default())),
            copied_pure_files: Arc::new(Mutex::new(Vec::new())),
            copied_game_files: Arc::new(Mutex::new(Vec::new())),
            operation_lock: Arc::new(StdMutex::new(OperationState::default())),
            cleanup_lock: Arc::new(Mutex::new(())),
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

    pub fn current_state(&self) -> CurrentState {
        let operation = lock_unpoisoned(&self.operation_lock).kind;
        let process_in_progress = operation != OperationKind::Idle;
        CurrentState {
            process_in_progress_legacy: process_in_progress,
            process_in_progress,
            verification_in_progress: matches!(
                operation,
                OperationKind::Verifying | OperationKind::UpdatingGame
            ),
            operation,
        }
    }

    pub fn set_launcher_update_path(&self, path: PathBuf) {
        *lock_unpoisoned(&self.launcher_update_path) = Some(path);
    }

    pub fn launcher_update_path(&self) -> Option<PathBuf> {
        lock_unpoisoned(&self.launcher_update_path).clone()
    }

    pub fn clear_launcher_update_path(&self) {
        *lock_unpoisoned(&self.launcher_update_path) = None;
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CancellationSlot {
    Install,
    Verify,
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
    use super::{AppState, CancellationSlot, OperationKind};
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
}
