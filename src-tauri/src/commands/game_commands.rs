use std::future::Future;
#[cfg(feature = "e2e")]
use tauri::Emitter;

use tauri::{AppHandle, State};

use crate::error::AppError;
use crate::models::validated_operation_id;
use crate::services::{config_service, game_process_service};
use crate::state::{AppState, OperationKind};

#[tauri::command]
pub async fn launch_game(
    app: AppHandle,
    state: State<'_, AppState>,
    operation_id: Option<String>,
) -> Result<(), AppError> {
    let operation_id = validated_operation_id(operation_id)?;
    let recovery_state = state.inner().clone();
    let guard_state = state.inner().clone();
    let launch_state = state.inner().clone();
    let recovery_app = app.clone();
    let launch_app = app.clone();
    run_launch_sequence(
        state.inner(),
        move || async move {
            #[cfg(feature = "e2e")]
            return Ok(());

            #[cfg(not(feature = "e2e"))]
            {
                if game_process_service::game_is_running(&recovery_state).await {
                    return Err(AppError::GameAlreadyRunning);
                }
                let game_path = config_service::get_game_path()
                    .await?
                    .ok_or(AppError::GamePathNotSet)?;
                crate::services::content_journal_service::recover_pending_content(&game_path)
                    .await?;
                let _ = recovery_app;
                Ok(game_path)
            }
        },
        move |game_path| async move {
            #[cfg(feature = "e2e")]
            return Ok(());

            #[cfg(not(feature = "e2e"))]
            {
                let prerequisites =
                    super::prerequisite_commands::check_game_prerequisites_fast_at(&game_path)
                        .await?;
                super::prerequisite_commands::guard_prerequisite_result(prerequisites)?;
                let _ = guard_state;
                Ok(())
            }
        },
        move |game_path| async move {
            #[cfg(feature = "e2e")]
            {
                let _ = (game_path, operation_id, launch_state);
                launch_app.emit("game-started", ())?;
                launch_app.emit("game-closed", ())?;
                Ok(())
            }

            #[cfg(not(feature = "e2e"))]
            game_process_service::launch_game(launch_app, &launch_state, game_path, operation_id)
                .await
        },
    )
    .await
}

async fn run_launch_sequence<
    R,
    T,
    Recover,
    RecoverFuture,
    Guard,
    GuardFuture,
    Launch,
    LaunchFuture,
>(
    state: &AppState,
    recover: Recover,
    guard: Guard,
    launch: Launch,
) -> Result<T, AppError>
where
    R: Clone,
    Recover: FnOnce() -> RecoverFuture,
    RecoverFuture: Future<Output = Result<R, AppError>>,
    Guard: FnOnce(R) -> GuardFuture,
    GuardFuture: Future<Output = Result<(), AppError>>,
    Launch: FnOnce(R) -> LaunchFuture,
    LaunchFuture: Future<Output = Result<T, AppError>>,
{
    let _lease = state.begin_operation(OperationKind::LaunchingGame, None)?;
    let recovered = recover().await?;
    guard(recovered.clone()).await?;
    launch(recovered).await
}

#[tauri::command]
pub async fn stop_game(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    let process = state.process_state.read().await.clone();
    if let Some(pid) = process.pid.filter(|_| process.owned) {
        game_process_service::stop_game_process(pid).await?;
    }
    let _ = app;
    Ok(())
}

#[cfg(test)]
mod launch_lease_tests {
    use std::sync::{Arc, Mutex};

    use super::run_launch_sequence;
    use crate::error::AppError;
    use crate::state::{AppState, CancellationSlot, OperationKind};

    #[tokio::test]
    async fn launch_lease_covers_recovery_guard_and_preparation_in_order() {
        let state = AppState::new();
        let steps = Arc::new(Mutex::new(Vec::new()));
        let recover_steps = steps.clone();
        let guard_steps = steps.clone();
        let launch_steps = steps.clone();
        let recover_state = state.clone();

        run_launch_sequence(
            &state,
            || async move {
                assert_eq!(
                    recover_state.current_state().operation,
                    OperationKind::LaunchingGame
                );
                assert!(recover_state
                    .begin_operation(OperationKind::Installing, Some(CancellationSlot::Install))
                    .is_err());
                recover_steps.lock().unwrap().push("recover");
                Ok(())
            },
            |_| async move {
                guard_steps.lock().unwrap().push("guard");
                Ok(())
            },
            |_| async move {
                launch_steps.lock().unwrap().push("prepare");
                Ok::<_, AppError>(())
            },
        )
        .await
        .expect("guarded launch sequence should finish");

        assert_eq!(*steps.lock().unwrap(), vec!["recover", "guard", "prepare"]);
        assert_eq!(state.current_state().operation, OperationKind::Idle);
    }
}
