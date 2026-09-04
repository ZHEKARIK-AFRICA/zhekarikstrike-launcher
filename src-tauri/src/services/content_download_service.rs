#![allow(dead_code)] // Loose v2 transport is retained only for recovery/downgrade compatibility.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::{header, Client, Response, StatusCode, Url};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::models::ContentChunk;
use crate::services::download_service::{
    validate_full_response, validate_partial_response, wait_before_retry, DOWNLOAD_IDLE_TIMEOUT,
    MAX_DATA_RETRIES, MAX_NETWORK_RETRIES,
};
use crate::utils::hash_utils::sha256_file;

const DRIVE_ATTEMPTS: u32 = 3;
const DRIVE_FAILURE_LIMIT: u32 = 3;

#[derive(Debug, Clone, Default)]
pub struct ChunkDownloadReport {
    pub network_bytes: u64,
    pub used_drive: bool,
    pub drive_failed: bool,
    pub drive_throttled: bool,
}

#[derive(Debug, Default)]
struct DriveCircuitState {
    failed_chunks: u32,
    disabled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DriveCircuitBreaker {
    state: Arc<Mutex<DriveCircuitState>>,
}

impl DriveCircuitBreaker {
    pub fn is_enabled(&self) -> bool {
        !self
            .state
            .lock()
            .expect("Drive circuit mutex should not be poisoned")
            .disabled
    }

    pub fn register_failed_chunk(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("Drive circuit mutex should not be poisoned");
        state.failed_chunks = state.failed_chunks.saturating_add(1);
        if state.failed_chunks >= DRIVE_FAILURE_LIMIT {
            state.disabled = true;
        }
        state.disabled
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttemptFailureKind {
    Retryable,
    Permanent,
    Integrity,
}

#[derive(Debug)]
struct AttemptFailure {
    error: AppError,
    kind: AttemptFailureKind,
    retry_after: Option<Duration>,
    throttled: bool,
    discard_part: bool,
    network_bytes: u64,
}

#[derive(Debug)]
struct AttemptSuccess {
    network_bytes: u64,
}

#[derive(Debug)]
struct SourceFailure {
    error: AppError,
    throttled: bool,
    discard_part: bool,
    network_bytes: u64,
}

pub fn decode_verified_chunk(
    compressed: &[u8],
    expected_raw_sha256: &str,
    chunk: &ContentChunk,
) -> Result<Vec<u8>, AppError> {
    content_pack_core::integrity::decode_verified(
        compressed,
        chunk.compressed_size,
        &chunk.compressed_sha256,
        chunk.uncompressed_size,
        expected_raw_sha256,
    )
    .map_err(Into::into)
}

pub async fn read_verified_local_chunk(
    path: &Path,
    offset: u64,
    expected_raw_sha256: &str,
    chunk: &ContentChunk,
) -> Result<Option<Vec<u8>>, AppError> {
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return Ok(None);
    };
    if file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
        return Ok(None);
    }
    let length = usize::try_from(chunk.uncompressed_size)
        .map_err(|_| AppError::InvalidData("content chunk is too large".into()))?;
    let mut bytes = vec![0_u8; length];
    if file.read_exact(&mut bytes).await.is_err() {
        return Ok(None);
    }
    if hex::encode(Sha256::digest(&bytes)) != expected_raw_sha256 {
        return Ok(None);
    }
    Ok(Some(bytes))
}

#[cfg(test)]
pub async fn download_content_chunk(
    client: &Client,
    url: &str,
    target: &Path,
    chunk: &ContentChunk,
    cancel: CancellationToken,
) -> Result<(), AppError> {
    if verified_compressed_file(target, chunk).await? {
        return Ok(());
    }
    prepare_chunk_target(target).await?;
    let part = partial_path(target);
    download_from_source(client, url, target, &part, chunk, false, &cancel)
        .await
        .map(|_| ())
        .map_err(|failure| failure.error)
}

#[allow(clippy::too_many_arguments)]
pub async fn download_content_chunk_with_fallback(
    drive_client: &Client,
    oracle_client: &Client,
    drive_url: Option<&str>,
    oracle_url: &str,
    target: &Path,
    chunk: &ContentChunk,
    circuit: &DriveCircuitBreaker,
    cancel: CancellationToken,
) -> Result<ChunkDownloadReport, AppError> {
    if verified_compressed_file(target, chunk).await? {
        return Ok(ChunkDownloadReport::default());
    }
    prepare_chunk_target(target).await?;
    let part = partial_path(target);
    let mut report = ChunkDownloadReport::default();

    if let Some(url) = drive_url.filter(|_| circuit.is_enabled()) {
        validate_drive_url(url)?;
        match download_from_source(drive_client, url, target, &part, chunk, true, &cancel).await {
            Ok(success) => {
                report.network_bytes = success.network_bytes;
                report.used_drive = true;
                return Ok(report);
            }
            Err(failure) => {
                if matches!(&failure.error, AppError::Canceled) {
                    return Err(AppError::Canceled);
                }
                report.network_bytes = report.network_bytes.saturating_add(failure.network_bytes);
                report.drive_failed = true;
                report.drive_throttled = failure.throttled;
                if failure.discard_part {
                    tokio::fs::remove_file(&part).await.ok();
                }
                let disabled = circuit.register_failed_chunk();
                crate::logger::warn(&format!(
                    "Google Drive content chunk failed; switching to Oracle ({})",
                    failure.error
                ));
                if disabled {
                    crate::logger::warn(
                        "Google Drive content mirror disabled for the rest of this operation",
                    );
                }
            }
        }
    }

    let success = download_from_source(
        oracle_client,
        oracle_url,
        target,
        &part,
        chunk,
        false,
        &cancel,
    )
    .await
    .map_err(|failure| failure.error)?;
    report.network_bytes = report.network_bytes.saturating_add(success.network_bytes);
    Ok(report)
}

async fn prepare_chunk_target(target: &Path) -> Result<(), AppError> {
    if let Ok(metadata) = tokio::fs::symlink_metadata(target).await {
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(AppError::InvalidData(format!(
                "content chunk target is not a regular file: {}",
                target.display()
            )));
        }
        tokio::fs::remove_file(target).await?;
    }
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    Ok(())
}

