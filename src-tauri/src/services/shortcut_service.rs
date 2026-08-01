use std::env;
use std::path::{Path, PathBuf};

use crate::constants::PRODUCT_NAME;
use crate::error::AppError;
use crate::services::launcher_move_service;

pub async fn create_default_shortcuts() -> Result<(), AppError> {
    if cfg!(debug_assertions) {
        return Ok(());
    }

    let target_path = launcher_move_service::shortcut_target_path().await?;
    create_default_shortcuts_for_target(target_path).await
}

pub async fn create_default_shortcuts_for_target(target_path: PathBuf) -> Result<(), AppError> {
    create_desktop_shortcut(target_path.clone(), default_shortcut_name()).await?;
    create_start_menu_shortcut(target_path, default_shortcut_name()).await?;
    Ok(())
}

pub async fn repair_existing_default_shortcuts_for_target(
    target_path: PathBuf,
) -> Result<(), AppError> {
    if cfg!(debug_assertions) {
        return Ok(());
    }

    let shortcut_name = default_shortcut_name();
    repair_existing_shortcut(&target_path, &desktop_shortcut_path(&shortcut_name)?).await?;
    repair_existing_shortcut(&target_path, &start_menu_shortcut_path(&shortcut_name)?).await
}

pub async fn create_desktop_shortcut(
    target_path: PathBuf,
    shortcut_name: String,
) -> Result<(), AppError> {
    if cfg!(debug_assertions) {
        return Ok(());
    }

    create_shortcut(&target_path, &desktop_shortcut_path(&shortcut_name)?)
}

pub async fn create_start_menu_shortcut(
    target_path: PathBuf,
    shortcut_name: String,
) -> Result<(), AppError> {
    if cfg!(debug_assertions) {
        return Ok(());
    }

    create_shortcut(&target_path, &start_menu_shortcut_path(&shortcut_name)?)
}

fn desktop_shortcut_path(shortcut_name: &str) -> Result<PathBuf, AppError> {
    Ok(env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::FileSystem("USERPROFILE is not set".to_string()))?
        .join("Desktop")
        .join(format!("{shortcut_name}.lnk")))
}

fn start_menu_shortcut_path(shortcut_name: &str) -> Result<PathBuf, AppError> {
    Ok(env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::FileSystem("APPDATA is not set".to_string()))?
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join(format!("{shortcut_name}.lnk")))
}

async fn repair_existing_shortcut(
    target_path: &Path,
    shortcut_path: &Path,
) -> Result<(), AppError> {
    if !tokio::fs::try_exists(shortcut_path).await? {
        return Ok(());
    }
    create_shortcut(target_path, shortcut_path)
}

#[cfg(target_os = "windows")]
fn create_shortcut(target_path: &Path, shortcut_path: &Path) -> Result<(), AppError> {
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    if let Some(parent) = shortcut_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let init_result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    init_result
        .ok()
        .map_err(|error| AppError::Unknown(format!("COM initialization failed: {error}")))?;

    let result = (|| {
        let shell_link: IShellLinkW =
            unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.map_err(
                |error| AppError::Unknown(format!("ShellLink creation failed: {error}")),
            )?;

        let target = wide_path(target_path);
        unsafe {
            shell_link
                .SetPath(PCWSTR(target.as_ptr()))
                .map_err(|error| AppError::Unknown(format!("shortcut target failed: {error}")))?;
            shell_link
                .SetIconLocation(PCWSTR(target.as_ptr()), 0)
                .map_err(|error| AppError::Unknown(format!("shortcut icon failed: {error}")))?;
        }

        let persist: IPersistFile = shell_link
            .cast()
            .map_err(|error| AppError::Unknown(format!("IPersistFile cast failed: {error}")))?;
        let shortcut = wide_path(shortcut_path);
        unsafe {
            persist
                .Save(PCWSTR(shortcut.as_ptr()), true)
                .map_err(|error| AppError::Unknown(format!("shortcut save failed: {error}")))?;
        }

        Ok(())
    })();

    unsafe {
        CoUninitialize();
    }

    result
}

#[cfg(not(target_os = "windows"))]
fn create_shortcut(_target_path: &Path, _shortcut_path: &Path) -> Result<(), AppError> {
    Ok(())
}

fn default_shortcut_name() -> String {
    PRODUCT_NAME.to_string()
}

#[cfg(target_os = "windows")]
fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod release_1_6_13_tests {
    use tempfile::tempdir;

    use super::repair_existing_shortcut;

    #[tokio::test]
    async fn deleted_shortcut_is_not_recreated_during_portable_move() {
        let directory = tempdir().expect("temporary directory should exist");
        let shortcut = directory.path().join("deleted-by-user.lnk");

        repair_existing_shortcut(&directory.path().join("launcher.exe"), &shortcut)
            .await
            .expect("missing shortcut should be ignored");

        assert!(!shortcut.exists());
    }
}
