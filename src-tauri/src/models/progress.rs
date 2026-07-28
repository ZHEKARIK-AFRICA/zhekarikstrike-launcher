use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::error::AppError;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProgressStage {
    Checking,
    Install,
    Download,
    Extract,
    Verify,
    Update,
    Complete,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProgressPayload {
    #[serde(rename = "operationId")]
    pub operation_id: String,
    pub stage: ProgressStage,
    pub progress: Option<f64>,
    #[serde(rename = "currentFile")]
    pub current_file: Option<String>,
    #[serde(rename = "downloadedBytes")]
    pub downloaded_bytes: Option<u64>,
    #[serde(rename = "totalBytes")]
    pub total_bytes: Option<u64>,
    #[serde(rename = "speedBytesPerSec")]
    pub speed_bytes_per_sec: Option<f64>,
    #[serde(rename = "timeRemainingSec")]
    pub time_remaining_sec: Option<f64>,
    pub message: Option<String>,
}

impl ProgressPayload {
    pub fn new(operation_id: impl Into<String>, stage: ProgressStage) -> Self {
        Self {
            operation_id: operation_id.into(),
            stage,
            progress: None,
            current_file: None,
            downloaded_bytes: None,
            total_bytes: None,
            speed_bytes_per_sec: None,
            time_remaining_sec: None,
            message: None,
        }
    }
}

#[derive(Clone)]
pub struct ProgressEmitter {
    app: AppHandle,
    event: String,
    operation_id: String,
}

impl ProgressEmitter {
    pub fn new(app: AppHandle, event: impl Into<String>, operation_id: impl Into<String>) -> Self {
        Self {
            app,
            event: event.into(),
            operation_id: operation_id.into(),
        }
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub fn emit(&self, payload: ProgressPayload) -> Result<(), AppError> {
        self.app.emit(&self.event, payload)?;
        Ok(())
    }

    pub fn emit_stage(
        &self,
        stage: ProgressStage,
        progress: Option<f64>,
        message: Option<String>,
    ) -> Result<(), AppError> {
        let mut payload = ProgressPayload::new(self.operation_id.clone(), stage);
        payload.progress = progress;
        payload.message = message;
        self.emit(payload)
    }
}
