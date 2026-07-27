use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::constants::ALLOWED_EXTERNAL_URLS;
use crate::error::AppError;
use crate::services::api_client::ApiClient;
use crate::services::{
    content_install_service, disk_service, elevation_service, launcher_move_service,
    shortcut_service,
};

#[tauri::command]
pub async fn select_game_folder(app: AppHandle) -> Result<Option<String>, AppError> {
    let folder = app.dialog().file().blocking_pick_folder();
    Ok(folder.map(|path| path.to_string()))
}

#[tauri::command]
pub async fn open_external_url(app: AppHandle, url: String) -> Result<(), AppError> {
    let allowed = ALLOWED_EXTERNAL_URLS
        .iter()
        .any(|allowed| url == *allowed || url.starts_with(&format!("{allowed}/")));

    if !allowed {
        return Err(AppError::InvalidData(format!(
            "External URL is not allowed: {url}"
        )));
    }

    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|error| AppError::Unknown(error.to_string()))?;
    Ok(())
}

#[tauri::command]
pub async fn check_disk_space_for_install(
    game_path: String,
) -> Result<disk_service::DiskSpaceStatus, AppError> {
    let api = ApiClient::new()?;
    if let Some(manifest) = api.get_content_manifest().await? {
        let game_path = std::path::Path::new(&game_path);
        let backup =
            content_install_service::estimate_existing_backup_bytes(game_path, &manifest).await?;
        let required =
            content_install_service::conservative_content_install_bytes(&manifest, backup)?;
        return disk_service::check_disk_space(game_path, required).await;
    }
    let archive = api.get_full_manifest().await?.archive;
    let required_bytes = archive
        .size
        .checked_add(archive.unpacked_size)
        .ok_or_else(|| AppError::InvalidData("installation size overflow".to_string()))?;
    disk_service::check_disk_space(std::path::Path::new(&game_path), required_bytes).await
}

#[tauri::command]
pub async fn create_shortcuts() -> Result<(), AppError> {
    shortcut_service::create_default_shortcuts().await
}

#[tauri::command]
pub fn is_elevated() -> bool {
    elevation_service::is_elevated().unwrap_or(false)
}

#[tauri::command]
pub fn relaunch_as_admin() -> Result<(), AppError> {
    elevation_service::relaunch_as_admin()
}

#[tauri::command]
pub async fn move_launcher_to_game_path(app: AppHandle) -> Result<(), AppError> {
    launcher_move_service::move_launcher_to_game_path(app).await
}
