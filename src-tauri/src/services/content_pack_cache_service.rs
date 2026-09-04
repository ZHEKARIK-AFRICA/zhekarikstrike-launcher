use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;

use crate::error::AppError;
use crate::models::validate_sha256;
use crate::services::content_journal_service::content_root;
use crate::services::content_pack_plan_service::ByteRange;

#[derive(Debug, Clone)]
pub struct PackCache {
    root: PathBuf,
    transaction_id: String,
}

#[derive(Debug)]
pub struct PackClaim {
    path: PathBuf,
}

impl Drop for PackClaim {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl PackCache {
    pub async fn new(
        game_path: &Path,
        content_sha256: &str,
        transaction_id: &str,
    ) -> Result<Self, AppError> {
        validate_sha256(content_sha256, "pack cache content")?;
        if transaction_id.is_empty()
            || !transaction_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(AppError::InvalidData(
                "invalid pack cache transaction identifier".into(),
            ));
        }
        let root = content_root(game_path)
            .join("pack-cache")
            .join(content_sha256);
        tokio::fs::create_dir_all(root.join("full")).await?;
        tokio::fs::create_dir_all(root.join("ranges")).await?;
        // A crashed process cannot retain handles. Clean only our uniquely named
        // retired generations; normal .part/verified files remain resumable.
        for directory in [root.join("full"), root.join("ranges")] {
            let mut entries = tokio::fs::read_dir(directory).await?;
            while let Some(entry) = entries.next_entry().await? {
                let name = entry.file_name().to_string_lossy().into_owned();
                let parts = name.rsplit('.').collect::<Vec<_>>();
                if parts.first() == Some(&"retired")
                    && parts
                        .get(1)
                        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
                {
                    Self::regular_file_size(&entry.path()).await?;
                    Self::discard(&entry.path()).await?;
                }
            }
        }
        let claims = root.join("claims");
        tokio::fs::create_dir_all(&claims).await?;
        let mut entries = tokio::fs::read_dir(&claims).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            if !file_type.is_file() || file_type.is_symlink() {
                return Err(AppError::InvalidData(format!(
                    "pack cache claim is not a regular file: {}",
                    entry.path().display()
                )));
            }
            tokio::fs::remove_file(entry.path()).await?;
        }
        Ok(Self {
            root,
            transaction_id: transaction_id.to_string(),
        })
    }

    pub async fn claim(&self, pack_sha256: &str) -> Result<PackClaim, AppError> {
        validate_sha256(pack_sha256, "claimed content pack")?;
        let path = self
            .root
            .join("claims")
            .join(format!("{pack_sha256}.claim"));
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
            .map_err(|error| {
                AppError::FileSystem(format!("could not claim content pack: {error}"))
            })?;
        file.write_all(self.transaction_id.as_bytes()).await?;
        file.flush().await?;
        Ok(PackClaim { path })
    }

    pub async fn has_resume_data(&self) -> Result<bool, AppError> {
        for directory in [self.root.join("full"), self.root.join("ranges")] {
            let mut entries = tokio::fs::read_dir(directory).await?;
            if entries.next_entry().await?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn full_path(&self, pack_sha256: &str) -> Result<PathBuf, AppError> {
        validate_sha256(pack_sha256, "cached content pack")?;
        Ok(self.root.join("full").join(format!("{pack_sha256}.pack")))
    }

    pub fn full_partial_path(&self, pack_sha256: &str) -> Result<PathBuf, AppError> {
        Ok(self.full_path(pack_sha256)?.with_extension("pack.part"))
    }

    pub fn range_path(&self, pack_sha256: &str, range: ByteRange) -> Result<PathBuf, AppError> {
        validate_sha256(pack_sha256, "cached content pack range")?;
        range.len()?;
        Ok(self.root.join("ranges").join(format!(
            "{pack_sha256}-{}-{}.range",
            range.start, range.end_inclusive
        )))
    }

    pub fn range_partial_path(
        &self,
        pack_sha256: &str,
        range: ByteRange,
    ) -> Result<PathBuf, AppError> {
        Ok(self
            .range_path(pack_sha256, range)?
            .with_extension("range.part"))
    }

    pub async fn regular_file_size(path: &Path) -> Result<Option<u64>, AppError> {
        match tokio::fs::symlink_metadata(path).await {
            Ok(metadata)
                if metadata.file_type().is_file() && !metadata.file_type().is_symlink() =>
            {
                Ok(Some(metadata.len()))
            }
            Ok(_) => Err(AppError::InvalidData(format!(
                "pack cache entry is not a regular file: {}",
                path.display()
            ))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn discard(path: &Path) -> Result<(), AppError> {
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn promote(partial: &Path, verified: &Path) -> Result<(), AppError> {
        if let Some(parent) = verified.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        if Self::regular_file_size(verified).await?.is_some() {
            Self::discard(verified).await?;
        }
        tokio::fs::rename(partial, verified).await?;
        Ok(())
    }
}
