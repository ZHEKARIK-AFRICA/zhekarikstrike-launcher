use std::path::PathBuf;

use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use crate::constants::REV_LOADER_EXE;
use crate::error::AppError;
use crate::models::{ProgressEmitter, ProgressStage};
use crate::services::api_client::ApiClient;
use crate::services::archive_service::extract_zip;
use crate::services::config_service;
use crate::services::content_install_service;
use crate::services::disk_service::ensure_disk_space;
use crate::services::download_service::download_file;
use crate::services::elevation_service;
use crate::services::manifest_service::VerifyMode;
use crate::services::verify_service::verify_game_files;

fn required_install_bytes(archive_size: u64, unpacked_size: u64) -> Result<u64, AppError> {
    archive_size
        .checked_add(unpacked_size)
        .ok_or_else(|| AppError::InvalidData("installation size overflow".to_string()))
}

pub async fn install_game(
    app: AppHandle,
    game_path: PathBuf,
    cancel: CancellationToken,
    operation_id: String,
) -> Result<(), AppError> {
    if !elevation_service::is_elevated()? {
        return Err(AppError::AdminRequired);
    }

    tokio::fs::create_dir_all(&game_path).await?;
    config_service::set_game_path(game_path.clone()).await?;

    let api = ApiClient::new()?;
    if let Some(manifest) = api.get_compatible_content_manifest().await? {
        content_install_service::install_or_update_content(
            app,
            game_path.clone(),
            api,
            manifest,
            cancel,
            "install-progress",
            operation_id,
        )
        .await?;
        return Ok(());
    }

    let progress = ProgressEmitter::new(app.clone(), "install-progress", operation_id.clone());
    let manifest = api.get_full_manifest().await?;
    let archive = manifest.archive.clone();
    let required_bytes = required_install_bytes(archive.size, archive.unpacked_size)?;

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
            Some(archive.size),
            Some(&archive.sha256),
        )
        .await?;

        progress.emit_stage(ProgressStage::Extract, Some(0.0), None)?;
        extract_zip(
            archive_path,
            game_path.clone(),
            progress.clone(),
            cancel.clone(),
        )
        .await?;
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

    progress.emit_stage(ProgressStage::Complete, Some(100.0), None)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::required_install_bytes;

    #[test]
    fn installation_reserves_archive_and_unpacked_bytes_at_the_same_time() {
        assert_eq!(required_install_bytes(10, 25).unwrap(), 35);
        assert!(required_install_bytes(u64::MAX, 1).is_err());
    }
}
