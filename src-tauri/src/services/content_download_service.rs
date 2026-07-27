use std::io::Read;
use std::path::Path;

use futures_util::StreamExt;
use reqwest::{header, Client, StatusCode};
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

pub fn decode_verified_chunk(
    compressed: &[u8],
    expected_raw_sha256: &str,
    chunk: &ContentChunk,
) -> Result<Vec<u8>, AppError> {
    if compressed.len() as u64 != chunk.compressed_size
        || hex::encode(Sha256::digest(compressed)) != chunk.compressed_sha256
    {
        return Err(AppError::InvalidData(
            "compressed content chunk failed verification".into(),
        ));
    }
    let decoder = zstd::stream::read::Decoder::new(compressed)
        .map_err(|error| AppError::InvalidData(format!("invalid zstd chunk: {error}")))?;
    let limit = chunk
        .uncompressed_size
        .checked_add(1)
        .ok_or_else(|| AppError::InvalidData("content chunk size overflow".into()))?;
    let mut raw = Vec::with_capacity(chunk.uncompressed_size as usize);
    decoder
        .take(limit)
        .read_to_end(&mut raw)
        .map_err(|error| AppError::InvalidData(format!("invalid zstd chunk: {error}")))?;
    if raw.len() as u64 != chunk.uncompressed_size
        || hex::encode(Sha256::digest(&raw)) != expected_raw_sha256
    {
        return Err(AppError::InvalidData(
            "raw content chunk failed verification".into(),
        ));
    }
    Ok(raw)
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
    let part = target.with_extension(
        target
            .extension()
            .map(|extension| format!("{}.part", extension.to_string_lossy()))
            .unwrap_or_else(|| "part".to_string()),
    );
    let mut network_retries = 0;
    let mut data_retries = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(AppError::Canceled);
        }
        match try_download_content_chunk(client, url, target, &part, chunk, &cancel).await {
            Ok(()) => return Ok(()),
            Err(AppError::Canceled) => return Err(AppError::Canceled),
            Err(error @ AppError::Network(_)) => {
                if network_retries >= MAX_NETWORK_RETRIES {
                    return Err(error);
                }
                network_retries += 1;
                wait_before_retry(&cancel, network_retries).await?;
            }
            Err(error @ AppError::InvalidData(_)) => {
                tokio::fs::remove_file(&part).await.ok();
                if data_retries >= MAX_DATA_RETRIES {
                    return Err(error);
                }
                data_retries += 1;
                wait_before_retry(&cancel, data_retries).await?;
            }
            Err(error) => return Err(error),
        }
    }
}

async fn try_download_content_chunk(
    client: &Client,
    url: &str,
    target: &Path,
    part: &Path,
    chunk: &ContentChunk,
    cancel: &CancellationToken,
) -> Result<(), AppError> {
    let mut offset = match tokio::fs::symlink_metadata(part).await {
        Ok(metadata) => {
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err(AppError::InvalidData(format!(
                    "content chunk partial is not a regular file: {}",
                    part.display()
                )));
            }
            metadata.len()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };
    if offset > chunk.compressed_size {
        tokio::fs::remove_file(part).await?;
        offset = 0;
    }
    if offset == chunk.compressed_size && offset > 0 {
        if verified_compressed_file(part, chunk).await? {
            tokio::fs::rename(part, target).await?;
            return Ok(());
        }
        tokio::fs::remove_file(part).await?;
        offset = 0;
    }

    let mut request = client.get(url);
    if offset > 0 {
        request = request.header(header::RANGE, format!("bytes={offset}-"));
    }
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err(AppError::Canceled),
        result = tokio::time::timeout(DOWNLOAD_IDLE_TIMEOUT, request.send()) => {
            match result {
                Ok(response) => response?,
                Err(_) => return Err(AppError::Network(
                    "content download timed out waiting for response headers".into(),
                )),
            }
        },
    };
    if response.status() == StatusCode::RANGE_NOT_SATISFIABLE && offset > 0 {
        tokio::fs::remove_file(part).await?;
        return Err(AppError::Network(
            "content server rejected the resume offset".into(),
        ));
    }
    let status = response.status();
    if offset > 0 && status == StatusCode::OK {
        offset = 0;
    }
    let invalid_status = (offset > 0 && status != StatusCode::PARTIAL_CONTENT)
        || (offset == 0 && !status.is_success());
    if invalid_status {
        return Err(AppError::Network(format!(
            "content chunk request failed with HTTP {status}",
        )));
    }
    if offset > 0 {
        validate_partial_response(&response, offset, Some(chunk.compressed_size))?;
    } else {
        validate_full_response(&response, Some(chunk.compressed_size))?;
    }

    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).write(true);
    if offset > 0 {
        options.append(true);
    } else {
        options.truncate(true);
    }
    let mut output = options.open(part).await?;
    let mut written = offset;
    let mut stream = response.bytes_stream();
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::Canceled),
            result = tokio::time::timeout(DOWNLOAD_IDLE_TIMEOUT, stream.next()) => {
                match result {
                    Ok(next) => next,
                    Err(_) => return Err(AppError::Network(
                        "content download stalled while waiting for data".into(),
                    )),
                }
            },
        };
        let Some(next) = next else {
            break;
        };
        let bytes = next?;
        written = written
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| AppError::InvalidData("content download size overflow".into()))?;
        if written > chunk.compressed_size {
            return Err(AppError::InvalidData(
                "content chunk exceeded its declared size".into(),
            ));
        }
        output.write_all(&bytes).await?;
    }
    output.flush().await?;
    drop(output);
    if written != chunk.compressed_size {
        return Err(AppError::Network(
            "content download ended before the declared size".into(),
        ));
    }
    if !verified_compressed_file(part, chunk).await? {
        return Err(AppError::InvalidData(
            "downloaded content chunk failed verification".into(),
        ));
    }
    tokio::fs::rename(part, target).await?;
    Ok(())
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
