use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};

use tokio_util::sync::CancellationToken;
use zip::ZipArchive;

use crate::error::AppError;
use crate::models::{ProgressEmitter, ProgressStage};

pub async fn extract_zip(
    archive_path: PathBuf,
    target_dir: PathBuf,
    progress: ProgressEmitter,
    cancel: CancellationToken,
) -> Result<(), AppError> {
    let archive_path_for_block = archive_path.clone();
    let target_dir_for_block = target_dir.clone();

    tokio::task::spawn_blocking(move || {
        let file = File::open(&archive_path_for_block)?;
        let mut archive = ZipArchive::new(file)?;
        let total = archive.len().max(1);

        for index in 0..archive.len() {
            if cancel.is_cancelled() {
                return Err(AppError::Canceled);
            }

            let mut entry = archive.by_index(index)?;
            let enclosed = safe_zip_path(&target_dir_for_block, entry.name())?;

            if entry.is_dir() {
                std::fs::create_dir_all(&enclosed)?;
            } else {
                if let Some(parent) = enclosed.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut out = File::create(&enclosed)?;
                io::copy(&mut entry, &mut out)?;
            }

            progress.emit_stage(
                ProgressStage::Extract,
                Some(((index + 1) as f64 / total as f64) * 100.0),
                Some(entry.name().to_string()),
            )?;
        }

        Ok::<(), AppError>(())
    })
    .await
    .map_err(|error| AppError::Unknown(error.to_string()))??;

    tokio::fs::remove_file(archive_path).await?;
    Ok(())
}

fn safe_zip_path(target_dir: &Path, entry_name: &str) -> Result<PathBuf, AppError> {
    let mut relative = PathBuf::new();
    for component in Path::new(entry_name).components() {
        match component {
            Component::Normal(value) => relative.push(value),
            Component::CurDir => {}
            _ => {
                return Err(AppError::InvalidData(format!(
                    "Unsafe zip entry path: {entry_name}"
                )))
            }
        }
    }

    Ok(target_dir.join(relative))
}
