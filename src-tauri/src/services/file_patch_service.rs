use std::path::{Path, PathBuf};
use std::time::Duration;

use tauri::AppHandle;
use tokio_util::sync::CancellationToken;
use walkdir::WalkDir;

use crate::constants::TEMPORARY_FILES;
use crate::error::AppError;
use crate::utils::hash_utils::sha256_file;
use crate::utils::path_utils::resource_path;

pub async fn copy_files_and_track(
    source_root: PathBuf,
    target_root: PathBuf,
    temporary_mode: bool,
    cancel: Option<CancellationToken>,
) -> Result<Vec<PathBuf>, AppError> {
    let mut copied = Vec::new();

    for entry in WalkDir::new(&source_root).into_iter() {
        if let Some(cancel) = cancel.as_ref() {
            if cancel.is_cancelled() {
                return Err(AppError::Canceled);
            }
        }

        let entry = entry.map_err(|error| AppError::FileSystem(error.to_string()))?;
        if !entry.file_type().is_file() {
            continue;
        }

        let relative = entry
            .path()
            .strip_prefix(&source_root)
            .map_err(|error| AppError::FileSystem(error.to_string()))?;
        let target = target_root.join(relative);
        copy_one(entry.path(), &target).await?;
        copied.push(target.clone());

        if temporary_mode && is_temporary(relative) {
            let replacement_source = source_root
                .to_string_lossy()
                .replace("game_files_pure", "game_files");
            let replacement = PathBuf::from(replacement_source).join(relative);
            let target_clone = target.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                if let Some(cancel) = cancel {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(25_000)) => {}
                        _ = cancel.cancelled() => return,
                    }
                } else {
                    tokio::time::sleep(Duration::from_millis(25_000)).await;
                }
                let _ = copy_one(&replacement, &target_clone).await;
            });
        }
    }

    Ok(copied)
}

pub async fn delete_tracked_files(files: Vec<PathBuf>) -> Result<(), AppError> {
    for file in files {
        if tokio::fs::try_exists(&file).await.unwrap_or(false) {
            delete_one(&file).await?;
        }
    }
    Ok(())
}

pub async fn restore_game_files(
    source_root: PathBuf,
    target_root: PathBuf,
) -> Result<Vec<PathBuf>, AppError> {
    copy_files_and_track(source_root, target_root, false, None).await
}

pub fn bundled_game_files_pure(app: &AppHandle) -> PathBuf {
    resource_path(app, "game_files_pure")
}

pub fn bundled_game_files(app: &AppHandle) -> PathBuf {
    resource_path(app, "game_files")
}

async fn copy_one(source: &Path, target: &Path) -> Result<(), AppError> {
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    retry_operation(|| async {
        if is_locked(target).await? {
            return Err(AppError::FileSystem(format!(
                "target file is locked: {}",
                target.display()
            )));
        }
        tokio::fs::copy(source, target).await?;
        Ok(())
    })
    .await?;

    let source_hash = sha256_file(source).await?;
    let target_hash = sha256_file(target).await?;

    if source_hash != target_hash {
        return Err(AppError::FileSystem(format!(
            "checksum mismatch after copy: {}",
            target.display()
        )));
    }

    Ok(())
}

async fn delete_one(path: &Path) -> Result<(), AppError> {
    retry_operation(|| async {
        if is_locked(path).await? {
            return Err(AppError::FileSystem(format!(
                "target file is locked: {}",
                path.display()
            )));
        }
        tokio::fs::remove_file(path).await?;
        Ok(())
    })
    .await
}

async fn retry_operation<F, Fut>(mut operation: F) -> Result<(), AppError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), AppError>>,
{
    let mut last_error = None;
    for attempt in 0..3 {
        match operation().await {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(300 * 2_u64.pow(attempt))).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| AppError::FileSystem("file operation failed".to_string())))
}

async fn is_locked(path: &Path) -> Result<bool, AppError> {
    if !tokio::fs::try_exists(path).await.unwrap_or(false) {
        return Ok(false);
    }

    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map(|_| false)
            .or_else(|error| {
                if error.kind() == std::io::ErrorKind::PermissionDenied {
                    Ok(true)
                } else {
                    Err(AppError::FileSystem(format!(
                        "failed to check file lock for {}: {error}",
                        path.display()
                    )))
                }
            })
    })
    .await
    .map_err(|error| AppError::Unknown(error.to_string()))?
}

fn is_temporary(relative: &Path) -> bool {
    let normalized = relative.to_string_lossy().replace('/', "\\");
    TEMPORARY_FILES
        .iter()
        .any(|temporary| temporary.eq_ignore_ascii_case(&normalized))
}
