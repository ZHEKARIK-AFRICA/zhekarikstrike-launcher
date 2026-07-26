use std::path::Path;

use serde::{de::DeserializeOwned, Serialize};

use crate::error::AppError;

pub async fn read_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T, AppError> {
    if !path.exists() {
        return Ok(T::default());
    }

    let content = tokio::fs::read_to_string(path).await?;
    if content.trim().is_empty() {
        return Ok(T::default());
    }

    Ok(serde_json::from_str(&content)?)
}

pub async fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let content = serde_json::to_string_pretty(value)?;
    tokio::fs::write(path, content).await?;
    Ok(())
}
