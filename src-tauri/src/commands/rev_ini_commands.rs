use crate::error::AppError;
use crate::models::GameData;
use crate::services::{config_service, rev_ini_service};

#[tauri::command]
pub async fn get_game_data() -> Result<GameData, AppError> {
    let game_path = config_service::get_game_path()
        .await?
        .ok_or(AppError::GamePathNotSet)?;
    rev_ini_service::read_rev_ini(game_path).await
}

#[tauri::command]
pub async fn update_rev_ini(
    nickname: String,
    clan_tag: String,
    launch_params: String,
) -> Result<(), AppError> {
    let game_path = config_service::get_game_path()
        .await?
        .ok_or(AppError::GamePathNotSet)?;
    let language = config_service::get_language().await?;
    rev_ini_service::update_rev_ini(game_path, nickname, clan_tag, launch_params, language).await
}
