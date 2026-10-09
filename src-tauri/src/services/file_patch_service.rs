use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;
use walkdir::WalkDir;

use crate::error::AppError;
use crate::utils::hash_utils::sha256_file;

pub async fn copy_files_and_track(
    source_root: PathBuf,
    target_root: PathBuf,
    cancel: Option<CancellationToken>,
) -> Result<Vec<PathBuf>, AppError> {
    let mut copied = Vec::new();

    let copy_result = async {
        for entry in WalkDir::new(&source_root).sort_by_file_name().into_iter() {
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
            if super::user_settings_service::is_user_settings_path(&relative.to_string_lossy()) {
                seed_user_settings(entry.path(), &target).await?;
                // Persistent defaults never enter launch cleanup or delayed replacement.
                continue;
            }
            copy_one(entry.path(), &target).await?;
            copied.push(target.clone());
        }
        Ok::<(), AppError>(())
    }
    .await;

    if let Err(error) = copy_result {
        if let Err(cleanup_error) = delete_tracked_files(copied).await {
            return Err(AppError::FileSystem(format!(
                "layer activation failed ({error}) and rollback failed ({cleanup_error})"
            )));
        }
        return Err(error);
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
    if !tokio::fs::try_exists(&source_root).await.unwrap_or(false) {
        return Ok(Vec::new());
    }
    copy_files_and_track(source_root, target_root, None).await
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

async fn seed_user_settings(source: &Path, target: &Path) -> Result<(), AppError> {
    use tokio::io::AsyncWriteExt;
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // create_new is atomic: another writer cannot be truncated between probe and copy.
    let mut output = match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .await
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if !tokio::fs::symlink_metadata(target).await?.is_file() {
                return Err(AppError::InvalidData(
                    "user settings path is not a regular file".into(),
                ));
            }
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let result = async {
        output.write_all(&tokio::fs::read(source).await?).await?;
        output.flush().await?;
        Ok::<(), AppError>(())
    }
    .await;
    drop(output);
    if result.is_err() {
        tokio::fs::remove_file(target).await?;
    }
    result
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
                    tokio::time::sleep(std::time::Duration::from_millis(300 * 2_u64.pow(attempt)))
                        .await;
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

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{copy_files_and_track, restore_game_files};

    #[tokio::test]
    async fn pure_items_remain_active_until_session_cleanup() {
        let root = tempdir().unwrap();
        let pure = root.path().join("game_files_pure");
        let normal = root.path().join("game_files");
        let game = root.path().join("game");
        let relative = "csgo/scripts/items/items_game.txt";
        fs::create_dir_all(pure.join("csgo/scripts/items")).unwrap();
        fs::create_dir_all(normal.join("csgo/scripts/items")).unwrap();
        fs::write(pure.join(relative), b"server-compatible pure items").unwrap();
        fs::write(normal.join(relative), b"custom normal items").unwrap();
        let tracked = copy_files_and_track(pure, game.clone(), None)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(26)).await;
        assert_eq!(
            fs::read(game.join(relative)).unwrap(),
            b"server-compatible pure items",
            "elapsed startup time must not replace the active session overlay"
        );
        super::delete_tracked_files(tracked).await.unwrap();
        restore_game_files(normal, game.clone()).await.unwrap();
        assert_eq!(
            fs::read(game.join(relative)).unwrap(),
            b"custom normal items"
        );
    }

    #[tokio::test]
    async fn user_settings_seed_once_and_survive_launch_cleanup_and_updated_defaults() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("game_files_pure");
        let target = directory.path().join("game");
        fs::create_dir_all(source.join("csgo/cfg")).unwrap();
        fs::write(source.join("csgo/cfg/config.cfg"), b"packaged defaults").unwrap();
        let tracked = copy_files_and_track(source.clone(), target.clone(), None)
            .await
            .unwrap();
        assert_eq!(
            fs::read(target.join("csgo/cfg/config.cfg")).unwrap(),
            b"packaged defaults"
        );
        assert!(
            tracked.is_empty(),
            "persistent defaults must never enter launch cleanup"
        );
        fs::write(
            target.join("csgo/cfg/config.cfg"),
            b"my binds and sensitivity",
        )
        .unwrap();
        fs::write(source.join("csgo/cfg/config.cfg"), b"updated defaults").unwrap();
        let tracked = copy_files_and_track(source, target.clone(), None)
            .await
            .unwrap();
        super::delete_tracked_files(tracked).await.unwrap();
        assert_eq!(
            fs::read(target.join("csgo/cfg/config.cfg")).unwrap(),
            b"my binds and sensitivity"
        );
    }

    #[tokio::test]
    async fn missing_cached_game_files_are_a_cleanup_noop() {
        let directory = tempdir().expect("temporary directory should exist");
        let restored = restore_game_files(
            directory.path().join("missing-cache"),
            directory.path().join("game"),
        )
        .await
        .expect("an uninitialized cache should not fail cleanup");

        assert!(restored.is_empty());
    }

    #[tokio::test]
    async fn cached_game_files_are_restored_into_the_game_directory() {
        let directory = tempdir().expect("temporary directory should exist");
        let source = directory.path().join("game_files/csgo");
        let target = directory.path().join("game");
        fs::create_dir_all(&source).expect("cache directory should exist");
        fs::write(source.join("base.bin"), b"base").expect("cache file should exist");

        let restored = restore_game_files(directory.path().join("game_files"), target.clone())
            .await
            .expect("cache should restore");

        assert_eq!(restored, vec![target.join("csgo/base.bin")]);
        assert_eq!(
            fs::read(target.join("csgo/base.bin")).expect("restored file should exist"),
            b"base"
        );
    }

    #[tokio::test]
    async fn failed_layer_activation_removes_files_copied_before_the_error() {
        let directory = tempdir().expect("temporary directory should exist");
        let source = directory.path().join("pure");
        let target = directory.path().join("game");
        fs::create_dir_all(&source).expect("source should exist");
        fs::create_dir_all(target.join("b.bin")).expect("collision directory should exist");
        fs::write(source.join("a.bin"), b"copied first").expect("first source should exist");
        fs::write(source.join("b.bin"), b"must fail").expect("second source should exist");

        assert!(copy_files_and_track(source, target.clone(), None)
            .await
            .is_err());
        assert!(
            !target.join("a.bin").exists(),
            "a partially activated pure layer must be rolled back"
        );
    }
}
