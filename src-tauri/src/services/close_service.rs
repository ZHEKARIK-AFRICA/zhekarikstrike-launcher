use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::AppError;
use crate::services::{game_process_service, shutdown_service};
use crate::state::{AppState, OperationKind};

const OPERATION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const OPERATION_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseRequirement {
    Immediate,
    GameRunning,
    OperationActive(OperationKind),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseConfirmationPayload {
    pub reason: &'static str,
    pub operation: OperationKind,
}

pub fn close_requirement(game_running: bool, operation: OperationKind) -> CloseRequirement {
    if game_running {
        CloseRequirement::GameRunning
    } else if operation == OperationKind::Idle {
        CloseRequirement::Immediate
    } else {
        CloseRequirement::OperationActive(operation)
    }
}

pub async fn request_close(app: AppHandle, state: &AppState) -> Result<(), AppError> {
    if state.shutdown_started() {
        return Ok(());
    }
    let game_state = game_process_service::sync_game_process(app.clone(), state).await?;
    let operation = state.current_state().operation;
    match close_requirement(game_state.pid.is_some(), operation) {
        CloseRequirement::Immediate => {
            if state.begin_shutdown() {
                shutdown_and_exit(app, state).await?;
            }
        }
        CloseRequirement::GameRunning => {
            request_confirmation(app, state, "game-running", operation)?;
        }
        CloseRequirement::OperationActive(operation) => {
            request_confirmation(app, state, "operation-active", operation)?;
        }
    }
    Ok(())
}

fn request_confirmation(
    app: AppHandle,
    state: &AppState,
    reason: &'static str,
    operation: OperationKind,
) -> Result<(), AppError> {
    let newly_pending = state.begin_close_confirmation();
    if newly_pending || state.close_confirmation_pending() {
        if let Err(error) = app.emit(
            "close-confirmation-requested",
            CloseConfirmationPayload { reason, operation },
        ) {
            if newly_pending {
                state.cancel_close_confirmation();
            }
            return Err(error.into());
        }
    }
    Ok(())
}

pub fn cancel_close(state: &AppState) -> bool {
    state.cancel_close_confirmation()
}

pub async fn confirm_close(app: AppHandle, state: &AppState) -> Result<(), AppError> {
    if !state.confirm_close() {
        return Err(AppError::InvalidData(
            "no close confirmation is pending".to_string(),
        ));
    }

    state.cancel_active_operation();
    if tokio::time::timeout(OPERATION_SHUTDOWN_TIMEOUT, wait_for_idle(state))
        .await
        .is_err()
    {
        crate::logger::warn(
            "active operation did not stop within 10 seconds; preserving recovery journal",
        );
    }

    stop_observed_game_after_confirmation(app.clone(), state).await;
    shutdown_and_exit(app, state).await
}

async fn wait_for_idle(state: &AppState) {
    while state.current_state().operation != OperationKind::Idle {
        tokio::time::sleep(OPERATION_POLL_INTERVAL).await;
    }
}

async fn stop_observed_game_after_confirmation(app: AppHandle, state: &AppState) {
    let Ok(game_state) = game_process_service::sync_game_process(app, state).await else {
        return;
    };
    if let Some(pid) = game_state.pid.filter(|_| !game_state.owned) {
        if let Err(error) = game_process_service::stop_game_process(pid).await {
            crate::logger::warn(&format!(
                "failed to stop observed game process {pid} after confirmation: {error}"
            ));
        }
    }
}

async fn shutdown_and_exit(app: AppHandle, state: &AppState) -> Result<(), AppError> {
    let result = shutdown_service::shutdown(app.clone(), state).await;
    app.exit(0);
    result
}

pub fn spawn_close_request(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let Some(state) = app.try_state::<AppState>() else {
            crate::logger::warn("window close requested without managed app state");
            return;
        };
        let state = state.inner().clone();
        if let Err(error) = request_close(app, &state).await {
            crate::logger::error(&format!("window close request failed: {error}"));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{close_requirement, CloseRequirement};
    use crate::state::OperationKind;

    #[test]
    fn release_1_6_12_idle_close_does_not_require_confirmation() {
        assert_eq!(
            close_requirement(false, OperationKind::Idle),
            CloseRequirement::Immediate
        );
    }

    #[test]
    fn release_1_6_12_game_and_operations_require_the_right_confirmation() {
        assert_eq!(
            close_requirement(true, OperationKind::Verifying),
            CloseRequirement::GameRunning
        );
        for operation in [
            OperationKind::Installing,
            OperationKind::InstallingPrerequisites,
            OperationKind::Verifying,
            OperationKind::UpdatingGame,
            OperationKind::LaunchingGame,
            OperationKind::UpdatingLauncher,
            OperationKind::RecoveringContent,
        ] {
            assert_eq!(
                close_requirement(false, operation),
                CloseRequirement::OperationActive(operation)
            );
        }
    }
}
