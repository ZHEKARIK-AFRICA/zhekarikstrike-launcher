use std::env;
use std::path::{Path, PathBuf};

use tauri::AppHandle;
use tokio::process::Command;

use crate::constants::PRODUCT_NAME;
use crate::error::AppError;
use crate::services::config_service;

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

    tokio::fs::create_dir_all(&game_path).await?;
    let script = env::temp_dir().join("zhekarik_launcher_move.cmd");
    let log = env::temp_dir().join("zhekarik_launcher_move.log");
    let commands = format!(
        "@echo off\r\nsetlocal\r\nset \"CURRENT={}\"\r\nset \"TARGET={}\"\r\nset \"LOG={}\"\r\ntimeout /t 2 /nobreak >NUL\r\necho moving launcher from \"%CURRENT%\" to \"%TARGET%\" >> \"%LOG%\"\r\nif not exist \"%CURRENT%\" exit /b 0\r\nif exist \"%TARGET%\" del /F /Q \"%TARGET%\" >> \"%LOG%\" 2>&1\r\nmove /Y \"%CURRENT%\" \"%TARGET%\" >> \"%LOG%\" 2>&1\r\nif errorlevel 1 exit /b 20\r\nstart \"\" \"%TARGET%\"\r\nexit /b 0\r\n",
        current.display(),
        target.display(),
        log.display()
    );

    tokio::fs::write(&script, commands).await?;
    let script_arg = script.to_string_lossy().to_string();
    Command::new("cmd")
        .args(["/C", &script_arg])
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
