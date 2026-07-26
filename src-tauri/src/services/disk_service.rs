use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::AppError;

#[derive(Debug, Clone, Serialize)]
pub struct DiskSpaceStatus {
    #[serde(rename = "freeBytes")]
    pub free_bytes: u64,
    #[serde(rename = "requiredBytes")]
    pub required_bytes: u64,
    pub enough: bool,
}

pub async fn check_disk_space(
    target_path: &Path,
    required_bytes: u64,
) -> Result<DiskSpaceStatus, AppError> {
    check_disk_space_sync(target_path, required_bytes)
}

pub fn ensure_disk_space(
    target_path: &Path,
    required_bytes: u64,
) -> Result<DiskSpaceStatus, AppError> {
    let status = check_disk_space_sync(target_path, required_bytes)?;
    if !status.enough {
        return Err(AppError::InsufficientDiskSpace {
            required: status.required_bytes,
            available: status.free_bytes,
        });
    }

    Ok(status)
}

#[cfg(target_os = "windows")]
fn check_disk_space_sync(
    target_path: &Path,
    required_bytes: u64,
) -> Result<DiskSpaceStatus, AppError> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let query_path = nearest_existing_path(target_path);
    let mut free_bytes = 0_u64;
    let wide = wide_path(&query_path);

    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free_bytes), None, None) }
        .map_err(|error| {
            AppError::FileSystem(format!(
                "failed to query free disk space for {}: {error}",
                query_path.display()
            ))
        })?;

    Ok(DiskSpaceStatus {
        free_bytes,
        required_bytes,
        enough: free_bytes >= required_bytes,
    })
}

#[cfg(not(target_os = "windows"))]
fn check_disk_space_sync(
    _target_path: &Path,
    required_bytes: u64,
) -> Result<DiskSpaceStatus, AppError> {
    Ok(DiskSpaceStatus {
        free_bytes: required_bytes,
        required_bytes,
        enough: true,
    })
}

fn nearest_existing_path(path: &Path) -> PathBuf {
    let mut current = if path.exists() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(path).to_path_buf()
    };

    while !current.exists() {
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }

    current
}

#[cfg(target_os = "windows")]
fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
