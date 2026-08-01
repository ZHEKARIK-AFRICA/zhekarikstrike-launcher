use std::path::{Component, Path, PathBuf};

use crate::error::AppError;

pub fn safe_join(base: &Path, relative: &str) -> Result<PathBuf, AppError> {
    let relative_path = Path::new(relative);
    let mut safe = PathBuf::new();

    for component in relative_path.components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            _ => {
                return Err(AppError::InvalidData(format!(
                    "Unsafe relative path: {relative}"
                )))
            }
        }
    }

    Ok(base.join(safe))
}

pub async fn ensure_safe_descendant(root: &Path, path: &Path) -> Result<(), AppError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        AppError::InvalidData(format!(
            "content path is outside the game root: {}",
            path.display()
        ))
    })?;
    let mut current = root.to_path_buf();
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        if !matches!(component, Component::Normal(_)) {
            return Err(AppError::InvalidData(format!(
                "unsafe content descendant: {}",
                path.display()
            )));
        }
        current.push(component.as_os_str());
        match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) => {
                if metadata_is_reparse_point(&metadata) {
                    return Err(AppError::InvalidData(format!(
                        "content path crosses a reparse point: {}",
                        current.display()
                    )));
                }
                if index + 1 < components.len() && !metadata.is_dir() {
                    return Err(AppError::InvalidData(format!(
                        "content path ancestor is not a directory: {}",
                        current.display()
                    )));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn metadata_is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(target_os = "windows"))]
fn metadata_is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::safe_join;

    #[test]
    fn safe_join_accepts_only_relative_manifest_paths() {
        let root = Path::new(r"D:\Games\ZS");
        assert_eq!(
            safe_join(root, "csgo/scripts/items.txt").expect("relative path should be safe"),
            root.join("csgo/scripts/items.txt")
        );
        assert!(safe_join(root, "../outside.exe").is_err());
        assert!(safe_join(root, r"C:\outside.exe").is_err());
        assert!(safe_join(root, "/outside.exe").is_err());
    }
}