async fn download_from_source(
    client: &Client,
    url: &str,
    target: &Path,
    part: &Path,
    chunk: &ContentChunk,
    drive: bool,
    cancel: &CancellationToken,
) -> Result<AttemptSuccess, SourceFailure> {
    let max_network_retries = if drive {
        DRIVE_ATTEMPTS.saturating_sub(1)
    } else {
        MAX_NETWORK_RETRIES
    };
    let max_data_retries = if drive { 0 } else { MAX_DATA_RETRIES };
    let mut network_retries = 0;
    let mut data_retries = 0;
    let mut network_bytes = 0_u64;
    loop {
        if cancel.is_cancelled() {
            return Err(SourceFailure {
                error: AppError::Canceled,
                throttled: false,
                discard_part: false,
                network_bytes,
            });
        }
        match try_download_content_chunk(client, url, target, part, chunk, drive, cancel).await {
            Ok(mut success) => {
                success.network_bytes = success.network_bytes.saturating_add(network_bytes);
                return Ok(success);
            }
            Err(failure) => {
                network_bytes = network_bytes.saturating_add(failure.network_bytes);
                if failure.discard_part {
                    tokio::fs::remove_file(part).await.ok();
                }
                let retry = match failure.kind {
                    AttemptFailureKind::Permanent => None,
                    AttemptFailureKind::Retryable if network_retries < max_network_retries => {
                        network_retries += 1;
                        Some(network_retries)
                    }
                    AttemptFailureKind::Integrity if data_retries < max_data_retries => {
                        data_retries += 1;
                        Some(data_retries)
                    }
                    _ => None,
                };
                let Some(retry_number) = retry else {
                    return Err(SourceFailure {
                        error: failure.error,
                        throttled: failure.throttled,
                        discard_part: failure.discard_part,
                        network_bytes,
                    });
                };
                if let Err(error) =
                    wait_for_source_retry(cancel, retry_number, failure.retry_after, drive).await
                {
                    return Err(SourceFailure {
                        error,
                        throttled: failure.throttled,
                        discard_part: false,
                        network_bytes,
                    });
                }
            }
        }
    }
}

async fn wait_for_source_retry(
    cancel: &CancellationToken,
    retry_number: u32,
    retry_after: Option<Duration>,
    drive: bool,
) -> Result<(), AppError> {
    if drive {
        let fallback = Duration::from_millis(500_u64.saturating_mul(1 << retry_number.min(5)));
        let delay = retry_after.unwrap_or(fallback).min(Duration::from_secs(30));
        tokio::select! {
            _ = cancel.cancelled() => Err(AppError::Canceled),
            _ = tokio::time::sleep(delay) => Ok(()),
        }
    } else {
        wait_before_retry(cancel, retry_number).await
    }
}

