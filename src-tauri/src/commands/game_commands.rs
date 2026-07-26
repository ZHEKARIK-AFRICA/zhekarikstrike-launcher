use tauri::{AppHandle, Emitter, State};

use crate::error::AppError;
use crate::services::{config_service, game_process_service};
use crate::state::{AppState, OperationKind};

#[tauri::command]
pub async fn launch_game(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    state
        .acquire_operation(OperationKind::LaunchingGame)
        .await?;
    let game_path = config_service::get_game_path()
        .await?
        .ok_or(AppError::GamePathNotSet)?;
    let result = game_process_service::launch_game(app.clone(), state.inner(), game_path).await;
    state.release_operation().await;

    if let Err(error) = result.as_ref() {
        app.emit("game-error", error.frontend_error())?;
    }

    result
}

#[tauri::command]
pub async fn stop_game(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    let pid = state.process_state.read().await.pid;
    if let Some(pid) = pid {
        game_process_service::stop_game_process(pid).await?;
        app.emit("game-closed", ())?;
    }
    Ok(())
}
