use std::path::PathBuf;
use std::time::{Duration, Instant};

use sysinfo::System;
use tauri::{AppHandle, Emitter};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::constants::{
    GAME_PROCESS_NAME, GAME_START_TIMEOUT_MS, PROCESS_POLL_INTERVAL_MS, REV_LOADER_EXE,
};
use crate::error::AppError;
use crate::models::{GameProcessInfo, GameProcessStateKind};
use crate::services::file_patch_service::{copy_files_and_track, delete_tracked_files};
use crate::services::{
    discord_rpc_service, elevation_service, game_patch_service, shutdown_service,
};
use crate::state::AppState;

pub async fn launch_game(
    app: AppHandle,
    state: &AppState,
    game_path: PathBuf,
) -> Result<(), AppError> {
    if !elevation_service::is_elevated()? {
        return Err(AppError::AdminRequired);
    }

    let exe_path = game_path.join(REV_LOADER_EXE);
    if !tokio::fs::try_exists(&exe_path).await.unwrap_or(false) {
        return Err(AppError::GameExecutableNotFound(
            exe_path.display().to_string(),
        ));
    }

    let patch_cancel = CancellationToken::new();
    *state.file_patch_cancel_token.lock().await = Some(patch_cancel.clone());
    let patch_roots = match game_patch_service::sync_game_patch_cache(
        app.clone(),
        patch_cancel.clone(),
        "verify-progress",
        Uuid::new_v4().to_string(),
    )
    .await
    {
        Ok(roots) => roots,
        Err(error) => {
            state.file_patch_cancel_token.lock().await.take();
            return Err(error);
        }
    };

    let launch_result = async {
        app.emit("game-starting", ())?;
        {
            let mut process_state = state.process_state.write().await;
            process_state.kind = GameProcessStateKind::Starting;
            process_state.pid = None;
        }

        let previous = std::mem::take(&mut *state.copied_pure_files.lock().await);
        delete_tracked_files(previous).await?;

        let pure_source = patch_roots.game_files_pure;
        let copied_pure =
            copy_files_and_track(pure_source, game_path.clone(), true, Some(patch_cancel)).await?;
        *state.copied_pure_files.lock().await = copied_pure;

        if let Err(error) = discord_rpc_service::start_rich_presence(app.clone(), state).await {
            crate::logger::warn(&format!("Discord RPC start failed: {error}"));
        }

        Command::new(&exe_path)
            .current_dir(&game_path)
            .spawn()
            .map_err(|error| AppError::Unknown(format!("failed to spawn game: {error}")))?;

        let game_process = wait_for_process(
            GAME_PROCESS_NAME,
            Duration::from_millis(GAME_START_TIMEOUT_MS),
        )
        .await?;
        {
            let mut process_state = state.process_state.write().await;
            process_state.kind = GameProcessStateKind::Running;
            process_state.pid = Some(game_process.pid);
        }

        app.emit("game-started", game_process.clone())?;
        monitor_game_process(app.clone(), state.clone(), game_process.pid);
        Ok::<(), AppError>(())
    }
    .await;

    if let Err(error) = launch_result {
        if let Err(cleanup_error) = shutdown_service::cleanup_runtime(app, state).await {
            crate::logger::warn(&format!(
                "game launch failed ({error}) and cleanup also failed: {cleanup_error}"
            ));
        }
        return Err(error);
    }

    Ok(())
}

pub fn monitor_game_process(app: AppHandle, state: AppState, pid: u32) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(PROCESS_POLL_INTERVAL_MS)).await;
            if !is_pid_running(pid).await {
                break;
            }
        }

        let _ = shutdown_service::cleanup_runtime(app.clone(), &state).await;
    });
}

pub async fn stop_game_process(pid: u32) -> Result<(), AppError> {
    Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .spawn()
        .map_err(|error| AppError::Unknown(error.to_string()))?
        .wait()
        .await?;
    Ok(())
}

pub async fn find_process_by_name(name: &str) -> Option<GameProcessInfo> {
    let mut system = System::new_all();
    system.refresh_all();

    system.processes().iter().find_map(|(pid, process)| {
        let process_name = process.name().to_string_lossy().to_string();
        if process_name.eq_ignore_ascii_case(name) {
            Some(GameProcessInfo {
                pid: pid.as_u32(),
                name: process_name,
            })
        } else {
            None
        }
    })
}

async fn wait_for_process(name: &str, timeout: Duration) -> Result<GameProcessInfo, AppError> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(process) = find_process_by_name(name).await {
            return Ok(process);
        }
        tokio::time::sleep(Duration::from_millis(1_000)).await;
    }

    Err(AppError::Unknown(format!("Timed out waiting for {name}")))
}

async fn is_pid_running(pid: u32) -> bool {
    let mut system = System::new_all();
    system.refresh_all();
    system
        .processes()
        .keys()
        .any(|process_pid| process_pid.as_u32() == pid)
}
