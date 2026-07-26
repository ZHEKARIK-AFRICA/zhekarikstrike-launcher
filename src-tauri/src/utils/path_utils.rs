use std::path::{Component, Path, PathBuf};

use tauri::{path::BaseDirectory, AppHandle, Manager};

use crate::error::AppError;

pub fn normalize_manifest_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_string()
}

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

pub fn resource_path(app: &AppHandle, relative: &str) -> PathBuf {
    app.path()
        .resolve(relative, BaseDirectory::Resource)
        .unwrap_or_else(|_| {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|parent| parent.join(relative)))
                .filter(|path| path.exists())
                .unwrap_or_else(|| PathBuf::from("public").join(relative))
        })
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
