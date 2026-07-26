use std::path::PathBuf;

use tauri::State;

use crate::error::AppError;
use crate::models::{GameExistenceStatus, LauncherConfig, StartupState};
use crate::services::{config_service, launcher_update_service};
use crate::state::AppState;
use crate::{constants, state::CurrentState};

#[tauri::command]
pub async fn get_config() -> Result<LauncherConfig, AppError> {
    config_service::load_config().await
}

#[tauri::command]
pub async fn get_game_path() -> Result<Option<String>, AppError> {
    Ok(config_service::get_game_path()
        .await?
        .map(|path| path.to_string_lossy().to_string()))
}

#[tauri::command]
pub async fn set_game_path(game_path: String) -> Result<(), AppError> {
    config_service::set_game_path(PathBuf::from(game_path)).await
}

#[tauri::command]
pub async fn get_game_version() -> Result<String, AppError> {
    config_service::get_game_version().await
}

#[tauri::command]
pub async fn get_current_state(state: State<'_, AppState>) -> Result<CurrentState, AppError> {
    Ok(state.current_state().await)
}

#[tauri::command]
pub async fn check_game_exists() -> Result<GameExistenceStatus, AppError> {
    check_game_exists_inner().await
}

#[tauri::command]
pub async fn get_startup_state() -> Result<StartupState, AppError> {
    let language = config_service::get_language().await?;
    let game = check_game_exists_inner().await?;
    let launcher_update_required =
        launcher_update_service::check_launcher_update(env!("CARGO_PKG_VERSION"))
            .await
            .map(|status| status.has_update && status.can_apply)
            .unwrap_or(false);

    Ok(StartupState {
        launcher_update_required,
        game_exists: game.exists,
        game_path: game.game_path,
        language,
    })
}

#[tauri::command]
pub async fn get_game_process_state(
    state: State<'_, AppState>,
) -> Result<crate::models::GameProcessState, AppError> {
    Ok(state.process_state.read().await.clone())
}

async fn check_game_exists_inner() -> Result<GameExistenceStatus, AppError> {
    let game_path = config_service::get_game_path().await?;
    let Some(game_path) = game_path else {
        return Ok(GameExistenceStatus {
            exists: false,
            game_path: None,
            missing_files: vec!["gamePath".to_string()],
        });
    };

    let required = [
        constants::REV_LOADER_EXE,
        constants::GAME_PROCESS_NAME,
        constants::REV_INI,
    ];
    let mut missing_files = Vec::new();
    for file in required {
        if !tokio::fs::try_exists(game_path.join(file))
            .await
            .unwrap_or(false)
        {
            missing_files.push(file.to_string());
        }
    }

    let game_version = config_service::get_game_version().await?;
    if game_version.is_empty() || game_version == "0.0.0" {
        missing_files.push("gameVersion".to_string());
    }

    let exists = missing_files.is_empty();
    Ok(GameExistenceStatus {
        exists,
        game_path: Some(game_path.to_string_lossy().to_string()),
        missing_files,
    })
}