async fn try_download_content_chunk(
    client: &Client,
    url: &str,
    target: &Path,
    part: &Path,
    chunk: &ContentChunk,
    drive: bool,
    cancel: &CancellationToken,
) -> Result<AttemptSuccess, AttemptFailure> {
    let (mut offset, mut hasher) = inspect_partial(part, chunk).await.map_err(integrity)?;
    if offset == chunk.compressed_size && offset > 0 {
        if hex::encode(hasher.finalize()) == chunk.compressed_sha256 {
            tokio::fs::rename(part, target).await.map_err(fatal)?;
            return Ok(AttemptSuccess { network_bytes: 0 });
        }
        tokio::fs::remove_file(part).await.map_err(fatal)?;
        offset = 0;
        hasher = Sha256::new();
    }

    let mut request = client.get(url);
    if offset > 0 {
        request = request.header(header::RANGE, format!("bytes={offset}-"));
    }
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err(cancelled()),
        result = tokio::time::timeout(DOWNLOAD_IDLE_TIMEOUT, request.send()) => {
            match result {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => {
                    let pressured = error.is_timeout() || error.is_connect();
                    return Err(retryable(AppError::from(error), None, pressured));
                }
                Err(_) => return Err(retryable(AppError::Network(
                    "content download timed out waiting for response headers".into(),
                ), None, true)),
            }
        },
    };

    let status = response.status();
    if drive && matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) {
        return Err(permanent(format!(
            "Google Drive content request failed with HTTP {status}"
        )));
    }
    if matches!(
        status,
        StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
    {
        let retry_after = retry_after(&response);
        return Err(retryable(
            AppError::Network(format!("content chunk request failed with HTTP {status}")),
            retry_after,
            true,
        ));
    }
    if status == StatusCode::RANGE_NOT_SATISFIABLE && offset > 0 {
        return Err(AttemptFailure {
            error: AppError::Network("content server rejected the resume offset".into()),
            kind: if drive {
                AttemptFailureKind::Permanent
            } else {
                AttemptFailureKind::Retryable
            },
            retry_after: None,
            throttled: false,
            discard_part: true,
            network_bytes: 0,
        });
    }
    if drive && status.is_redirection() {
        return Err(permanent("Google Drive content redirect was rejected"));
    }
    if offset > 0 && status == StatusCode::OK {
        if drive {
            return Err(integrity(AppError::InvalidData(
                "Google Drive ignored the requested content range".into(),
            )));
        }
        offset = 0;
        hasher = Sha256::new();
    }
    let invalid_status = (offset > 0 && status != StatusCode::PARTIAL_CONTENT)
        || (offset == 0 && !status.is_success());
    if invalid_status {
        return Err(permanent(format!(
            "content chunk request failed with HTTP {status}"
        )));
    }
    validate_response(&response, offset, chunk.compressed_size).map_err(integrity)?;

    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).write(true);
    if offset > 0 {
        options.append(true);
    } else {
        options.truncate(true);
    }
    let mut output = options.open(part).await.map_err(fatal)?;
    let mut written = offset;
    let mut network_bytes = 0_u64;
    let mut stream = response.bytes_stream();
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled()),
            result = tokio::time::timeout(DOWNLOAD_IDLE_TIMEOUT, stream.next()) => {
                match result {
                    Ok(next) => next,
                    Err(_) => return Err(with_network_bytes(
                        retryable(AppError::Network(
                            "content download stalled while waiting for data".into(),
                        ), None, true),
                        network_bytes,
                    )),
                }
            },
        };
        let Some(next) = next else {
            break;
        };
        let bytes = next.map_err(|error| {
            let pressured = error.is_timeout() || error.is_connect();
            with_network_bytes(retryable(error.into(), None, pressured), network_bytes)
        })?;
        written = written.checked_add(bytes.len() as u64).ok_or_else(|| {
            with_network_bytes(
                integrity(AppError::InvalidData(
                    "content download size overflow".into(),
                )),
                network_bytes,
            )
        })?;
        network_bytes = network_bytes.saturating_add(bytes.len() as u64);
        if written > chunk.compressed_size {
            return Err(with_network_bytes(
                integrity(AppError::InvalidData(
                    "content chunk exceeded its declared size".into(),
                )),
                network_bytes,
            ));
        }
        hasher.update(&bytes);
        output.write_all(&bytes).await.map_err(fatal)?;
    }
    output.flush().await.map_err(fatal)?;
    drop(output);
    if written != chunk.compressed_size {
        return Err(with_network_bytes(
            retryable(
                AppError::Network("content download ended before the declared size".into()),
                None,
                false,
            ),
            network_bytes,
        ));
    }
    if hex::encode(hasher.finalize()) != chunk.compressed_sha256 {
        return Err(AttemptFailure {
            error: AppError::InvalidData("downloaded content chunk failed verification".into()),
            kind: AttemptFailureKind::Integrity,
            retry_after: None,
            throttled: false,
            discard_part: true,
            network_bytes,
        });
    }
    tokio::fs::rename(part, target).await.map_err(fatal)?;
    Ok(AttemptSuccess { network_bytes })
}

