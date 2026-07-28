use tauri::{AppHandle, Manager, State};

use crate::error::AppError;
use crate::services::{
    shutdown_service,
    window_resize_service::{self, MAIN_WINDOW_LAYOUT, UPDATE_WINDOW_LAYOUT},
};
use crate::state::AppState;

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
        let layout = if page.contains("launcher_update") {
            UPDATE_WINDOW_LAYOUT
        } else {
            MAIN_WINDOW_LAYOUT
        };
        window_resize_service::apply_layout(&window, layout)?;
        window.center()?;
    }
    Ok(())
}
