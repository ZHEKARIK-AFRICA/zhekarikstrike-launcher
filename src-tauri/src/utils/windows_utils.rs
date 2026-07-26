use std::process::Command;

use crate::error::AppError;

pub fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

pub fn command_status_error(command: &Command) -> AppError {
    AppError::Unknown(format!("command failed: {command:?}"))
}
