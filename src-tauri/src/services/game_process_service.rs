use std::future::Future;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use sysinfo::System;
use tauri::{AppHandle, Emitter};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::constants::{
    GAME_PROCESS_NAME, GAME_START_TIMEOUT_MS, PROCESS_POLL_INTERVAL_MS, REV_LOADER_EXE,
};
use crate::error::AppError;
use crate::models::{GameProcessInfo, GameProcessState, GameProcessStateKind};
use crate::services::file_patch_service::{copy_files_and_track, delete_tracked_files};
use crate::services::{
    discord_rpc_service, elevation_service, game_patch_service, shutdown_service,
};
use crate::state::AppState;

pub async fn launch_game(
    app: AppHandle,
    state: &AppState,
    game_path: PathBuf,
    operation_id: String,
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
        operation_id,
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
            process_state.owned = true;
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

        let mut revloader = Command::new(&exe_path)
            .current_dir(&game_path)
            .spawn()
            .map_err(|error| AppError::Unknown(format!("failed to spawn game: {error}")))?;

        let game_process = wait_for_game_or_loader_exit(
            wait_for_process(
                GAME_PROCESS_NAME,
                Duration::from_millis(GAME_START_TIMEOUT_MS),
            ),
            async move {
                let status = revloader.wait().await.map_err(|error| {
                    AppError::Unknown(format!("failed to observe RevLoader.exe: {error}"))
                })?;
                if let Some(game_process) = find_process_by_name(GAME_PROCESS_NAME).await {
                    return Ok(game_process);
                }
                revloader_exit_result(status.code().unwrap_or(-1))
            },
        )
        .await?;
        {
            let mut process_state = state.process_state.write().await;
            process_state.kind = GameProcessStateKind::Running;
            process_state.pid = Some(game_process.pid);
            process_state.owned = true;
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

pub async fn game_is_running(state: &AppState) -> bool {
    if let Some(pid) = state.process_state.read().await.pid {
        if is_pid_running(pid).await {
            return true;
        }
    }
    find_process_by_name(GAME_PROCESS_NAME).await.is_some()
}

pub async fn sync_game_process(
    app: AppHandle,
    state: &AppState,
) -> Result<GameProcessState, AppError> {
    let current = state.process_state.read().await.clone();
    if let Some(pid) = current.pid {
        if is_pid_running(pid).await {
            return Ok(current);
        }
        shutdown_service::cleanup_runtime_for_process(app.clone(), state, pid).await?;
    }

    let synchronized = detected_process_state(find_process_by_name(GAME_PROCESS_NAME).await);
    *state.process_state.write().await = synchronized.clone();
    if let Some(pid) = synchronized.pid {
        monitor_game_process(app, state.clone(), pid);
    }
    Ok(synchronized)
}

pub fn monitor_game_process(app: AppHandle, state: AppState, pid: u32) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(PROCESS_POLL_INTERVAL_MS)).await;
            if !is_pid_running(pid).await {
                break;
            }
        }

        let _ = shutdown_service::cleanup_runtime_for_process(app.clone(), &state, pid).await;
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

fn detected_process_state(process: Option<GameProcessInfo>) -> GameProcessState {
    match process {
        Some(process) => GameProcessState {
            kind: GameProcessStateKind::Running,
            pid: Some(process.pid),
            owned: false,
        },
        None => GameProcessState::default(),
    }
}

pub(crate) fn is_observed_external_process(tracked_pid: Option<u32>, owned: bool) -> bool {
    tracked_pid.is_some() && !owned
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

fn classify_revloader_exit(code: i32) -> Result<(), AppError> {
    if code == 0xC000_0135_u32 as i32 {
        return Err(AppError::PrerequisiteRestartRequired(
            "RevLoader.exe could not find a required runtime DLL; install prerequisites or restart Windows"
                .to_string(),
        ));
    }
    Err(AppError::Unknown(format!(
        "RevLoader.exe exited before the game appeared (code {code})"
    )))
}

fn revloader_exit_result(code: i32) -> Result<GameProcessInfo, AppError> {
    classify_revloader_exit(code)?;
    unreachable!("all loader exits are launch failures")
}

async fn wait_for_game_or_loader_exit<G, L>(game: G, loader: L) -> Result<GameProcessInfo, AppError>
where
    G: Future<Output = Result<GameProcessInfo, AppError>>,
    L: Future<Output = Result<GameProcessInfo, AppError>>,
{
    tokio::pin!(game);
    tokio::pin!(loader);
    tokio::select! {
        game = &mut game => game,
        loader = &mut loader => loader,
    }
}

async fn is_pid_running(pid: u32) -> bool {
    let mut system = System::new_all();
    system.refresh_all();
    system
        .processes()
        .keys()
        .any(|process_pid| process_pid.as_u32() == pid)
}

#[cfg(test)]
mod release_1_6_11_tests {
    use std::future::{pending, ready};

    use super::{
        classify_revloader_exit, detected_process_state, is_observed_external_process,
        revloader_exit_result, wait_for_game_or_loader_exit,
    };
    use crate::error::AppError;
    use crate::models::{GameProcessInfo, GameProcessStateKind};

    #[test]
    fn release_1_6_11_detected_game_process_becomes_running_state() {
        let state = detected_process_state(Some(GameProcessInfo {
            pid: 42,
            name: "zhekarikstrike.exe".to_string(),
        }));
        assert!(matches!(state.kind, GameProcessStateKind::Running));
        assert_eq!(state.pid, Some(42));

        let stopped = detected_process_state(None);
        assert!(matches!(stopped.kind, GameProcessStateKind::Stopped));
        assert_eq!(stopped.pid, None);
    }

    #[test]
    fn release_1_6_11_external_game_is_observed_but_not_owned() {
        assert!(is_observed_external_process(Some(42), false));
        assert!(!is_observed_external_process(Some(42), true));
        assert!(!is_observed_external_process(None, false));
    }

    #[test]
    fn release_1_6_13_missing_runtime_exit_is_a_targeted_prerequisite_error() {
        let error = classify_revloader_exit(0xC000_0135_u32 as i32)
            .expect_err("missing DLL exit must fail immediately");
        assert!(matches!(error, AppError::PrerequisiteRestartRequired(_)));
    }

    #[test]
    fn release_1_6_13_other_revloader_exit_is_reported_immediately() {
        let error = classify_revloader_exit(5)
            .expect_err("unexpected loader exit must not wait for the game timeout");
        assert_eq!(error.code(), "unknown");
        assert!(error.to_string().contains("code 5"));
    }

    #[tokio::test]
    async fn release_1_6_13_loader_exit_wins_without_waiting_for_game_timeout() {
        let result = wait_for_game_or_loader_exit(pending(), ready(revloader_exit_result(5))).await;

        assert!(result
            .expect_err("loader exit should win")
            .to_string()
            .contains("code 5"));
    }

    #[tokio::test]
    async fn release_1_6_13_loader_handoff_accepts_a_game_found_on_final_probe() {
        let handed_off = GameProcessInfo {
            pid: 73,
            name: "zhekarikstrike.exe".into(),
        };

        let game = wait_for_game_or_loader_exit(pending(), ready(Ok(handed_off.clone())))
            .await
            .expect("a game found as RevLoader exits should win the race");

        assert_eq!(game.pid, 73);
    }
}
