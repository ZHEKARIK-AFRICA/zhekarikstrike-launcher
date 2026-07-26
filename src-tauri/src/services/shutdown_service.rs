use tauri::{AppHandle, Emitter};

use crate::error::AppError;
use crate::models::GameProcessStateKind;
use crate::services::file_patch_service::{
    bundled_game_files, delete_tracked_files, restore_game_files,
};
use crate::services::{
    config_service, discord_rpc_service, game_process_service, launcher_move_service,
};
use crate::state::AppState;

pub async fn shutdown(app: AppHandle, state: &AppState) -> Result<(), AppError> {
    cleanup_runtime(app.clone(), state).await?;
    launcher_move_service::schedule_legacy_move_if_needed(app).await?;
    Ok(())
}

pub async fn cleanup_runtime(app: AppHandle, state: &AppState) -> Result<(), AppError> {
    let _guard = state.cleanup_lock.lock().await;

    if let Some(token) = state.file_patch_cancel_token.lock().await.take() {
        token.cancel();
    }

    let pid = state.process_state.read().await.pid;
    if let Some(pid) = pid {
        if let Err(error) = game_process_service::stop_game_process(pid).await {
            crate::logger::warn(&format!("failed to stop game process {pid}: {error}"));
        }
    }

    let pure_files = std::mem::take(&mut *state.copied_pure_files.lock().await);
    if let Err(error) = delete_tracked_files(pure_files).await {
        crate::logger::warn(&format!("failed to delete tracked pure files: {error}"));
    }

    if let Some(game_path) = config_service::get_game_path().await? {
        let source = bundled_game_files(&app);
        match restore_game_files(source, game_path).await {
            Ok(restored) => {
                *state.copied_game_files.lock().await = restored;
            }
            Err(error) => {
                crate::logger::warn(&format!("failed to restore game files: {error}"));
            }
        }
    }

    if let Err(error) = discord_rpc_service::stop_rich_presence(app.clone(), state).await {
        crate::logger::warn(&format!("failed to stop Discord RPC: {error}"));
    }

    {
        let mut process_state = state.process_state.write().await;
        process_state.kind = GameProcessStateKind::Stopped;
        process_state.pid = None;
    }

    let _ = app.emit("game-closed", ());
    Ok(())
}
