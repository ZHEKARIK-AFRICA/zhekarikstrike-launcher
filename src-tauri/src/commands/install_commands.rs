use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::error::AppError;
use crate::models::{validated_operation_id, ProgressEmitter, ProgressStage};
#[cfg(not(feature = "e2e"))]
use crate::services::shortcut_service;
use crate::services::{config_service, content_journal_service, install_service};
use crate::state::{AppState, CancellationSlot, OperationKind};

#[tauri::command]
pub async fn install_game(
    app: AppHandle,
    state: State<'_, AppState>,
    game_path: String,
    operation_id: Option<String>,
) -> Result<(), AppError> {
    let operation_id = validated_operation_id(operation_id)?;
    let lease =
        state.begin_operation(OperationKind::Installing, Some(CancellationSlot::Install))?;
    let cancel = lease
        .cancellation_token()
        .expect("install operations always have a cancellation token");

    #[cfg(feature = "e2e")]
    let content_result = {
        if game_path.to_ascii_lowercase().contains("error") {
            Err(AppError::Network(
                "native install fixture failed".to_string(),
            ))
        } else if game_path.to_ascii_lowercase().contains("cancel") {
            cancel.cancelled().await;
            Err(AppError::Canceled)
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            Ok(())
        }
    };

    #[cfg(not(feature = "e2e"))]
    let content_result = install_service::install_game(
        app.clone(),
        PathBuf::from(game_path),
        cancel,
        operation_id.clone(),
    )
    .await;

    drop(lease);
    complete_install_flow(
        content_result,
        || {
            super::prerequisite_commands::ensure_game_prerequisites_inner(
                app,
                state.inner(),
                operation_id,
            )
        },
        create_post_install_shortcuts,
    )
    .await
}

async fn create_post_install_shortcuts() -> Result<(), AppError> {
    #[cfg(feature = "e2e")]
    return Ok(());

    #[cfg(not(feature = "e2e"))]
    shortcut_service::create_default_shortcuts().await
}

async fn complete_install_flow<F, Fut, P, PostFut>(
    content_result: Result<(), AppError>,
    ensure: F,
    post_install: P,
) -> Result<(), AppError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<
        Output = Result<super::prerequisite_commands::PrerequisiteResult, AppError>,
    >,
    P: FnOnce() -> PostFut,
    PostFut: std::future::Future<Output = Result<(), AppError>>,
{
    content_result?;
    let prerequisites = ensure().await?;
    super::prerequisite_commands::guard_prerequisite_result(prerequisites)?;
    if let Err(error) = post_install().await {
        crate::logger::warn(&format!(
            "installed content and prerequisites but could not create shortcuts: {error}"
        ));
    }
    Ok(())
}

#[tauri::command]
pub async fn cancel_install(state: State<'_, AppState>) -> Result<bool, AppError> {
    Ok(match state.current_state().operation {
        OperationKind::InstallingPrerequisites => state.cancel_prerequisites(),
        _ => state.cancel_install(),
    })
}

#[derive(Debug, Serialize)]
pub struct PendingInstallRecovery {
    pub recovered: bool,
}

#[tauri::command]
pub async fn recover_pending_install(
    app: AppHandle,
    state: State<'_, AppState>,
    operation_id: Option<String>,
) -> Result<PendingInstallRecovery, AppError> {
    let operation_id = validated_operation_id(operation_id)?;
    #[cfg(feature = "e2e")]
    {
        let _ = (app, state, operation_id);
        return Ok(PendingInstallRecovery { recovered: false });
    }

    #[cfg(not(feature = "e2e"))]
    {
        let Some(game_path) = config_service::get_game_path().await? else {
            return Ok(PendingInstallRecovery { recovered: false });
        };
        if !tokio::fs::try_exists(content_journal_service::journal_path(&game_path)).await? {
            return Ok(PendingInstallRecovery { recovered: false });
        }

        let _lease = state.begin_operation(OperationKind::RecoveringContent, None)?;
        let progress = ProgressEmitter::new(app, "recovery-progress", operation_id);
        progress.emit_stage(
            ProgressStage::Cleanup,
            Some(0.0),
            Some("recovering interrupted content transaction".to_string()),
        )?;
        let recovered = content_journal_service::recover_pending_content(&game_path).await?;
        progress.emit_stage(ProgressStage::Cleanup, Some(100.0), None)?;
        Ok(PendingInstallRecovery { recovered })
    }
}

#[cfg(test)]
mod prerequisite_flow_tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::complete_install_flow;
    use crate::commands::prerequisite_commands::PrerequisiteResult;
    use crate::error::AppError;

    fn ready() -> PrerequisiteResult {
        PrerequisiteResult {
            ready: true,
            installed: Vec::new(),
            already_present: Vec::new(),
            restart_recommended: false,
        }
    }

    #[tokio::test]
    async fn release_1_6_13_prerequisites_run_after_content_commit() {
        let called = AtomicBool::new(false);

        complete_install_flow(
            Ok(()),
            || async {
                called.store(true, Ordering::SeqCst);
                Ok(ready())
            },
            || async { Ok(()) },
        )
        .await
        .expect("ready prerequisites should complete installation");

        assert!(called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn release_1_6_13_content_failure_never_starts_prerequisites() {
        let called = AtomicBool::new(false);
        let error = complete_install_flow(
            Err(AppError::Network("content failed".into())),
            || async {
                called.store(true, Ordering::SeqCst);
                Ok(ready())
            },
            || async { Ok(()) },
        )
        .await
        .expect_err("content failure should be preserved");

        assert_eq!(error.code(), "network");
        assert!(!called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn committed_content_still_runs_prerequisites_when_shortcuts_fail() {
        let prerequisite_called = AtomicBool::new(false);
        let post_install_called = AtomicBool::new(false);

        complete_install_flow(
            Ok(()),
            || async {
                prerequisite_called.store(true, Ordering::SeqCst);
                Ok(ready())
            },
            || async {
                post_install_called.store(true, Ordering::SeqCst);
                Err(AppError::FileSystem("shortcut denied".into()))
            },
        )
        .await
        .expect("post-commit shortcut failure must not invalidate installed content");

        assert!(prerequisite_called.load(Ordering::SeqCst));
        assert!(post_install_called.load(Ordering::SeqCst));
    }
}
