use tauri::{AppHandle, Manager, Size, State};

use crate::error::AppError;
use crate::services::shutdown_service;
use crate::state::AppState;

const MAIN_WINDOW_SIZE: (f64, f64) = (892.0, 496.0);
const UPDATE_WINDOW_SIZE: (f64, f64) = (788.0, 272.0);

#[tauri::command]
pub async fn minimize_window(app: AppHandle) -> Result<(), AppError> {
    if let Some(window) = app.get_webview_window("main") {
        window.minimize()?;
    }
    Ok(())
}

#[tauri::command]
pub async fn close_window(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    shutdown_service::shutdown(app.clone(), state.inner()).await?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub async fn show_main_window(app: AppHandle) -> Result<(), AppError> {
    if let Some(window) = app.get_webview_window("main") {
        window.show()?;
        window.set_focus()?;
    }
    Ok(())
}

#[tauri::command]
pub async fn set_window_layout(app: AppHandle, page: String) -> Result<(), AppError> {
    if let Some(window) = app.get_webview_window("main") {
        let size = if page.contains("launcher_update") {
            UPDATE_WINDOW_SIZE
        } else {
            MAIN_WINDOW_SIZE
        };
        window.set_size(Size::Logical(tauri::LogicalSize {
            width: size.0,
            height: size.1,
        }))?;
        window.center()?;
    }
    Ok(())
}
