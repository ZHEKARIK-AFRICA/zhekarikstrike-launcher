use crate::error::AppError;
use crate::services::config_service;

#[tauri::command]
pub async fn get_language() -> Result<String, AppError> {
    #[cfg(feature = "e2e")]
    return Ok("en".to_string());

    #[cfg(not(feature = "e2e"))]
    config_service::get_language().await
}

#[tauri::command]
pub async fn set_language(language: String) -> Result<(), AppError> {
    #[cfg(feature = "e2e")]
    {
        let _ = language;
        return Ok(());
    }

    #[cfg(not(feature = "e2e"))]
    config_service::set_language(language).await
}

#[tauri::command]
pub async fn translate(key: String) -> Result<String, AppError> {
    Ok(key)
}
