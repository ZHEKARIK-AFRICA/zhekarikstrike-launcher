use std::path::PathBuf;

use tauri::AppHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::constants::REV_LOADER_EXE;
use crate::error::AppError;
use crate::models::{ProgressEmitter, ProgressStage};
use crate::services::api_client::ApiClient;
use crate::services::archive_service::extract_zip;
use crate::services::config_service;
use crate::services::disk_service::ensure_disk_space;
use crate::services::download_service::download_file;
use crate::services::elevation_service;
use crate::services::manifest_service::VerifyMode;
use crate::services::shortcut_service;
use crate::services::verify_service::verify_game_files;
use crate::utils::hash_utils::sha256_file;

pub async fn install_game(
    app: AppHandle,
    game_path: PathBuf,
    cancel: CancellationToken,
) -> Result<(), AppError> {
    if !elevation_service::is_elevated()? {
        return Err(AppError::AdminRequired);
    }

    tokio::fs::create_dir_all(&game_path).await?;

    let operation_id = Uuid::new_v4().to_string();
    let progress = ProgressEmitter::new(app.clone(), "install-progress", operation_id.clone());
    let api = ApiClient::new()?;
    let manifest = api.get_full_manifest().await?;
    let archive = manifest.archive;
    let required_bytes = archive.size;

    if !tokio::fs::try_exists(game_path.join(REV_LOADER_EXE))
        .await
        .unwrap_or(false)
    {
        ensure_disk_space(&game_path, required_bytes)?;

        let archive_path = game_path.join("client.zip");
        progress.emit_stage(ProgressStage::Download, Some(0.0), None)?;
        download_file(
            api.http(),
            &archive.url,
            &archive_path,
            Some(progress.clone()),
            cancel.clone(),
            None,
            Some(&archive.sha256),
        )
        .await?;

        let actual = sha256_file(&archive_path).await?;
        if actual != archive.sha256 {
            return Err(AppError::InvalidData("Archive sha256 mismatch".to_string()));
        }

        progress.emit_stage(ProgressStage::Extract, Some(0.0), None)?;
        ensure_disk_space(&game_path, ((required_bytes as f64) * 1.6) as u64)?;
        extract_zip(
            archive_path,
            game_path.clone(),
            progress.clone(),
            cancel.clone(),
        )
        .await?;

        config_service::set_game_version("0.0.0".to_string()).await?;
    } else {
        progress.emit_stage(
            ProgressStage::Verify,
            Some(0.0),
            Some("existing RevLoader.exe found, skipping archive download".to_string()),
        )?;
    }

    verify_game_files(
        app.clone(),
        game_path.clone(),
        VerifyMode::Full,
        cancel,
        "install-progress",
        operation_id,
    )
    .await?;

    config_service::set_game_path(game_path).await?;
    shortcut_service::create_default_shortcuts().await?;
    progress.emit_stage(ProgressStage::Complete, Some(100.0), None)?;
    Ok(())
}
