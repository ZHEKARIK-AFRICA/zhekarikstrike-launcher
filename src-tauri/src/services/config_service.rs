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
    let config = load_config().await?;
    let system_locale = sys_locale::get_locale();
    Ok(resolve_language(
        config.language.as_deref(),
        system_locale.as_deref(),
    ))
}

pub fn resolve_language(saved_language: Option<&str>, system_locale: Option<&str>) -> String {
    if matches!(saved_language, Some("ru") | Some("en")) {
        return saved_language.unwrap_or("en").to_string();
    }
    if system_locale
        .unwrap_or_default()
        .to_ascii_lowercase()
        .starts_with("ru")
    {
        "ru".to_string()
    } else {
        "en".to_string()
    }
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

#[cfg(test)]
mod tests {
    use super::resolve_language;

    #[test]
    fn saved_supported_language_wins_over_system_locale() {
        assert_eq!(resolve_language(Some("en"), Some("ru-RU")), "en");
    }

    #[test]
    fn russian_system_locale_defaults_to_russian() {
        assert_eq!(resolve_language(None, Some("ru-RU")), "ru");
    }

    #[test]
    fn non_russian_system_locale_defaults_to_english() {
        assert_eq!(resolve_language(None, Some("de-DE")), "en");
    }
}
