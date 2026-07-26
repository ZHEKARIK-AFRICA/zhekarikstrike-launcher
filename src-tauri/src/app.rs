use std::time::Duration;

use tauri::{Emitter, Manager, Size};

use crate::commands::*;
use crate::logger;
use crate::services::{config_service, elevation_service, launcher_update_service};
use crate::state::AppState;

const MAIN_WINDOW_SIZE: (f64, f64) = (892.0, 496.0);
const UPDATE_WINDOW_SIZE: (f64, f64) = (788.0, 272.0);

pub fn run() {
    let config = tauri::async_runtime::block_on(config_service::load_config()).unwrap_or_default();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_shell::init())
        .manage(AppState::new(config))
        .invoke_handler(tauri::generate_handler![
            minimize_window,
            close_window,
            show_main_window,
            set_window_layout,
            get_config,
            get_game_path,
            set_game_path,
            get_language,
            set_language,
            translate,
            get_game_version,
            get_current_state,
            get_startup_state,
            check_game_exists,
            get_game_process_state,
            select_game_folder,
            open_external_url,
            check_disk_space_for_install,
            create_shortcuts,
            is_elevated,
            relaunch_as_admin,
            move_launcher_to_game_path,
            install_game,
            cancel_install,
            verify_files,
            update_game,
            cancel_verify,
            get_game_data,
            update_rev_ini,
            launch_game,
            stop_game,
            check_launcher_update,
            download_launcher_update,
            apply_launcher_update
        ])
        .setup(|app| {
            logger::init()?;

            if !cfg!(debug_assertions) && !elevation_service::is_elevated()? {
                if !elevation_service::elevation_attempted() {
                    elevation_service::relaunch_as_admin()?;
                    std::process::exit(0);
                }

                eprintln!("Launcher is still not elevated after relaunch attempt");
            }

            logger::set_app_handle(app.handle().clone());

            let update_status = tauri::async_runtime::block_on(
                launcher_update_service::check_launcher_update(env!("CARGO_PKG_VERSION")),
            );
            let show_update = match update_status {
                Ok(status) if status.has_update && status.can_apply => {
                    logger::info(&format!(
                        "signed launcher update available: {} -> {}",
                        status.current_version, status.latest_version
                    ));
                    true
                }
                Ok(status) if status.has_update => {
                    logger::warn(&format!(
                        "launcher update blocked: {}",
                        status
                            .blocked_reason
                            .unwrap_or_else(|| "unknown reason".to_string())
                    ));
                    false
                }
                Ok(_) => false,
                Err(error) => {
                    logger::warn(&format!("launcher update check failed: {error}"));
                    false
                }
            };

            let handle = app.handle().clone();
            if let Some(window) = app.get_webview_window("main") {
                if show_update {
                    window.set_size(Size::Logical(tauri::LogicalSize {
                        width: UPDATE_WINDOW_SIZE.0,
                        height: UPDATE_WINDOW_SIZE.1,
                    }))?;
                    window.eval("window.location.replace('launcher_update.html')")?;
                } else {
                    window.set_size(Size::Logical(tauri::LogicalSize {
                        width: MAIN_WINDOW_SIZE.0,
                        height: MAIN_WINDOW_SIZE.1,
                    }))?;
                }
                window.show()?;
                window.set_focus()?;

                let close_handle = app.handle().clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let app = close_handle.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Some(state) = app.try_state::<AppState>() {
                                if let Err(error) = crate::services::shutdown_service::shutdown(
                                    app.clone(),
                                    state.inner(),
                                )
                                .await
                                {
                                    logger::error(&format!("shutdown cleanup failed: {error}"));
                                }
                            }
                            app.exit(0);
                        });
                    }
                });
            }

            if !show_update {
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(4_500)).await;
                    let next_page =
                        match crate::commands::config_commands::check_game_exists().await {
                            Ok(status) if status.exists => "./public/index.html",
                            _ => "./public/install.html",
                        };
                    let _ = handle.emit("start-fade-out", next_page);
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
