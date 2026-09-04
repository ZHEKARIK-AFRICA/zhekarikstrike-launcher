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
        tokio::fs::create_dir_all(root.join("plans")).await?;
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

    /// Freeze only when a job starts. Dynamic measurements never change the
    /// layout underneath an existing partial file.
    pub async fn freeze_plan(
        &self,
        manifest: &crate::models::DrivePackManifest,
        proposed: super::content_pack_plan_service::PackFetchPlan,
    ) -> Result<super::content_pack_plan_service::PackFetchPlan, AppError> {
        use super::content_pack_plan_service::{
            legacy_plan_pack_fetches, validate_fetch_plan, PackFetchPlan, PackTransferMode,
        };
        #[derive(serde::Serialize, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Saved {
            schema_version: u32,
            manifest_sha256: String,
            plan: PackFetchPlan,
        }
        validate_fetch_plan(manifest, &proposed)?;
        // std/Tokio support long paths, but the atomic Win32 rename needs the
        // verbatim prefix returned by canonicalize for our nested CAS paths.
        let path = tokio::fs::canonicalize(self.root.join("plans"))
            .await?
            .join(format!("{}.json", proposed.pack_sha256));
        let mut selected = proposed.clone();
        if let Some(size) = Self::regular_file_size(&path).await? {
            if size > 1024 * 1024 {
                return Err(AppError::InvalidData("oversized frozen pack plan".into()));
            }
            let saved: Saved = serde_json::from_slice(&tokio::fs::read(&path).await?)?;
            if saved.schema_version != 1
                || saved.manifest_sha256 != manifest.manifest_sha256
                || saved.plan.pack_sha256 != proposed.pack_sha256
            {
                return Err(AppError::InvalidData(
                    "frozen pack plan identity mismatch".into(),
                ));
            }
            validate_fetch_plan(manifest, &saved.plan)?;
            selected.mode = saved.plan.mode;
            // A later repair can need additional chunks. Preserve complete old
            // ranges and append only missing chunk spans; never rewrite them.
            if validate_fetch_plan(manifest, &selected).is_err() {
                if let PackTransferMode::Ranges(ranges) = &mut selected.mode {
                    for raw in &selected.required_chunks {
                        let c = &manifest.chunks[raw];
                        if !ranges
                            .iter()
                            .any(|r| r.contains(c.offset, c.compressed_size))
                        {
                            ranges.push(ByteRange {
                                start: c.offset,
                                end_inclusive: c.offset + c.compressed_size - 1,
                            });
                        }
                    }
                    ranges.sort_by_key(|r| r.start);
                }
            }
        } else if Self::regular_file_size(&self.full_partial_path(&selected.pack_sha256)?)
            .await?
            .is_some()
            || Self::regular_file_size(&self.full_path(&selected.pack_sha256)?)
                .await?
                .is_some()
        {
            selected.mode = PackTransferMode::Full;
        } else {
            let legacy = legacy_plan_pack_fetches(manifest, &selected.required_chunks)?.remove(0);
            if let PackTransferMode::Ranges(ranges) = &legacy.mode {
                for range in ranges {
                    if Self::regular_file_size(
                        &self.range_partial_path(&selected.pack_sha256, *range)?,
                    )
                    .await?
                    .is_some()
                        || Self::regular_file_size(&self.range_path(&selected.pack_sha256, *range)?)
                            .await?
                            .is_some()
                    {
                        selected.mode = legacy.mode.clone();
                        break;
                    }
                }
            }
        }
        validate_fetch_plan(manifest, &selected)?;
        super::content_journal_service::atomic_json(
            &path,
            &Saved {
                schema_version: 1,
                manifest_sha256: manifest.manifest_sha256.clone(),
                plan: selected.clone(),
            },
        )
        .await?;
        Ok(selected)
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
