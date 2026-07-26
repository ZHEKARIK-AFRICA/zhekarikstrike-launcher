use std::path::{Path, PathBuf};

use md5::Md5;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::error::AppError;

pub async fn md5_file(path: impl AsRef<Path>) -> Result<String, AppError> {
    hash_file(path.as_ref().to_path_buf(), HashKind::Md5).await
}

pub async fn sha256_file(path: impl AsRef<Path>) -> Result<String, AppError> {
    hash_file(path.as_ref().to_path_buf(), HashKind::Sha256).await
}

enum HashKind {
    Md5,
    Sha256,
}

async fn hash_file(path: PathBuf, kind: HashKind) -> Result<String, AppError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0_u8; 1024 * 128];

    match kind {
        HashKind::Md5 => {
            let mut hasher = Md5::new();
            loop {
                let read = file.read(&mut buffer).await?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            Ok(hex::encode(hasher.finalize()))
        }
        HashKind::Sha256 => {
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
    }
}
