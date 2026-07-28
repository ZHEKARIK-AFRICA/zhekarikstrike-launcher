use tauri::{AppHandle, Emitter};

use crate::error::AppError;
use crate::services::file_patch_service::{delete_tracked_files, restore_game_files};
use crate::services::{
    config_service, discord_rpc_service, game_patch_service, game_process_service,
    launcher_move_service,
};
use crate::state::AppState;

pub async fn shutdown(app: AppHandle, state: &AppState) -> Result<(), AppError> {
    cleanup_runtime(app.clone(), state).await?;
    launcher_move_service::schedule_legacy_move_if_needed(app).await?;
    Ok(())
}

pub async fn cleanup_runtime(app: AppHandle, state: &AppState) -> Result<(), AppError> {
    cleanup_runtime_inner(app, state, None).await
}

pub async fn cleanup_runtime_for_process(
    app: AppHandle,
    state: &AppState,
    expected_pid: u32,
) -> Result<(), AppError> {
    cleanup_runtime_inner(app, state, Some(expected_pid)).await
}

async fn cleanup_runtime_inner(
    app: AppHandle,
    state: &AppState,
    expected_pid: Option<u32>,
) -> Result<(), AppError> {
    let _guard = state.cleanup_lock.lock().await;

    let finished_process = {
        let mut process_state = state.process_state.write().await;
        if !cleanup_matches_expected_pid(process_state.pid, expected_pid) {
            return Ok(());
        }
        std::mem::take(&mut *process_state)
    };
    if game_process_service::is_observed_external_process(
        finished_process.pid,
        finished_process.owned,
    ) {
        let _ = app.emit("game-closed", ());
        return Ok(());
    }

    if let Some(token) = state.file_patch_cancel_token.lock().await.take() {
        token.cancel();
    }

    if let Some(pid) = finished_process.pid.filter(|_| finished_process.owned) {
        if let Err(error) = game_process_service::stop_game_process(pid).await {
            crate::logger::warn(&format!("failed to stop game process {pid}: {error}"));
        }
    }

    let pure_files = std::mem::take(&mut *state.copied_pure_files.lock().await);
    if let Err(error) = delete_tracked_files(pure_files).await {
        crate::logger::warn(&format!("failed to delete tracked pure files: {error}"));
    }

    if let Some(game_path) = config_service::get_game_path().await? {
        let source = game_patch_service::game_patch_roots()?.game_files;
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

    let _ = app.emit("game-closed", ());
    Ok(())
}

fn cleanup_matches_expected_pid(current_pid: Option<u32>, expected_pid: Option<u32>) -> bool {
    expected_pid.is_none() || current_pid == expected_pid
}

#[cfg(test)]
mod release_1_6_11_tests {
    use super::cleanup_matches_expected_pid;

    #[test]
    fn release_1_6_11_stale_process_monitor_cannot_clean_up_a_new_game() {
        assert!(cleanup_matches_expected_pid(Some(42), Some(42)));
        assert!(!cleanup_matches_expected_pid(Some(43), Some(42)));
        assert!(!cleanup_matches_expected_pid(None, Some(42)));
        assert!(cleanup_matches_expected_pid(Some(43), None));
    }
}
