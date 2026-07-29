use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use crate::constants::DOWNLOAD_CONCURRENCY;
use crate::error::AppError;
use crate::models::{GameFileManifestEntry, ProgressEmitter, ProgressPayload, ProgressStage};
use crate::services::api_client::ApiClient;
use crate::services::config_service;
use crate::services::download_service::{download_files_parallel, DownloadFileTask};
use crate::services::elevation_service;
use crate::services::manifest_service::{load_manifest, VerifyMode};
use crate::services::verify_hash_service::{
    find_hash_mismatches, VerifyHashProgress, VerifyHashTask,
};
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
    // Classification and hashing are a complete integrity check.  Downloads use
    // their own Download stage below, so the UI can restart at 0% for repair.
    let progress_stage = ProgressStage::Checking;

    let api = ApiClient::new()?;
    let manifest = load_manifest(&api, mode).await?;
    let exclude_files: HashSet<String> = api
        .get_exclude_files()
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();

    let total_entries = manifest.files.len().max(1);
    let mut files_to_download = Vec::new();
    let mut hash_tasks = Vec::new();

    for (index, file) in manifest.files.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(AppError::Canceled);
        }

        classify_file(
            &game_path,
            file,
            &exclude_files,
            &mut files_to_download,
            &mut hash_tasks,
        )
        .await?;

        emit_verify_progress(
            &progress,
            progress_stage.clone(),
            ((index + 1) as f64 / total_entries as f64) * 5.0,
            Some(file.path.clone()),
            None,
            None,
        )?;
    }

    let hash_progress_emitter = progress.clone();
    let hash_progress_stage = progress_stage.clone();
    let hash_progress = Arc::new(move |update: VerifyHashProgress| {
        let ratio = if update.total_bytes == 0 {
            1.0
        } else {
            update.completed_bytes as f64 / update.total_bytes as f64
        };
        if let Err(error) = emit_verify_progress(
            &hash_progress_emitter,
            hash_progress_stage.clone(),
            5.0 + ratio.clamp(0.0, 1.0) * 95.0,
            update.current_file,
            Some(update.speed_bytes_per_sec),
            update.time_remaining_sec,
        ) {
            crate::logger::warn(&format!(
                "failed to emit integrity verification progress: {error}"
            ));
        }
    });

    let mismatches = find_hash_mismatches(hash_tasks, cancel.clone(), hash_progress).await?;
    files_to_download.extend(mismatches.iter().map(download_task));

    if !files_to_download.is_empty() {
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

async fn classify_file(
    game_path: &std::path::Path,
    file: &GameFileManifestEntry,
    exclude_files: &HashSet<String>,
    files_to_download: &mut Vec<DownloadFileTask>,
    hash_tasks: &mut Vec<VerifyHashTask>,
) -> Result<(), AppError> {
    let local_path = safe_join(game_path, &file.path)?;
    if !tokio::fs::try_exists(&local_path).await.unwrap_or(false) {
        files_to_download.push(download_task(file));
        return Ok(());
    }

    if exclude_files.contains(&file.path) || file.excluded_from_hash_check {
        return Ok(());
    }

    let metadata = tokio::fs::metadata(&local_path).await?;
    if !metadata.is_file() || metadata.len() != file.size {
        files_to_download.push(download_task(file));
        return Ok(());
    }

    hash_tasks.push(VerifyHashTask {
        file: file.clone(),
        local_path,
    });
    Ok(())
}

fn download_task(file: &GameFileManifestEntry) -> DownloadFileTask {
    DownloadFileTask {
        url: file.url.clone(),
        relative_path: file.path.clone(),
        expected_size: Some(file.size),
        expected_sha256: Some(file.sha256.clone()),
    }
}

fn emit_verify_progress(
    progress: &ProgressEmitter,
    stage: ProgressStage,
    percentage: f64,
    current_file: Option<String>,
    speed_bytes_per_sec: Option<f64>,
    time_remaining_sec: Option<f64>,
) -> Result<(), AppError> {
    let mut payload = ProgressPayload::new(progress.operation_id().to_string(), stage);
    payload.progress = Some(percentage.clamp(0.0, 100.0));
    payload.current_file = current_file;
    payload.speed_bytes_per_sec = speed_bytes_per_sec;
    payload.time_remaining_sec = time_remaining_sec;
    progress.emit(payload)
}
