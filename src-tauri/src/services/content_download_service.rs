use std::io::Read;
use std::path::Path;

use futures_util::StreamExt;
use reqwest::{header, Client, StatusCode};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::models::ContentChunk;
use crate::services::download_service::{validate_full_response, validate_partial_response};
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
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let part = target.with_extension(
        target
            .extension()
            .map(|extension| format!("{}.part", extension.to_string_lossy()))
            .unwrap_or_else(|| "part".to_string()),
    );
    let mut offset = tokio::fs::metadata(&part)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if offset > chunk.compressed_size {
        tokio::fs::remove_file(&part).await?;
        offset = 0;
    }
    if offset == chunk.compressed_size && offset > 0 {
        if verified_compressed_file(&part, chunk).await? {
            tokio::fs::rename(&part, target).await?;
            return Ok(());
        }
        tokio::fs::remove_file(&part).await?;
        offset = 0;
    }

    for _ in 0..2 {
        if cancel.is_cancelled() {
            return Err(AppError::Canceled);
        }
        let mut request = client.get(url);
        if offset > 0 {
            request = request.header(header::RANGE, format!("bytes={offset}-"));
        }
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::Canceled),
            result = request.send() => result?,
        };
        if response.status() == StatusCode::RANGE_NOT_SATISFIABLE && offset > 0 {
            tokio::fs::remove_file(&part).await?;
            offset = 0;
            continue;
        }
        if offset > 0 && response.status() == StatusCode::OK {
            offset = 0;
        } else if offset > 0 && response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(AppError::Network(format!(
                "content chunk request failed with HTTP {}",
                response.status()
            )));
        } else if offset == 0 && !response.status().is_success() {
            return Err(AppError::Network(format!(
                "content chunk request failed with HTTP {}",
                response.status()
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
        let mut output = options.open(&part).await?;
        let mut written = offset;
        let mut stream = response.bytes_stream();
        while let Some(next) = tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::Canceled),
            next = stream.next() => next,
        } {
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
        if written != chunk.compressed_size || !verified_compressed_file(&part, chunk).await? {
            tokio::fs::remove_file(&part).await.ok();
            return Err(AppError::InvalidData(
                "downloaded content chunk failed verification".into(),
            ));
        }
        tokio::fs::rename(&part, target).await?;
        return Ok(());
    }
    Err(AppError::Network(
        "content server rejected the resume request".into(),
    ))
}

pub async fn verified_compressed_file(path: &Path, chunk: &ContentChunk) -> Result<bool, AppError> {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return Ok(false);
    };
    if !metadata.is_file() || metadata.len() != chunk.compressed_size {
        return Ok(false);
    }
    Ok(sha256_file(path).await? == chunk.compressed_sha256)
}
