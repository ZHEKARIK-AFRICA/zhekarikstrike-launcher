use tauri::{AppHandle, Emitter, State};

use crate::error::AppError;
use crate::services::{config_service, game_process_service};
use crate::state::{AppState, OperationKind};

#[tauri::command]
pub async fn launch_game(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    #[cfg(not(feature = "e2e"))]
    if game_process_service::game_is_running(state.inner()).await {
        return Err(AppError::GameAlreadyRunning);
    }
    let _lease = state.begin_operation(OperationKind::LaunchingGame, None)?;

    #[cfg(feature = "e2e")]
    {
        app.emit("game-started", ())?;
        app.emit("game-closed", ())?;
        return Ok(());
    }

    #[cfg(not(feature = "e2e"))]
    {
        let game_path = config_service::get_game_path()
            .await?
            .ok_or(AppError::GamePathNotSet)?;
        game_process_service::launch_game(app, state.inner(), game_path).await
    }
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
