use std::env;
use std::path::PathBuf;

use crate::constants::APP_NAME;
use crate::error::AppError;
use crate::models::LauncherConfig;
use crate::utils::json_utils::{read_json, write_json};

pub fn get_config_dir() -> Result<PathBuf, AppError> {
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        return Ok(PathBuf::from(local_app_data).join(APP_NAME));
    }

    if let Ok(user_profile) = env::var("USERPROFILE") {
        return Ok(PathBuf::from(user_profile)
            .join("AppData")
            .join("Local")
            .join(APP_NAME));
    }

    Err(AppError::Config(
        "Unable to resolve LOCALAPPDATA".to_string(),
    ))
}

pub fn get_config_path() -> Result<PathBuf, AppError> {
    Ok(get_config_dir()?.join("config.json"))
}

pub async fn load_config() -> Result<LauncherConfig, AppError> {
    read_json(&get_config_path()?).await
}

pub async fn save_config(config: &LauncherConfig) -> Result<(), AppError> {
    write_json(&get_config_path()?, config).await
}

pub async fn get_game_path() -> Result<Option<PathBuf>, AppError> {
    Ok(load_config().await?.game_path.map(PathBuf::from))
}

pub async fn set_game_path(path: PathBuf) -> Result<(), AppError> {
    let mut config = load_config().await?;
    config.game_path = Some(path.to_string_lossy().to_string());
    save_config(&config).await
}

pub async fn get_language() -> Result<String, AppError> {
    Ok(load_config()
        .await?
        .language
        .filter(|language| language == "ru" || language == "en")
        .unwrap_or_else(|| "ru".to_string()))
}

pub async fn set_language(language: String) -> Result<(), AppError> {
    let mut config = load_config().await?;
    config.language = Some(if language == "en" { "en" } else { "ru" }.to_string());
    save_config(&config).await
}

pub async fn get_game_version() -> Result<String, AppError> {
    Ok(load_config()
        .await?
        .game_version
        .unwrap_or_else(|| "0.0.0".to_string()))
}

pub async fn set_game_version(version: String) -> Result<(), AppError> {
    let mut config = load_config().await?;
    config.game_version = Some(version);
    save_config(&config).await
}
