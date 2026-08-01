use std::env;
use std::path::{Path, PathBuf};

use tauri::AppHandle;
use tokio::process::Command;

use crate::constants::PRODUCT_NAME;
use crate::error::AppError;
use crate::services::{config_service, shortcut_service};

pub fn legacy_move_enabled() -> bool {
    cfg!(feature = "portable") && !cfg!(debug_assertions)
}

pub async fn shortcut_target_path() -> Result<PathBuf, AppError> {
    if legacy_move_enabled() {
        if let Some(game_path) = config_service::get_game_path().await? {
            return Ok(game_path.join(format!("{PRODUCT_NAME}.exe")));
        }
    }

    Ok(env::current_exe()?)
}

pub async fn move_launcher_to_game_path(app: AppHandle) -> Result<(), AppError> {
    schedule_legacy_move_if_needed(app.clone()).await?;
    app.exit(0);
    Ok(())
}

fn move_script_commands(current: &Path, target: &Path, log: &Path) -> String {
    format!(
        "@echo off\r\nsetlocal\r\nset \"CURRENT={}\"\r\nset \"TARGET={}\"\r\nset \"LOG={}\"\r\ntimeout /t 2 /nobreak >NUL\r\necho moving launcher from \"%CURRENT%\" to \"%TARGET%\" >> \"%LOG%\"\r\nif not exist \"%CURRENT%\" goto cleanup\r\nif exist \"%TARGET%\" del /F /Q \"%TARGET%\" >> \"%LOG%\" 2>&1\r\nmove /Y \"%CURRENT%\" \"%TARGET%\" >> \"%LOG%\" 2>&1\r\nif errorlevel 1 exit /b 20\r\n:cleanup\r\ndel /F /Q \"%~f0\" >NUL 2>&1\r\nexit /b 0\r\n",
        current.display(),
        target.display(),
        log.display()
    )
}

pub async fn schedule_legacy_move_if_needed(_app: AppHandle) -> Result<(), AppError> {
    if !legacy_move_enabled() {
        return Ok(());
    }

    let Some(game_path) = config_service::get_game_path().await? else {
        return Ok(());
    };

    let current = env::current_exe()?;
    let target = game_path.join(format!("{PRODUCT_NAME}.exe"));
    if same_path(&current, &target) {
        return Ok(());
    }

    if let Err(error) =
        shortcut_service::repair_existing_default_shortcuts_for_target(target.clone()).await
    {
        crate::logger::warn(&format!(
            "portable move will continue after shortcut repair failed: {error}"
        ));
    }

    tokio::fs::create_dir_all(&game_path).await?;
    let script = env::temp_dir().join("zhekarik_launcher_move.cmd");
    let log = env::temp_dir().join("zhekarik_launcher_move.log");
    let commands = move_script_commands(&current, &target, &log);

    tokio::fs::write(&script, commands).await?;
    let script_arg = script.to_string_lossy().to_string();
    let mut command = Command::new("cmd.exe");
    command
        .args(["/D", "/S", "/C", &script_arg])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.as_std_mut().creation_flags(0x0800_0000);
    }
    command
        .spawn()
        .map_err(|error| AppError::Unknown(error.to_string()))?;
    Ok(())
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::move_script_commands;

    #[test]
    fn release_1_6_13_portable_move_does_not_restart_the_launcher() {
        let script = move_script_commands(
            Path::new(r"C:\Downloads\launcher.exe"),
            Path::new(r"D:\Games\ZHEKARIKSTRIKE\ZHEKARIK STRIKE.exe"),
            Path::new(r"C:\Temp\launcher-move.log"),
        );

        assert!(script.contains("move /Y \"%CURRENT%\" \"%TARGET%\""));
        assert!(!script
            .lines()
            .any(|line| line.trim_start().to_ascii_lowercase().starts_with("start ")));
        assert!(script.contains("del /F /Q \"%~f0\""));
    }
}
