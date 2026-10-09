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
                let output = if super::user_settings_service::is_user_settings_path(entry.name()) {
                    match std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&enclosed)
                    {
                        Ok(file) => Some(file),
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                            if !std::fs::symlink_metadata(&enclosed)?.is_file() {
                                return Err(AppError::InvalidData(
                                    "user settings path is not a regular file".into(),
                                ));
                            }
                            None
                        }
                        Err(error) => return Err(error.into()),
                    }
                } else {
                    Some(File::create(&enclosed)?)
                };
                if let Some(mut out) = output {
                    if let Err(error) = io::copy(&mut entry, &mut out) {
                        drop(out);
                        std::fs::remove_file(&enclosed)?;
                        return Err(error.into());
                    }
                }
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::safe_zip_path;

    #[tokio::test]
    async fn archive_settings_defaults_do_not_replace_a_partial_install_users_edits() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("client.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        for name in ["csgo/cfg/config.cfg", "csgo/cfg/autoexec.cfg"] {
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"packaged defaults").unwrap();
        }
        writer.finish().unwrap();
        let game = root.path().join("game");
        std::fs::create_dir_all(game.join("csgo/cfg")).unwrap();
        std::fs::write(game.join("csgo/cfg/config.cfg"), b"existing edits").unwrap();
        super::extract_zip(
            archive,
            game.clone(),
            crate::models::ProgressEmitter::headless("settings"),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(game.join("csgo/cfg/config.cfg")).unwrap(),
            b"existing edits"
        );
        assert_eq!(
            std::fs::read(game.join("csgo/cfg/autoexec.cfg")).unwrap(),
            b"packaged defaults"
        );
    }

    #[test]
    fn zip_entries_cannot_escape_the_target_directory() {
        let root = Path::new(r"D:\Games\ZS");
        assert_eq!(
            safe_zip_path(root, "csgo/maps/test.bsp").expect("relative entry should be safe"),
            root.join("csgo/maps/test.bsp")
        );
        assert!(safe_zip_path(root, "../../launcher.exe").is_err());
        assert!(safe_zip_path(root, r"C:\Windows\system.ini").is_err());
        assert!(safe_zip_path(root, "/Windows/system.ini").is_err());
    }
}
