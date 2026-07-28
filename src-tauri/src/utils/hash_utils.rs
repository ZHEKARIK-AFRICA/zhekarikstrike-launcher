use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::error::AppError;

pub async fn sha256_file(path: impl AsRef<Path>) -> Result<String, AppError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0_u8; 1024 * 128];

    let mut hasher = Sha256::new();
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub async fn sha256_file_tracked(
    path: impl AsRef<Path>,
    cancel: &CancellationToken,
    completed_bytes: &AtomicU64,
) -> Result<String, AppError> {
    if cancel.is_cancelled() {
        return Err(AppError::Canceled);
    }

    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut hasher = Sha256::new();

    loop {
        if cancel.is_cancelled() {
            return Err(AppError::Canceled);
        }

        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        completed_bytes.fetch_add(read as u64, Ordering::Relaxed);
    }

    Ok(hex::encode(hasher.finalize()))
}
