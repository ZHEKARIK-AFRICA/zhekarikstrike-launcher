use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{stream::FuturesUnordered, StreamExt};
use reqwest::Client;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::models::{ProgressEmitter, ProgressPayload, ProgressStage};
use crate::utils::hash_utils::sha256_file;
use crate::utils::time_utils::seconds_remaining;

#[derive(Debug, Clone)]
pub struct DownloadFileTask {
    pub url: String,
    pub relative_path: String,
    pub expected_size: Option<u64>,
    pub expected_sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub path: PathBuf,
    pub bytes: u64,
}

pub async fn download_file(
    client: &Client,
    url: &str,
    target_path: &Path,
    progress: Option<ProgressEmitter>,
    cancel: CancellationToken,
    expected_size: Option<u64>,
    expected_sha256: Option<&str>,
) -> Result<DownloadResult, AppError> {
    let part_path = target_path.with_extension(
        target_path
            .extension()
            .map(|ext| format!("{}.part", ext.to_string_lossy()))
            .unwrap_or_else(|| "part".to_string()),
    );

    if let Some(parent) = target_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut last_error = None;
    for attempt in 0..=3 {
        if cancel.is_cancelled() {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(AppError::Canceled);
        }

        match try_download_file(
            client,
            url,
            &part_path,
            progress.clone(),
            cancel.clone(),
            expected_size,
        )
        .await
        {
            Ok(bytes) => {
                let verification = async {
                    validate_download_size(bytes, expected_size)?;
                    verify_download_hash(&part_path, expected_sha256).await
                }
                .await;
                if let Err(error) = verification {
                    last_error = Some(error);
                    let _ = tokio::fs::remove_file(&part_path).await;
                    if attempt < 3 {
                        let delay_ms = 500_u64.saturating_mul(2_u64.saturating_pow(attempt));
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                    continue;
                }

                if let Ok(metadata) = tokio::fs::symlink_metadata(target_path).await {
                    if metadata.is_dir() {
                        return Err(AppError::InvalidData(format!(
                            "download target is a directory: {}",
                            target_path.display()
                        )));
                    }
                    tokio::fs::remove_file(target_path).await?;
                }
                tokio::fs::rename(&part_path, target_path).await?;
                return Ok(DownloadResult {
                    path: target_path.to_path_buf(),
                    bytes,
                });
            }
            Err(AppError::Canceled) => {
                let _ = tokio::fs::remove_file(&part_path).await;
                return Err(AppError::Canceled);
            }
            Err(error) => {
                last_error = Some(error);
                let _ = tokio::fs::remove_file(&part_path).await;
                if attempt < 3 {
                    let delay_ms = 500_u64.saturating_mul(2_u64.saturating_pow(attempt));
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
        }
    }

    let _ = tokio::fs::remove_file(&part_path).await;
    Err(last_error.unwrap_or_else(|| AppError::Network("download failed".to_string())))
}

async fn try_download_file(
    client: &Client,
    url: &str,
    part_path: &Path,
    progress: Option<ProgressEmitter>,
    cancel: CancellationToken,
    expected_size: Option<u64>,
) -> Result<u64, AppError> {
    let response = client.get(url).send().await?.error_for_status()?;
    let total = response.content_length();
    if let (Some(actual), Some(expected)) = (total, expected_size) {
        validate_download_size(actual, Some(expected))?;
    }
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(part_path).await?;
    let mut downloaded = 0_u64;
    let start = Instant::now();

    while let Some(chunk) = stream.next().await {
        if cancel.is_cancelled() {
            return Err(AppError::Canceled);
        }

        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;

        if let Some(progress) = progress.as_ref() {
            let mut payload =
                ProgressPayload::new(progress.operation_id().to_string(), ProgressStage::Download);
            payload.downloaded_bytes = Some(downloaded);
            payload.total_bytes = total;
            payload.progress = total.map(|total| (downloaded as f64 / total as f64) * 100.0);
            payload.speed_bytes_per_sec =
                Some(downloaded as f64 / start.elapsed().as_secs_f64().max(0.001));
            payload.time_remaining_sec = seconds_remaining(start, downloaded, total);
            progress.emit(payload)?;
        }
    }

    file.flush().await?;
    Ok(downloaded)
}

fn validate_download_size(actual: u64, expected: Option<u64>) -> Result<(), AppError> {
    if let Some(expected) = expected {
        if actual != expected {
            return Err(AppError::InvalidData(format!(
                "download size mismatch: expected {expected}, received {actual}"
            )));
        }
    }
    Ok(())
}

async fn verify_download_hash(
    part_path: &Path,
    expected_sha256: Option<&str>,
) -> Result<(), AppError> {
    if let Some(expected) = expected_sha256 {
        let actual = sha256_file(part_path).await?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(AppError::InvalidData(format!(
                "sha256 mismatch for {}",
                part_path.display()
            )));
        }
    }

    Ok(())
}

pub async fn download_files_parallel(
    client: Client,
    files: Vec<DownloadFileTask>,
    target_root: PathBuf,
    concurrency: usize,
    progress: ProgressEmitter,
    cancel: CancellationToken,
) -> Result<(), AppError> {
    let concurrency = concurrency.clamp(1, crate::constants::MAX_DOWNLOAD_CONCURRENCY);
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let total = files.len().max(1);
    let mut completed = 0_usize;
    let mut futures = FuturesUnordered::new();

    for task in files {
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let client = client.clone();
        let target = target_root.join(&task.relative_path);
        let progress = progress.clone();
        let task_cancel = cancel.clone();

        futures.push(tokio::spawn(async move {
            let _permit = permit;
            download_file(
                &client,
                &task.url,
                &target,
                None,
                task_cancel,
                task.expected_size,
                task.expected_sha256.as_deref(),
            )
            .await
        }));

        if cancel.is_cancelled() {
            return Err(AppError::Canceled);
        }

        while futures.len() >= concurrency {
            if let Some(result) = futures.next().await {
                result.map_err(|error| AppError::Unknown(error.to_string()))??;
                completed += 1;
                progress.emit_stage(
                    ProgressStage::Download,
                    Some((completed as f64 / total as f64) * 100.0),
                    None,
                )?;
            }
        }
    }

    while let Some(result) = futures.next().await {
        result.map_err(|error| AppError::Unknown(error.to_string()))??;
        completed += 1;
        progress.emit_stage(
            ProgressStage::Download,
            Some((completed as f64 / total as f64) * 100.0),
            None,
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_download_size;

    #[test]
    fn download_size_must_match_the_signed_manifest_value() {
        validate_download_size(9_153_970_381, Some(9_153_970_381))
            .expect("exact archive size should pass");
        assert!(validate_download_size(9_153_970_380, Some(9_153_970_381)).is_err());
        assert!(validate_download_size(9_153_970_382, Some(9_153_970_381)).is_err());
        validate_download_size(123, None).expect("legacy unbounded callers should still work");
    }
}
