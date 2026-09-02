use std::path::{Path, PathBuf};

use crate::error::AppError;
use crate::models::{ContentInventory, ContentManifest, DrivePackManifest};
use crate::services::content_journal_service::{atomic_json, content_root};

pub fn inventory_path(game_path: &Path, content_sha256: &str) -> PathBuf {
    content_root(game_path)
        .join("inventories")
        .join(format!("{content_sha256}.json"))
}

pub async fn save_content_inventory(
    game_path: &Path,
    inventory: &ContentInventory,
) -> Result<(), AppError> {
    inventory.validate()?;
    atomic_json(
        &inventory_path(game_path, &inventory.content_sha256),
        inventory,
    )
    .await
}

pub async fn load_content_inventory(
    game_path: &Path,
    content_sha256: &str,
) -> Result<Option<ContentInventory>, AppError> {
    crate::models::validate_sha256(content_sha256, "content inventory")?;
    let path = inventory_path(game_path, content_sha256);
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let inventory: ContentInventory = serde_json::from_slice(&bytes).map_err(|error| {
        AppError::InvalidData(format!("invalid persisted content inventory: {error}"))
    })?;
    inventory.validate()?;
    if inventory.content_sha256 != content_sha256 {
        return Err(AppError::InvalidData(
            "persisted content inventory path does not match its identity".into(),
        ));
    }
    Ok(Some(inventory))
}

pub async fn persist_v3_inventory(
    game_path: &Path,
    manifest: &DrivePackManifest,
) -> Result<ContentInventory, AppError> {
    let inventory = ContentInventory::from_v3(manifest)?;
    save_content_inventory(game_path, &inventory).await?;
    Ok(inventory)
}

pub async fn migrate_persisted_v2_manifest(
    game_path: &Path,
    content_sha256: &str,
    release_id: &str,
) -> Result<Option<ContentInventory>, AppError> {
    if let Some(inventory) = load_content_inventory(game_path, content_sha256).await? {
        return Ok(Some(inventory));
    }
    let path = content_root(game_path)
        .join("manifests")
        .join(format!("{content_sha256}.json"));
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let manifest: ContentManifest = serde_json::from_slice(&bytes).map_err(|error| {
        AppError::InvalidData(format!("invalid persisted v2 content manifest: {error}"))
    })?;
    manifest.validate()?;
    if manifest.content_sha256 != content_sha256 || manifest.release_id != release_id {
        return Err(AppError::InvalidData(
            "persisted v2 content manifest identity is invalid".into(),
        ));
    }
    let inventory = ContentInventory::from_v2(&manifest)?;
    save_content_inventory(game_path, &inventory).await?;
    Ok(Some(inventory))
}
