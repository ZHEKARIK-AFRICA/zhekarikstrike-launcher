use serde::ser::{SerializeStruct, Serializer};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Serialize)]
pub struct FrontendError {
    pub code: String,
    pub message: String,
    pub details: Option<String>,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Config error: {0}")]
    Config(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("File system error: {0}")]
    FileSystem(String),

    #[error("Insufficient disk space. Required: {required}, available: {available}")]
    InsufficientDiskSpace { required: u64, available: u64 },

    #[error("Game path is not set")]
    GamePathNotSet,

    #[error("Game executable not found: {0}")]
    GameExecutableNotFound(String),

    #[error("Operation already in progress: {0}")]
    OperationInProgress(String),

    #[error("Operation canceled")]
    Canceled,

    #[error("Administrator privileges are required")]
    AdminRequired,

    #[error("Invalid data: {0}")]
    InvalidData(String),

    #[error("Unknown error: {0}")]
    Unknown(String),
}

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            AppError::Config(_) => "config",
            AppError::Network(_) => "network",
            AppError::FileSystem(_) => "file-system",
            AppError::InsufficientDiskSpace { .. } => "insufficient-disk-space",
            AppError::GamePathNotSet => "game-path-not-set",
            AppError::GameExecutableNotFound(_) => "game-executable-not-found",
            AppError::OperationInProgress(_) => "operation-in-progress",
            AppError::Canceled => "canceled",
            AppError::AdminRequired => "admin-required",
            AppError::InvalidData(_) => "invalid-data",
            AppError::Unknown(_) => "unknown",
        }
    }

    pub fn frontend_error(&self) -> FrontendError {
        FrontendError {
            code: self.code().to_string(),
            message: self.to_string(),
            details: None,
        }
    }
}

impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let frontend = self.frontend_error();
        let mut state = serializer.serialize_struct("FrontendError", 3)?;
        state.serialize_field("code", &frontend.code)?;
        state.serialize_field("message", &frontend.message)?;
        state.serialize_field("details", &frontend.details)?;
        state.end()
    }
}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        if value.kind() == std::io::ErrorKind::OutOfMemory {
            return AppError::FileSystem("out of memory".to_string());
        }

        AppError::FileSystem(value.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(value: reqwest::Error) -> Self {
        AppError::Network(value.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(value: serde_json::Error) -> Self {
        AppError::InvalidData(value.to_string())
    }
}

impl From<zip::result::ZipError> for AppError {
    fn from(value: zip::result::ZipError) -> Self {
        AppError::FileSystem(value.to_string())
    }
}

impl From<tauri::Error> for AppError {
    fn from(value: tauri::Error) -> Self {
        AppError::Unknown(value.to_string())
    }
}
