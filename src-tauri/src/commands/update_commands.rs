use tauri::{AppHandle, Emitter, State};

use crate::error::AppError;
use crate::models::LauncherUpdateStatus;
use crate::services::launcher_update_service;
use crate::state::{AppState, OperationKind};

#[tauri::command]
pub async fn check_launcher_update() -> Result<LauncherUpdateStatus, AppError> {
    launcher_update_service::check_launcher_update(env!("CARGO_PKG_VERSION")).await
}

#[tauri::command]
pub async fn download_launcher_update(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    state
        .acquire_operation(OperationKind::UpdatingLauncher)
        .await?;
    let result = launcher_update_service::download_launcher_update(app.clone()).await;
    state.release_operation().await;

    match result {
        Ok(_) => {
            app.emit("launcher-update-ready", ())?;
            Ok(())
        }
        Err(error) => {
            app.emit("launcher-update-error", error.frontend_error())?;
            Err(error)
        }
    }
}

#[tauri::command]
pub async fn apply_launcher_update(app: AppHandle) -> Result<(), AppError> {
    launcher_update_service::apply_launcher_update(app).await
}
