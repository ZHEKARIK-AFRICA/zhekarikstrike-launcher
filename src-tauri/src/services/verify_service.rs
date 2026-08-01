use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use crate::constants::DOWNLOAD_CONCURRENCY;
use crate::error::AppError;
use crate::models::{
    GameFileManifestEntry, GameManifest, ProgressEmitter, ProgressPayload, ProgressStage,
};
use crate::services::api_client::ApiClient;
use crate::services::config_service;
use crate::services::download_service::{download_files_parallel, DownloadFileTask};
use crate::services::elevation_service;
use crate::services::manifest_service::{load_manifest, VerifyMode};
use crate::services::prerequisite_service::bind_verified_legacy_manifest;
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
    let manifest = load_manifest(&api, mode.clone()).await?;
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
            game_path.clone(),
            DOWNLOAD_CONCURRENCY,
            progress.clone(),
            cancel.clone(),
        )
        .await?;
    }

    if let Some(version) = complete_verified_manifest(&game_path, &mode, &manifest).await? {
        config_service::set_game_version(version).await?;
    }

    progress.emit_stage(ProgressStage::Complete, Some(100.0), None)?;
    Ok(())
}

async fn complete_verified_manifest(
    game_root: &Path,
    mode: &VerifyMode,
    manifest: &GameManifest,
) -> Result<Option<String>, AppError> {
    if matches!(mode, VerifyMode::Full) {
        bind_verified_legacy_manifest(game_root, manifest).await?;
    }
    Ok((!manifest.game_version.is_empty()).then(|| manifest.game_version.clone()))
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

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::models::{
        expected_game_file_url, GameArchiveManifest, GameFileManifestEntry, GameManifest,
    };

    fn complete_manifest(version: &str) -> GameManifest {
        GameManifest {
            game_version: version.into(),
            generated_at: "2026-08-01T00:00:00Z".into(),
            archive: GameArchiveManifest {
                url: "https://api.zhekarik.africa/launcher/game/archive".into(),
                size: 1,
                sha256: "a".repeat(64),
                unpacked_size: 2,
            },
            files: ["game.exe", "RevLoader.exe"]
                .into_iter()
                .map(|path| GameFileManifestEntry {
                    path: path.into(),
                    size: 1,
                    sha256: "b".repeat(64),
                    url: expected_game_file_url(path),
                    excluded_from_hash_check: false,
                    temporary: false,
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn only_full_verification_binds_the_exact_verified_manifest() {
        let archive_manifest = complete_manifest("1.0.3.5");
        let verified_manifest = complete_manifest("1.0.3.6");
        let full_root = tempdir().expect("full verification root");
        let analysis_path = full_root
            .path()
            .join(".zhekarik/prerequisites/analysis-v1.json");
        fs::create_dir_all(analysis_path.parent().unwrap()).unwrap();
        fs::write(&analysis_path, b"poisoned cache").unwrap();

        let version =
            complete_verified_manifest(full_root.path(), &VerifyMode::Full, &verified_manifest)
                .await
                .expect("full verification should bind its manifest");
        assert_eq!(version.as_deref(), Some("1.0.3.6"));
        assert_ne!(
            version.as_deref(),
            Some(archive_manifest.game_version.as_str())
        );
        let sidecar: GameManifest = serde_json::from_slice(
            &fs::read(
                full_root
                    .path()
                    .join(".zhekarik/prerequisites/legacy-manifest.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(sidecar.game_version, "1.0.3.6");
        assert!(!analysis_path.exists());

        for mode in [
            VerifyMode::AdditionalOnly,
            VerifyMode::UpdateFromVersion("1.0.3.5".into()),
        ] {
            let root = tempdir().expect("non-full verification root");
            let cache = root.path().join(".zhekarik/prerequisites/analysis-v1.json");
            fs::create_dir_all(cache.parent().unwrap()).unwrap();
            fs::write(&cache, b"keep this cache").unwrap();

            complete_verified_manifest(root.path(), &mode, &verified_manifest)
                .await
                .expect("non-full verification should complete normally");

            assert!(!root
                .path()
                .join(".zhekarik/prerequisites/legacy-manifest.json")
                .exists());
            assert_eq!(fs::read(cache).unwrap(), b"keep this cache");
        }
    }

    #[tokio::test]
    async fn failed_full_binding_yields_no_known_version_and_retries() {
        let root = tempdir().expect("full verification root");
        let manifest = complete_manifest("1.0.3.6");
        let sidecar = root
            .path()
            .join(".zhekarik/prerequisites/legacy-manifest.json");
        fs::create_dir_all(&sidecar).expect("directory should block the atomic sidecar write");

        complete_verified_manifest(root.path(), &VerifyMode::Full, &manifest)
            .await
            .expect_err("failed binding must not yield a version for config persistence");
        assert!(
            crate::services::prerequisite_service::legacy_manifest_refresh_required(
                root.path(),
                "0.0.0",
            )
            .await
            .is_err()
        );

        fs::remove_dir(&sidecar).expect("failed-write fixture should be removable");
        let retry = complete_verified_manifest(root.path(), &VerifyMode::Full, &manifest)
            .await
            .expect("the next full verification should retry binding");
        assert_eq!(retry.as_deref(), Some("1.0.3.6"));
        assert!(sidecar.is_file());
    }
}
