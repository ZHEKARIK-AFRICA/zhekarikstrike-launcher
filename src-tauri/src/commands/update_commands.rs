use tauri::{AppHandle, State};

use crate::error::AppError;
use crate::models::{validated_operation_id, LauncherUpdateStatus};
use crate::services::launcher_update_service;
use crate::state::{AppState, CancellationSlot, OperationKind};

#[tauri::command]
pub async fn check_launcher_update() -> Result<LauncherUpdateStatus, AppError> {
    launcher_update_service::check_launcher_update(env!("CARGO_PKG_VERSION")).await
}

#[tauri::command]
pub async fn download_launcher_update(
    app: AppHandle,
    state: State<'_, AppState>,
    operation_id: Option<String>,
) -> Result<(), AppError> {
    let operation_id = validated_operation_id(operation_id)?;
    let lease = state.begin_operation(
        OperationKind::UpdatingLauncher,
        Some(CancellationSlot::LauncherUpdate),
    )?;
    let cancel = lease
        .cancellation_token()
        .expect("launcher update operations always have a cancellation token");
    state.clear_launcher_update();

    #[cfg(feature = "e2e")]
    {
        let _ = (app, cancel, operation_id);
        return Err(AppError::InvalidData(
            "tampered native artifact".to_string(),
        ));
    }

    #[cfg(not(feature = "e2e"))]
    {
        let update =
            launcher_update_service::download_launcher_update(app, cancel, operation_id).await?;
        state.set_launcher_update(update);
        Ok(())
    }
}

#[tauri::command]
pub async fn apply_launcher_update(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let _lease = state.begin_operation(OperationKind::UpdatingLauncher, None)?;
    let update = state
        .launcher_update()
        .ok_or_else(|| AppError::InvalidData("no verified launcher update is ready".to_string()))?;
    launcher_update_service::apply_launcher_update(app, &update).await
}
