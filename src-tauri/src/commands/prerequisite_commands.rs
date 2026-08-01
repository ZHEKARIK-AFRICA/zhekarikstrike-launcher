use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::error::AppError;
use crate::models::validated_operation_id;
use crate::services::api_client::ApiClient;
use crate::services::config_service;
use crate::services::prerequisite_service::{
    legacy_manifest_refresh_required, refresh_legacy_manifest, EnsurePrerequisitesResult,
    PrerequisiteService, PrerequisiteServiceProgress, RestartStatus,
};
use crate::state::{
    AppState, CancellationSlot, OperationKind, PrerequisiteOperationState, PrerequisiteOutcome,
    PrerequisiteTerminalResult,
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrerequisiteResult {
    pub ready: bool,
    pub installed: Vec<String>,
    pub already_present: Vec<String>,
    pub restart_recommended: bool,
}

impl From<EnsurePrerequisitesResult> for PrerequisiteResult {
    fn from(value: EnsurePrerequisitesResult) -> Self {
        Self {
            ready: value.ready,
            installed: value.installed,
            already_present: value.already_present,
            restart_recommended: value.restart_status != RestartStatus::None,
        }
    }
}

impl From<&PrerequisiteResult> for PrerequisiteTerminalResult {
    fn from(value: &PrerequisiteResult) -> Self {
        Self {
            ready: value.ready,
            installed: value.installed.clone(),
            already_present: value.already_present.clone(),
            restart_recommended: value.restart_recommended,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrerequisiteProgressPayload {
    pub operation_id: String,
    pub stage: String,
    pub component_id: Option<String>,
    pub progress: Option<f64>,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub restart_recommended: bool,
}

impl PrerequisiteProgressPayload {
    fn from_service(operation_id: &str, progress: PrerequisiteServiceProgress) -> Self {
        Self {
            operation_id: operation_id.to_string(),
            stage: progress.stage.to_string(),
            component_id: progress.component_id,
            progress: progress.progress,
            downloaded_bytes: progress.downloaded_bytes,
            total_bytes: progress.total_bytes,
            restart_recommended: progress.restart_recommended,
        }
    }

    fn snapshot(&self, active: bool) -> PrerequisiteOperationState {
        PrerequisiteOperationState {
            active,
            operation_id: Some(self.operation_id.clone()),
            stage: Some(self.stage.clone()),
            component_id: self.component_id.clone(),
            progress: self.progress,
            downloaded_bytes: self.downloaded_bytes,
            total_bytes: self.total_bytes,
            restart_recommended: self.restart_recommended,
            outcome: PrerequisiteOutcome::Running,
            result: None,
            error: None,
        }
    }
}

#[tauri::command]
pub async fn ensure_game_prerequisites(
    app: AppHandle,
    state: State<'_, AppState>,
    operation_id: Option<String>,
) -> Result<PrerequisiteResult, AppError> {
    let operation_id = validated_operation_id(operation_id)?;
    ensure_game_prerequisites_inner(app, state.inner(), operation_id).await
}

#[tauri::command]
pub fn get_prerequisite_state(state: State<'_, AppState>) -> PrerequisiteOperationState {
    state.prerequisite_state()
}

#[tauri::command]
pub fn acknowledge_prerequisite_state(state: State<'_, AppState>, operation_id: String) -> bool {
    state.acknowledge_prerequisite_state(&operation_id)
}

pub(crate) async fn ensure_game_prerequisites_inner(
    app: AppHandle,
    state: &AppState,
    operation_id: String,
) -> Result<PrerequisiteResult, AppError> {
    let lease = state.begin_operation(
        OperationKind::InstallingPrerequisites,
        Some(CancellationSlot::Prerequisite),
    )?;
    let cancel = lease
        .cancellation_token()
        .expect("prerequisite operations always expose cancellation");
    state.begin_prerequisite_operation(&operation_id);

    let result: Result<PrerequisiteResult, AppError> = async {
        #[cfg(feature = "e2e")]
        {
            let _ = (app, cancel);
            Ok(PrerequisiteResult {
                ready: true,
                installed: Vec::new(),
                already_present: Vec::new(),
                restart_recommended: false,
            })
        }

        #[cfg(not(feature = "e2e"))]
        {
            let callback_app = app.clone();
            let callback_state = state.clone();
            let callback_operation_id = operation_id.clone();
            let callback = Arc::new(move |update: PrerequisiteServiceProgress| {
                let payload =
                    PrerequisiteProgressPayload::from_service(&callback_operation_id, update);
                callback_state.update_prerequisite_state(payload.snapshot(true));
                if let Err(error) = callback_app.emit("prerequisite-progress", payload) {
                    crate::logger::warn(&format!("failed to emit prerequisite progress: {error}"));
                }
            });
            let game_path = config_service::get_game_path()
                .await?
                .ok_or(AppError::GamePathNotSet)?;
            let installed_version = config_service::get_game_version().await?;
            if legacy_manifest_refresh_required(&game_path, &installed_version).await? {
                let client = ApiClient::new().map_err(normalize_manifest_acquisition_error)?;
                let manifest = client
                    .get_full_manifest()
                    .await
                    .map_err(normalize_manifest_acquisition_error)?;
                refresh_legacy_manifest(&game_path, &installed_version, &manifest).await?;
            }
            let service = PrerequisiteService::windows_with_progress(callback)?;
            Ok(service
                .ensure_active_for_version(&game_path, &installed_version, &cancel)
                .await?
                .into())
        }
    }
    .await;

    let result = finalize_prerequisite_result(result);

    match &result {
        Ok(value) => state.finish_prerequisite_success(&operation_id, value.into()),
        Err(AppError::Canceled) => {
            state.finish_prerequisite_canceled(&operation_id, AppError::Canceled.frontend_error())
        }
        Err(error) => state.finish_prerequisite_failure(&operation_id, error.frontend_error()),
    }
    drop(lease);
    result
}

fn finalize_prerequisite_result(
    result: Result<PrerequisiteResult, AppError>,
) -> Result<PrerequisiteResult, AppError> {
    result.and_then(guard_prerequisite_result)
}

fn normalize_manifest_acquisition_error(error: AppError) -> AppError {
    match error {
        AppError::Network(message) => AppError::PrerequisiteDownload(message),
        AppError::InvalidData(message) => AppError::PrerequisiteVerification(message),
        error @ (AppError::PrerequisiteDownload(_) | AppError::PrerequisiteVerification(_)) => {
            error
        }
        error => AppError::PrerequisiteVerification(error.to_string()),
    }
}

pub(crate) async fn check_game_prerequisites_fast_at(
    game_path: &std::path::Path,
) -> Result<PrerequisiteResult, AppError> {
    #[cfg(feature = "e2e")]
    return Ok(PrerequisiteResult {
        ready: true,
        installed: Vec::new(),
        already_present: Vec::new(),
        restart_recommended: false,
    });

    #[cfg(not(feature = "e2e"))]
    {
        let installed_version = config_service::get_game_version().await?;
        let service = PrerequisiteService::windows()?;
        Ok(service
            .check_active_for_version(
                game_path,
                &installed_version,
                &tokio_util::sync::CancellationToken::new(),
            )
            .await?
            .into())
    }
}

pub(crate) fn guard_prerequisite_result(
    result: PrerequisiteResult,
) -> Result<PrerequisiteResult, AppError> {
    if result.ready {
        return Ok(result);
    }
    if result.restart_recommended {
        return Err(AppError::PrerequisiteRestartRequired(
            "restart Windows to make the installed runtime components available".into(),
        ));
    }
    Err(AppError::PrerequisiteInstall(
        "required runtime components are still unavailable after installation".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        finalize_prerequisite_result, guard_prerequisite_result,
        normalize_manifest_acquisition_error, PrerequisiteProgressPayload, PrerequisiteResult,
    };
    use crate::error::AppError;
    use crate::services::prerequisite_service::{EnsurePrerequisitesResult, RestartStatus};
    use crate::state::{AppState, PrerequisiteOutcome};

    #[test]
    fn release_1_6_13_public_result_uses_the_frontend_contract() {
        let result = PrerequisiteResult::from(EnsurePrerequisitesResult {
            ready: true,
            installed: vec!["vc2010-sp1-x86".into()],
            already_present: vec!["directx-june-2010".into()],
            restart_status: RestartStatus::Recommended,
        });
        let value = serde_json::to_value(result).expect("result should serialize");

        assert_eq!(value["ready"], true);
        assert_eq!(value["installed"][0], "vc2010-sp1-x86");
        assert_eq!(value["alreadyPresent"][0], "directx-june-2010");
        assert_eq!(value["restartRecommended"], true);
        assert!(value.get("restartStatus").is_none());
    }

    #[test]
    fn release_1_6_13_progress_contains_reload_and_byte_fields() {
        let payload = PrerequisiteProgressPayload {
            operation_id: "op".into(),
            stage: "downloading".into(),
            component_id: Some("vc2010-sp1-x86".into()),
            progress: Some(25.0),
            downloaded_bytes: Some(250),
            total_bytes: Some(1000),
            restart_recommended: false,
        };
        let value = serde_json::to_value(payload).expect("progress should serialize");

        assert_eq!(value["operationId"], "op");
        assert_eq!(value["componentId"], "vc2010-sp1-x86");
        assert_eq!(value["downloadedBytes"], 250);
        assert_eq!(value["totalBytes"], 1000);
        assert_eq!(value["restartRecommended"], false);
    }

    #[test]
    fn release_1_6_13_internal_launch_guard_rejects_a_pending_restart() {
        let error = guard_prerequisite_result(PrerequisiteResult {
            ready: false,
            installed: vec!["vc2010-sp1-x86".into()],
            already_present: Vec::new(),
            restart_recommended: true,
        })
        .expect_err("launch must not bypass the final prerequisite check");

        assert!(matches!(error, AppError::PrerequisiteRestartRequired(_)));
    }

    #[test]
    fn not_ready_restart_is_recorded_as_a_structured_failed_terminal() {
        let state = AppState::new();
        state.begin_prerequisite_operation("restart");
        let result = finalize_prerequisite_result(Ok(PrerequisiteResult {
            ready: false,
            installed: vec!["vc2010-sp1-x86".into()],
            already_present: Vec::new(),
            restart_recommended: true,
        }));
        let error = result.expect_err("restart-required result must not become success");
        state.finish_prerequisite_failure("restart", error.frontend_error());

        let terminal = state.prerequisite_state();
        assert_eq!(terminal.outcome, PrerequisiteOutcome::Failed);
        assert_eq!(
            terminal.error.as_ref().map(|error| error.code.as_str()),
            Some("prerequisite_restart_required")
        );
    }

    #[test]
    fn not_ready_without_restart_is_a_structured_install_failure() {
        let error = finalize_prerequisite_result(Ok(PrerequisiteResult {
            ready: false,
            installed: Vec::new(),
            already_present: Vec::new(),
            restart_recommended: false,
        }))
        .expect_err("unavailable components must not become success");

        assert_eq!(error.code(), "prerequisite_install_failed");
    }

    #[test]
    fn full_manifest_acquisition_errors_use_prerequisite_codes() {
        assert_eq!(
            normalize_manifest_acquisition_error(AppError::Network("offline".into())).code(),
            "prerequisite_download_failed"
        );
        assert_eq!(
            normalize_manifest_acquisition_error(AppError::InvalidData("incomplete".into())).code(),
            "prerequisite_verification_failed"
        );
    }
}
