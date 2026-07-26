use std::path::PathBuf;
use std::sync::Arc;

use discord_rich_presence::DiscordIpcClient;
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::models::{GameProcessState, LauncherConfig};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OperationKind {
    Idle,
    Installing,
    Verifying,
    UpdatingGame,
    LaunchingGame,
    UpdatingLauncher,
}

impl Default for OperationKind {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct OperationState {
    pub kind: OperationKind,
}

pub struct DiscordRpcState {
    pub client: Option<DiscordIpcClient>,
    pub started_at: Option<i64>,
}

impl Default for DiscordRpcState {
    fn default() -> Self {
        Self {
            client: None,
            started_at: None,
        }
    }
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
    pub config: Arc<RwLock<LauncherConfig>>,
    pub install_cancel_token: Arc<Mutex<Option<CancellationToken>>>,
    pub verify_cancel_token: Arc<Mutex<Option<CancellationToken>>>,
    pub file_patch_cancel_token: Arc<Mutex<Option<CancellationToken>>>,
    pub process_state: Arc<RwLock<GameProcessState>>,
    pub discord_rpc_state: Arc<Mutex<DiscordRpcState>>,
    pub copied_pure_files: Arc<Mutex<Vec<PathBuf>>>,
    pub copied_game_files: Arc<Mutex<Vec<PathBuf>>>,
    pub operation_lock: Arc<Mutex<OperationState>>,
    pub cleanup_lock: Arc<Mutex<()>>,
}

impl AppState {
    pub fn new(config: LauncherConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            install_cancel_token: Arc::new(Mutex::new(None)),
            verify_cancel_token: Arc::new(Mutex::new(None)),
            file_patch_cancel_token: Arc::new(Mutex::new(None)),
            process_state: Arc::new(RwLock::new(GameProcessState::default())),
            discord_rpc_state: Arc::new(Mutex::new(DiscordRpcState::default())),
            copied_pure_files: Arc::new(Mutex::new(Vec::new())),
            copied_game_files: Arc::new(Mutex::new(Vec::new())),
            operation_lock: Arc::new(Mutex::new(OperationState::default())),
            cleanup_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn acquire_operation(&self, kind: OperationKind) -> Result<(), AppError> {
        let mut state = self.operation_lock.lock().await;
        if state.kind != OperationKind::Idle {
            return Err(AppError::OperationInProgress(format!("{:?}", state.kind)));
        }
        state.kind = kind;
        Ok(())
    }

    pub async fn release_operation(&self) {
        let mut state = self.operation_lock.lock().await;
        state.kind = OperationKind::Idle;
    }

    pub async fn current_state(&self) -> CurrentState {
        let operation = self.operation_lock.lock().await.kind.clone();
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
}
