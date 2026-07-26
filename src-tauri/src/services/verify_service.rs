use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use crate::constants::DOWNLOAD_CONCURRENCY;
use crate::error::AppError;
use crate::models::{GameFileManifestEntry, ProgressEmitter, ProgressStage};
use crate::services::api_client::ApiClient;
use crate::services::config_service;
use crate::services::download_service::{download_files_parallel, DownloadFileTask};
use crate::services::elevation_service;
use crate::services::manifest_service::{load_manifest, VerifyMode};
use crate::utils::hash_utils::{md5_file, sha256_file};
use crate::utils::path_utils::safe_join;

pub async fn verify_game_files(
    app: AppHandle,
    game_path: PathBuf,
    mode: VerifyMode,
    cancel: CancellationToken,
    event_name: &str,
    operation_id: String,
) -> Result<(), AppError> {
    if !elevation_service::is_elevated()? {
        return Err(AppError::AdminRequired);
    }

    let progress = ProgressEmitter::new(app, event_name, operation_id);
    progress.emit_stage(ProgressStage::Checking, Some(0.0), None)?;
    let progress_stage = match mode {
        VerifyMode::UpdateFromVersion(_) => ProgressStage::Update,
        _ => ProgressStage::Verify,
    };

    let api = ApiClient::new()?;
    let manifest = load_manifest(&api, mode).await?;
    let exclude_files: HashSet<String> = api
        .get_exclude_files()
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();

    let total = manifest.files.len().max(1);
    let mut files_to_download = Vec::new();

    for (index, file) in manifest.files.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(AppError::Canceled);
        }

        if needs_download(&game_path, file, &exclude_files).await? {
            files_to_download.push(DownloadFileTask {
                url: file.url.clone(),
                relative_path: file.path.clone(),
                expected_md5: file.md5.clone(),
                expected_sha256: file.sha256.clone(),
            });
        }

        progress.emit_stage(
            progress_stage.clone(),
            Some(((index + 1) as f64 / total as f64) * 100.0),
            Some(file.path.clone()),
        )?;
    }

    let downloaded_files = files_to_download.len();
    if downloaded_files > 0 {
        download_files_parallel(
            api.http().clone(),
            files_to_download,
            game_path,
            DOWNLOAD_CONCURRENCY,
            progress.clone(),
            cancel.clone(),
        )
        .await?;
    }

    if !manifest.game_version.is_empty() {
        config_service::set_game_version(manifest.game_version).await?;
    }

    progress.emit_stage(ProgressStage::Complete, Some(100.0), None)?;
    Ok(())
}

async fn needs_download(
    game_path: &Path,
    file: &GameFileManifestEntry,
    exclude_files: &HashSet<String>,
) -> Result<bool, AppError> {
    let local_path = safe_join(game_path, &file.path)?;
    if !tokio::fs::try_exists(&local_path).await.unwrap_or(false) {
        return Ok(true);
    }

    if exclude_files.contains(&file.path) || file.excluded_from_hash_check {
        return Ok(false);
    }

    if let Some(expected) = file.sha256.as_deref() {
        let actual = sha256_file(&local_path).await?;
        return Ok(!actual.eq_ignore_ascii_case(expected));
    }

    if let Some(expected) = file.md5.as_deref() {
        let actual = md5_file(&local_path).await?;
        return Ok(!actual.eq_ignore_ascii_case(expected));
    }

    Ok(false)
}
