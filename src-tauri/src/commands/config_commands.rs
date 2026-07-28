use std::path::PathBuf;

use tauri::{AppHandle, State};

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
    #[cfg(feature = "e2e")]
    return Ok(Some("D:\\Games\\ZHEKARIKSTRIKE".to_string()));

    #[cfg(not(feature = "e2e"))]
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
    #[cfg(feature = "e2e")]
    return Ok("1.6.0".to_string());

    #[cfg(not(feature = "e2e"))]
    config_service::get_game_version().await
}

#[tauri::command]
pub async fn get_current_state(state: State<'_, AppState>) -> Result<CurrentState, AppError> {
    Ok(state.current_state())
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
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<crate::models::GameProcessState, AppError> {
    #[cfg(feature = "e2e")]
    {
        let _ = app;
        Ok(state.process_state.read().await.clone())
    }
    #[cfg(not(feature = "e2e"))]
    {
        crate::services::game_process_service::sync_game_process(app, state.inner()).await
    }
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

    Ok(check_game_exists_at(game_path).await)
}

async fn check_game_exists_at(game_path: PathBuf) -> GameExistenceStatus {
    let loader_exists = tokio::fs::try_exists(game_path.join(constants::REV_LOADER_EXE))
        .await
        .unwrap_or(false);
    GameExistenceStatus {
        exists: loader_exists,
        game_path: Some(game_path.to_string_lossy().to_string()),
        missing_files: if loader_exists {
            Vec::new()
        } else {
            vec![constants::REV_LOADER_EXE.to_string()]
        },
    }
}

#[cfg(test)]
mod tests {
    use super::check_game_exists_at;

    #[tokio::test]
    async fn rev_loader_is_enough_to_recognize_an_existing_installation() {
        let directory = tempfile::tempdir().expect("temporary directory");
        tokio::fs::write(directory.path().join("RevLoader.exe"), b"test")
            .await
            .expect("write RevLoader fixture");

        let status = check_game_exists_at(directory.path().to_path_buf()).await;

        assert!(status.exists);
        assert!(status.missing_files.is_empty());
    }

    #[tokio::test]
    async fn missing_rev_loader_requires_installation() {
        let directory = tempfile::tempdir().expect("temporary directory");

        let status = check_game_exists_at(directory.path().to_path_buf()).await;

        assert!(!status.exists);
        assert_eq!(status.missing_files, vec!["RevLoader.exe"]);
    }
}
