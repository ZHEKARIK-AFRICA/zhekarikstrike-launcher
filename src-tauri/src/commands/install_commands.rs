use std::path::PathBuf;

use tauri::{AppHandle, State};

use crate::error::AppError;
use crate::services::install_service;
use crate::state::{AppState, CancellationSlot, OperationKind};

#[tauri::command]
pub async fn install_game(
    app: AppHandle,
    state: State<'_, AppState>,
    game_path: String,
) -> Result<(), AppError> {
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
    install_service::install_game(app, PathBuf::from(game_path), cancel).await
}

#[tauri::command]
pub async fn cancel_install(state: State<'_, AppState>) -> Result<bool, AppError> {
    Ok(state.cancel_install())
}
