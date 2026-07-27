use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::error::AppError;
use crate::services::api_client::ApiClient;
use crate::services::config_service;
use crate::services::content_install_service;
use crate::services::manifest_service::{update_mode_for_version, VerifyMode};
use crate::services::verify_service;
use crate::state::{AppState, CancellationSlot, OperationKind};

#[tauri::command]
pub async fn verify_files(
    app: AppHandle,
    state: State<'_, AppState>,
    check_all_files: bool,
) -> Result<(), AppError> {
    #[cfg(feature = "e2e")]
    {
        let lease =
            state.begin_operation(OperationKind::Verifying, Some(CancellationSlot::Verify))?;
        let cancel = lease
            .cancellation_token()
            .expect("verify operations always have a cancellation token");
        let _ = (app, check_all_files);
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(25)) => Ok(()),
            _ = cancel.cancelled() => Err(AppError::Canceled),
        }
    }

    #[cfg(not(feature = "e2e"))]
    {
        let mode = if check_all_files {
            VerifyMode::Full
        } else {
            VerifyMode::AdditionalOnly
        };
        run_verify(app, state, mode, OperationKind::Verifying).await
    }
}

#[tauri::command]
pub async fn update_game(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    #[cfg(feature = "e2e")]
    {
        let _lease = state.begin_operation(OperationKind::UpdatingGame, None)?;
        let _ = app;
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        return Ok(());
    }

    #[cfg(not(feature = "e2e"))]
    {
        let lease =
            state.begin_operation(OperationKind::UpdatingGame, Some(CancellationSlot::Verify))?;
        let cancel = lease
            .cancellation_token()
            .expect("game update operations always have a cancellation token");
        let game_path = config_service::get_game_path()
            .await?
            .ok_or(AppError::GamePathNotSet)?;
        let api = ApiClient::new()?;
        if let Some(manifest) = api.get_content_manifest().await? {
            return content_install_service::install_or_update_content(
                app,
                game_path,
                api,
                manifest,
                cancel,
                "verify-progress",
                Uuid::new_v4().to_string(),
            )
            .await;
        }
        let current = config_service::get_game_version().await?;
        verify_service::verify_game_files(
            app,
            game_path,
            update_mode_for_version(&current),
            cancel,
            "verify-progress",
            Uuid::new_v4().to_string(),
        )
        .await
    }
}

#[tauri::command]
pub async fn cancel_verify(state: State<'_, AppState>) -> Result<bool, AppError> {
    Ok(state.cancel_verify())
}

async fn run_verify(
    app: AppHandle,
    state: State<'_, AppState>,
    mode: VerifyMode,
    operation: OperationKind,
) -> Result<(), AppError> {
    let lease = state.begin_operation(operation, Some(CancellationSlot::Verify))?;
    let cancel = lease
        .cancellation_token()
        .expect("verify operations always have a cancellation token");

    let game_path = config_service::get_game_path()
        .await?
        .ok_or(AppError::GamePathNotSet)?;

    verify_service::verify_game_files(
        app.clone(),
        game_path,
        mode,
        cancel,
        "verify-progress",
        Uuid::new_v4().to_string(),
    )
    .await
    .map(|_| ())
}
