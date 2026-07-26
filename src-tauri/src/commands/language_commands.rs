use crate::error::AppError;
use crate::services::config_service;

#[tauri::command]
pub async fn get_language() -> Result<String, AppError> {
    config_service::get_language().await
}

#[tauri::command]
pub async fn set_language(language: String) -> Result<(), AppError> {
    config_service::set_language(language).await
}

#[tauri::command]
pub async fn translate(key: String) -> Result<String, AppError> {
    Ok(key)
}
