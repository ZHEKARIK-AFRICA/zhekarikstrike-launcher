use std::path::PathBuf;

use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::services::install_service;
use crate::state::{AppState, OperationKind};

#[tauri::command]
pub async fn install_game(
    app: AppHandle,
    state: State<'_, AppState>,
    game_path: String,
) -> Result<(), AppError> {
    state.acquire_operation(OperationKind::Installing).await?;
    let cancel = CancellationToken::new();
    *state.install_cancel_token.lock().await = Some(cancel.clone());

    let result = install_service::install_game(app.clone(), PathBuf::from(game_path), cancel).await;

    *state.install_cancel_token.lock().await = None;
    state.release_operation().await;

    match result {
        Ok(()) => {
            app.emit("install-complete", ())?;
            app.emit("install-finalized", ())?;
            Ok(())
        }
        Err(AppError::Canceled) => {
            app.emit("install-canceled", ())?;
            app.emit("install-finalized", ())?;
            Err(AppError::Canceled)
        }
        Err(error) => {
            app.emit("install-error", error.frontend_error())?;
            app.emit("install-finalized", ())?;
            Err(error)
        }
    }
}

#[tauri::command]
pub async fn cancel_install(state: State<'_, AppState>) -> Result<bool, AppError> {
    if let Some(token) = state.install_cancel_token.lock().await.take() {
        token.cancel();
        return Ok(true);
    }
    Ok(false)
}
