use tauri::{AppHandle, State};

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
    let _lease = state.begin_operation(OperationKind::UpdatingLauncher, None)?;
    state.clear_launcher_update();

    #[cfg(feature = "e2e")]
    {
        let _ = app;
        return Err(AppError::InvalidData(
            "tampered native artifact".to_string(),
        ));
    }

    #[cfg(not(feature = "e2e"))]
    {
        let update = launcher_update_service::download_launcher_update(app).await?;
        state.set_launcher_update(update);
        Ok(())
    }
}

#[tauri::command]
pub async fn apply_launcher_update(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let update = state
        .launcher_update()
        .ok_or_else(|| AppError::InvalidData("no verified launcher update is ready".to_string()))?;
    launcher_update_service::apply_launcher_update(app, &update).await
}
