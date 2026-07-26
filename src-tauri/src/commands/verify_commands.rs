use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::AppError;
use crate::services::config_service;
use crate::services::manifest_service::VerifyMode;
use crate::services::verify_service;
use crate::state::{AppState, OperationKind};

#[tauri::command]
pub async fn verify_files(
    app: AppHandle,
    state: State<'_, AppState>,
    check_all_files: bool,
) -> Result<(), AppError> {
    let mode = if check_all_files {
        VerifyMode::Full
    } else {
        VerifyMode::AdditionalOnly
    };
    run_verify(app, state, mode, OperationKind::Verifying).await
}

#[tauri::command]
pub async fn update_game(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    let current = config_service::get_game_version().await?;
    run_verify(
        app,
        state,
        VerifyMode::UpdateFromVersion(current),
        OperationKind::UpdatingGame,
    )
    .await
}

#[tauri::command]
pub async fn cancel_verify(state: State<'_, AppState>) -> Result<bool, AppError> {
    if let Some(token) = state.verify_cancel_token.lock().await.take() {
        token.cancel();
        return Ok(true);
    }
    Ok(false)
}

async fn run_verify(
    app: AppHandle,
    state: State<'_, AppState>,
    mode: VerifyMode,
    operation: OperationKind,
) -> Result<(), AppError> {
    state.acquire_operation(operation).await?;
    let cancel = CancellationToken::new();
    *state.verify_cancel_token.lock().await = Some(cancel.clone());

    let game_path = config_service::get_game_path()
        .await?
        .ok_or(AppError::GamePathNotSet)?;

    let result = verify_service::verify_game_files(
        app.clone(),
        game_path,
        mode,
        cancel,
        "verify-progress",
        Uuid::new_v4().to_string(),
    )
    .await;

    *state.verify_cancel_token.lock().await = None;
    state.release_operation().await;

    match result {
        Ok(_) => {
            app.emit("verify-complete", ())?;
            Ok(())
        }
        Err(AppError::Canceled) => {
            app.emit("verify-canceled", ())?;
            Err(AppError::Canceled)
        }
        Err(error) => {
            app.emit("verify-error", error.frontend_error())?;
            Err(error)
        }
    }
}
