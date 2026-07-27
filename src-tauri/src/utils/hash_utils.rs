use std::path::Path;

use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

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
