use std::env;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

use minisign_verify::{PublicKey, Signature};
use semver::Version;
use tauri::{AppHandle, Emitter};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::constants::LAUNCHER_UPDATE_PUBLIC_KEY;
use crate::error::AppError;
use crate::models::{LauncherUpdateStatus, ProgressEmitter, ProgressStage};
use crate::services::api_client::ApiClient;
use crate::services::download_service::download_file;
use crate::utils::hash_utils::sha256_file;

pub async fn check_launcher_update(
    current_version: &str,
) -> Result<LauncherUpdateStatus, AppError> {
    let api = ApiClient::new()?;
    let manifest = api.get_launcher_update(current_version).await?;
    let has_update = is_newer(&manifest.version, current_version);
    let platform = manifest.platform("windows-x86_64");
    let download_size = platform
        .as_ref()
        .map(|platform| platform.size)
        .or(manifest.size);
    let blocked_reason = if has_update {
        update_blocked_reason(platform.as_ref())
    } else {
        None
    };

    Ok(LauncherUpdateStatus {
        has_update,
        can_apply: has_update && blocked_reason.is_none(),
        blocked_reason,
        current_version: current_version.to_string(),
        latest_version: manifest.version,
        download_size,
    })
}

pub async fn download_launcher_update(app: AppHandle) -> Result<PathBuf, AppError> {
    let api = ApiClient::new()?;
    let current_version = env!("CARGO_PKG_VERSION");
    let manifest = api.get_launcher_update(current_version).await?;
    let platform = manifest.platform("windows-x86_64").ok_or_else(|| {
        AppError::InvalidData("launcher update manifest is missing windows-x86_64 data".to_string())
    })?;

    ensure_update_public_key_ready()?;

    let target = env::temp_dir().join("ZHEKARIK STRIKE.new.exe");
    let progress =
        ProgressEmitter::new(app, "launcher-update-progress", Uuid::new_v4().to_string());
    download_file(
        api.http(),
        &platform.url,
        &target,
        Some(progress.clone()),
        CancellationToken::new(),
        None,
        Some(&platform.sha256),
    )
    .await?;

    let actual = sha256_file(&target).await?;
    if !actual.eq_ignore_ascii_case(&platform.sha256) {
        return Err(AppError::InvalidData(
            "launcher update hash mismatch".to_string(),
        ));
    }

    verify_minisign_signature(&target, &platform.signature)?;

    progress.emit_stage(ProgressStage::Complete, Some(100.0), None)?;
    Ok(target)
}

pub async fn apply_launcher_update(app: AppHandle) -> Result<(), AppError> {
    let current = env::current_exe()?;
    let new_launcher = env::temp_dir().join("ZHEKARIK STRIKE.new.exe");

    if !tokio::fs::try_exists(&new_launcher).await.unwrap_or(false) {
        return Err(AppError::FileSystem(format!(
            "Downloaded launcher not found: {}",
            new_launcher.display()
        )));
    }

    let old = current.with_extension("old.exe");
    let script = env::temp_dir().join("zhekarik_launcher_update.cmd");
    let commands = format!(
        "@echo off\r\nsetlocal\r\nset \"CURRENT={}\"\r\nset \"NEW={}\"\r\nset \"OLD={}\"\r\ntimeout /t 2 /nobreak >NUL\r\nif not exist \"%NEW%\" exit /b 10\r\nif exist \"%OLD%\" del /F /Q \"%OLD%\" >NUL 2>NUL\r\nmove /Y \"%CURRENT%\" \"%OLD%\" >NUL\r\nif errorlevel 1 exit /b 20\r\nmove /Y \"%NEW%\" \"%CURRENT%\" >NUL\r\nif errorlevel 1 (\r\n  if exist \"%OLD%\" move /Y \"%OLD%\" \"%CURRENT%\" >NUL\r\n  exit /b 30\r\n)\r\nstart \"\" \"%CURRENT%\"\r\nif errorlevel 1 (\r\n  del /F /Q \"%CURRENT%\" >NUL 2>NUL\r\n  if exist \"%OLD%\" move /Y \"%OLD%\" \"%CURRENT%\" >NUL\r\n  start \"\" \"%CURRENT%\"\r\n  exit /b 40\r\n)\r\nexit /b 0\r\n",
        current.display(),
        new_launcher.display(),
        old.display()
    );

    tokio::fs::write(&script, commands).await?;
    let script_arg = script.to_string_lossy().to_string();
    Command::new("cmd")
        .args(["/C", &script_arg])
        .spawn()
        .map_err(|error| AppError::Unknown(error.to_string()))?;

    app.emit("launcher-update-applied", ())?;
    app.exit(0);
    Ok(())
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (Version::parse(latest), Version::parse(current)) {
        (Ok(latest), Ok(current)) => latest > current,
        _ => latest != current && !latest.is_empty(),
    }
}

fn ensure_update_public_key_ready() -> Result<(), AppError> {
    if public_key_is_placeholder() {
        return Err(AppError::InvalidData(
            "launcher update public key is not configured".to_string(),
        ));
    }

    Ok(())
}

fn update_blocked_reason(
    platform: Option<&crate::models::LauncherUpdateManifestPlatform>,
) -> Option<String> {
    let Some(platform) = platform else {
        return Some("launcher update manifest is missing windows-x86_64 data".to_string());
    };

    if platform.url.trim().is_empty() {
        return Some("launcher update url is missing".to_string());
    }
    if platform.sha256.trim().is_empty() {
        return Some("launcher update sha256 is missing".to_string());
    }
    if platform.signature.trim().is_empty() {
        return Some("launcher update signature is missing".to_string());
    }
    if public_key_is_placeholder() {
        return Some("launcher update public key is not configured".to_string());
    }

    None
}

fn public_key_is_placeholder() -> bool {
    LAUNCHER_UPDATE_PUBLIC_KEY.trim().is_empty()
        || LAUNCHER_UPDATE_PUBLIC_KEY.contains("REPLACE_WITH")
}

fn verify_minisign_signature(path: &Path, signature_text: &str) -> Result<(), AppError> {
    let public_key = PublicKey::from_base64(LAUNCHER_UPDATE_PUBLIC_KEY).map_err(|error| {
        AppError::InvalidData(format!("invalid launcher update public key: {error}"))
    })?;
    let signature = Signature::decode(signature_text).map_err(|error| {
        AppError::InvalidData(format!("invalid launcher update signature: {error}"))
    })?;
    let mut verifier = public_key.verify_stream(&signature).map_err(|error| {
        AppError::InvalidData(format!("launcher update signature setup failed: {error}"))
    })?;

    let mut file = std::fs::File::open(path)?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        verifier.update(&buffer[..read]);
    }

    verifier.finalize().map_err(|error| {
        AppError::InvalidData(format!("launcher update signature mismatch: {error}"))
    })?;
    Ok(())
}
