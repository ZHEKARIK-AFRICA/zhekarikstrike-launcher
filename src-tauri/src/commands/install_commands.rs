use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::error::AppError;
use crate::models::{validated_operation_id, ProgressEmitter, ProgressStage};
use crate::services::{config_service, content_journal_service, install_service};
use crate::state::{AppState, CancellationSlot, OperationKind};

#[tauri::command]
pub async fn install_game(
    app: AppHandle,
    state: State<'_, AppState>,
    game_path: String,
    operation_id: Option<String>,
) -> Result<(), AppError> {
    let operation_id = validated_operation_id(operation_id)?;
    let lease =
        state.begin_operation(OperationKind::Installing, Some(CancellationSlot::Install))?;
    let cancel = lease
        .cancellation_token()
        .expect("install operations always have a cancellation token");

    #[cfg(feature = "e2e")]
    {
        let _ = app;
        if game_path.to_ascii_lowercase().contains("error") {
            return Err(AppError::Network(
                "native install fixture failed".to_string(),
            ));
        }
        if game_path.to_ascii_lowercase().contains("cancel") {
            cancel.cancelled().await;
            return Err(AppError::Canceled);
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        return Ok(());
    }

    #[cfg(not(feature = "e2e"))]
    install_service::install_game(app, PathBuf::from(game_path), cancel, operation_id).await
}

#[tauri::command]
pub async fn cancel_install(state: State<'_, AppState>) -> Result<bool, AppError> {
    Ok(state.cancel_install())
}

#[derive(Debug, Serialize)]
pub struct PendingInstallRecovery {
    pub recovered: bool,
}

#[tauri::command]
pub async fn recover_pending_install(
    app: AppHandle,
    state: State<'_, AppState>,
    operation_id: Option<String>,
) -> Result<PendingInstallRecovery, AppError> {
    let operation_id = validated_operation_id(operation_id)?;
    #[cfg(feature = "e2e")]
    {
        let _ = (app, state, operation_id);
        return Ok(PendingInstallRecovery { recovered: false });
    }

    #[cfg(not(feature = "e2e"))]
    {
        let Some(game_path) = config_service::get_game_path().await? else {
            return Ok(PendingInstallRecovery { recovered: false });
        };
        if !tokio::fs::try_exists(content_journal_service::journal_path(&game_path)).await? {
            return Ok(PendingInstallRecovery { recovered: false });
        }

        let _lease = state.begin_operation(OperationKind::RecoveringContent, None)?;
        let progress = ProgressEmitter::new(app, "recovery-progress", operation_id);
        progress.emit_stage(
            ProgressStage::Cleanup,
            Some(0.0),
            Some("recovering interrupted content transaction".to_string()),
        )?;
        let recovered = content_journal_service::recover_pending_content(&game_path).await?;
        progress.emit_stage(ProgressStage::Cleanup, Some(100.0), None)?;
        Ok(PendingInstallRecovery { recovered })
    }
}
