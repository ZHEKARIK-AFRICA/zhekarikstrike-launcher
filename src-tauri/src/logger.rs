use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use chrono::Utc;
use tauri::{AppHandle, Emitter};

use crate::constants::APP_NAME;
use crate::error::{AppError, FrontendError};

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

pub fn init() -> Result<(), AppError> {
    let log_dir = log_dir()?;
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("launcher.log");
    let _ = LOG_PATH.set(log_path);

    std::panic::set_hook(Box::new(|panic_info| {
        let message = panic_info.to_string();
        error(&format!("panic: {message}"));
        emit_global_error("panic", &message);
    }));

    info("logger initialized");
    Ok(())
}

pub fn set_app_handle(app: AppHandle) {
    let _ = APP_HANDLE.set(app);
}

pub fn info(message: &str) {
    write("info", message);
}

pub fn warn(message: &str) {
    write("warn", message);
}

pub fn error(message: &str) {
    write("error", message);
}

pub fn emit_error(error: &AppError) {
    emit_global_error(error.code(), &error.to_string());
}

fn emit_global_error(code: &str, message: &str) {
    if let Some(app) = APP_HANDLE.get() {
        let _ = app.emit(
            "global-error",
            FrontendError {
                code: code.to_string(),
                message: message.to_string(),
                details: None,
            },
        );
    }
}

fn write(level: &str, message: &str) {
    let Some(path) = LOG_PATH.get() else {
        eprintln!("[{level}] {message}");
        return;
    };

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "[{}] [{level}] {message}", Utc::now().to_rfc3339());
    }
}

fn log_dir() -> Result<PathBuf, AppError> {
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        return Ok(PathBuf::from(local_app_data).join(APP_NAME).join("logs"));
    }

    if let Ok(user_profile) = env::var("USERPROFILE") {
        return Ok(PathBuf::from(user_profile)
            .join("AppData")
            .join("Local")
            .join(APP_NAME)
            .join("logs"));
    }

    Err(AppError::Config(
        "Unable to resolve log directory".to_string(),
    ))
}