async fn inspect_partial(part: &Path, chunk: &ContentChunk) -> Result<(u64, Sha256), AppError> {
    let metadata = match tokio::fs::symlink_metadata(part).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((0, Sha256::new()));
        }
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(AppError::InvalidData(format!(
            "content chunk partial is not a regular file: {}",
            part.display()
        )));
    }
    if metadata.len() > chunk.compressed_size {
        tokio::fs::remove_file(part).await?;
        return Ok((0, Sha256::new()));
    }
    let mut file = tokio::fs::File::open(part).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok((metadata.len(), hasher))
}

fn validate_response(response: &Response, offset: u64, total: u64) -> Result<(), AppError> {
    if offset > 0 {
        validate_partial_response(response, offset, Some(total))
    } else {
        validate_full_response(response, Some(total))
    }
}

fn retry_after(response: &Response) -> Option<Duration> {
    response
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.min(30)))
}

fn validate_drive_url(value: &str) -> Result<(), AppError> {
    let url = Url::parse(value)
        .map_err(|_| AppError::InvalidData("invalid Google Drive content URL".into()))?;
    let query = url.query_pairs().collect::<Vec<_>>();
    let valid_query = query.len() == 3
        && query.iter().any(|(key, value)| {
            key == "id"
                && (10..=128).contains(&value.len())
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
        && query
            .iter()
            .any(|(key, value)| key == "export" && value == "download")
        && query
            .iter()
            .any(|(key, value)| key == "confirm" && value == "t");
    if url.scheme() != "https"
        || url.host_str() != Some("drive.usercontent.google.com")
        || url.path() != "/download"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !valid_query
    {
        return Err(AppError::InvalidData(
            "untrusted Google Drive content URL".into(),
        ));
    }
    Ok(())
}

fn partial_path(target: &Path) -> std::path::PathBuf {
    target.with_extension(
        target
            .extension()
            .map(|extension| format!("{}.part", extension.to_string_lossy()))
            .unwrap_or_else(|| "part".to_string()),
    )
}

fn retryable(error: AppError, retry_after: Option<Duration>, throttled: bool) -> AttemptFailure {
    AttemptFailure {
        error,
        kind: AttemptFailureKind::Retryable,
        retry_after,
        throttled,
        discard_part: false,
        network_bytes: 0,
    }
}

fn integrity(error: AppError) -> AttemptFailure {
    AttemptFailure {
        error,
        kind: AttemptFailureKind::Integrity,
        retry_after: None,
        throttled: false,
        discard_part: true,
        network_bytes: 0,
    }
}

fn permanent(message: impl Into<String>) -> AttemptFailure {
    AttemptFailure {
        error: AppError::Network(message.into()),
        kind: AttemptFailureKind::Permanent,
        retry_after: None,
        throttled: false,
        discard_part: false,
        network_bytes: 0,
    }
}

fn fatal(error: impl Into<AppError>) -> AttemptFailure {
    AttemptFailure {
        error: error.into(),
        kind: AttemptFailureKind::Permanent,
        retry_after: None,
        throttled: false,
        discard_part: false,
        network_bytes: 0,
    }
}

fn cancelled() -> AttemptFailure {
    AttemptFailure {
        error: AppError::Canceled,
        kind: AttemptFailureKind::Permanent,
        retry_after: None,
        throttled: false,
        discard_part: false,
        network_bytes: 0,
    }
}

fn with_network_bytes(mut failure: AttemptFailure, network_bytes: u64) -> AttemptFailure {
    failure.network_bytes = network_bytes;
    failure
}

pub async fn verified_compressed_file(path: &Path, chunk: &ContentChunk) -> Result<bool, AppError> {
    let Ok(metadata) = tokio::fs::symlink_metadata(path).await else {
        return Ok(false);
    };
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() != chunk.compressed_size
    {
        return Ok(false);
    }
    Ok(sha256_file(path).await? == chunk.compressed_sha256)
}

#[cfg(test)]
mod tests {
    use super::validate_drive_url;

    #[test]
    fn drive_url_allows_only_the_fixed_google_download_confirmation() {
        assert!(validate_drive_url(
            "https://drive.usercontent.google.com/download?id=1O6eniBjd9dd1ES-j1OKuVRXmKL6ke4vE&export=download&confirm=t"
        )
        .is_ok());
        assert!(validate_drive_url(
            "https://drive.usercontent.google.com/download?id=1O6eniBjd9dd1ES-j1OKuVRXmKL6ke4vE&export=download&confirm=false"
        )
        .is_err());
        assert!(validate_drive_url(
            "https://drive.usercontent.google.com/download?id=1O6eniBjd9dd1ES-j1OKuVRXmKL6ke4vE&export=download&confirm=t&next=https://attacker.invalid"
        )
        .is_err());
    }
}
