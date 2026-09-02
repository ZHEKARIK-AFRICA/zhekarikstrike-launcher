use std::time::Duration;

use tauri::{Emitter, Manager};

use crate::commands::*;
use crate::error::AppError;
use crate::logger;
use crate::services::{
    elevation_service, launcher_update_service,
    window_resize_service::{self, MAIN_WINDOW_LAYOUT, UPDATE_WINDOW_LAYOUT},
};
use crate::state::AppState;

#[derive(Debug, PartialEq, Eq)]
enum StartupElevationAction {
    Continue,
    Relaunch,
    Reject,
}

fn startup_elevation_action(
    debug_build: bool,
    elevated: bool,
    elevation_attempted: bool,
) -> StartupElevationAction {
    if debug_build || elevated {
        StartupElevationAction::Continue
    } else if elevation_attempted {
        StartupElevationAction::Reject
    } else {
        StartupElevationAction::Relaunch
    }
}

pub fn run() {
    let debug_build = cfg!(debug_assertions);
    let elevated = elevation_service::is_elevated().unwrap_or(false);
    if !debug_build
        && !elevated
        && use_existing_instance_before_elevation(
            debug_build,
            elevated,
            focus_existing_main_window(),
        )
    {
        return;
    }

    let builder = tauri::Builder::default();
    let builder = if single_instance_enabled(debug_build, elevated) {
        builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
    } else {
        builder
    };
    let builder = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_opener::init());
    #[cfg(feature = "e2e")]
    let builder = builder
        .plugin(tauri_plugin_wdio::init())
        .plugin(tauri_plugin_wdio_webdriver::init());

    builder
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            minimize_window,
            close_window,
            confirm_close_window,
            cancel_close_window,
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
            recover_pending_install,
            ensure_game_prerequisites,
            get_prerequisite_state,
            acknowledge_prerequisite_state,
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

            match startup_elevation_action(
                cfg!(debug_assertions),
                elevation_service::is_elevated()?,
                elevation_service::elevation_attempted(),
            ) {
                StartupElevationAction::Continue => {}
                StartupElevationAction::Relaunch => {
                    elevation_service::relaunch_as_admin()?;
                    std::process::exit(0);
                }
                StartupElevationAction::Reject => return Err(AppError::AdminRequired.into()),
            }

            logger::set_app_handle(app.handle().clone());

            if let Ok(Some(game_path)) =
                tauri::async_runtime::block_on(crate::services::config_service::get_game_path())
            {
                tauri::async_runtime::spawn(async move {
                    if let Err(error) =
                        crate::services::content_commit_service::retry_background_cleanup(
                            &game_path,
                        )
                        .await
                    {
                        crate::logger::warn(&format!(
                            "startup content cleanup retry failed: {error}"
                        ));
                    }
                });
            }

            #[cfg(feature = "e2e")]
            let show_update = false;
            #[cfg(not(feature = "e2e"))]
            let show_update = match tauri::async_runtime::block_on(
                launcher_update_service::check_launcher_update(env!("CARGO_PKG_VERSION")),
            ) {
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
                    logger::warn(&format!(
                        "SECURITY WARNING: signed launcher update check rejected: {error}"
                    ));
                    false
                }
            };

            let handle = app.handle().clone();
            if let Some(window) = app.get_webview_window("main") {
                let layout = if show_update {
                    UPDATE_WINDOW_LAYOUT
                } else {
                    MAIN_WINDOW_LAYOUT
                };
                window_resize_service::initialize(&window, layout)?;
                if show_update {
                    window.eval("window.location.replace('launcher_update.html')")?;
                }
                window.show()?;
                window.set_focus()?;

                let close_handle = app.handle().clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        logger::info("window close requested");
                        // Only the controlled shutdown path may actually close the window.
                        // In particular, a second Alt+F4 must not bypass cancellation/cleanup.
                        api.prevent_close();
                        let Some(state) = close_handle.try_state::<AppState>() else {
                            logger::warn("window close requested without managed app state");
                            return;
                        };
                        if state.shutdown_started() {
                            logger::info(
                                "window close ignored while controlled shutdown is running",
                            );
                            return;
                        }
                        crate::services::close_service::spawn_close_request(close_handle.clone());
                    }
                });
            }

            #[cfg(not(feature = "e2e"))]
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

            #[cfg(feature = "e2e")]
            let _ = handle;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn single_instance_enabled(debug_build: bool, elevated: bool) -> bool {
    debug_build || elevated
}

fn use_existing_instance_before_elevation(
    debug_build: bool,
    elevated: bool,
    existing_window_focused: bool,
) -> bool {
    !debug_build && !elevated && existing_window_focused
}

fn launcher_executable_matches(existing: &str, current: &str) -> bool {
    existing
        .replace('/', "\\")
        .eq_ignore_ascii_case(&current.replace('/', "\\"))
}

#[cfg(target_os = "windows")]
fn focus_existing_main_window() -> bool {
    use std::os::windows::ffi::OsStrExt;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    let title = std::ffi::OsStr::new(crate::constants::PRODUCT_NAME)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let Ok(current_executable) = std::env::current_exe() else {
        return false;
    };
    let mut previous = HWND::default();
    loop {
        let Ok(window) = (unsafe {
            FindWindowExW(
                HWND::default(),
                previous,
                PCWSTR::null(),
                PCWSTR(title.as_ptr()),
            )
        }) else {
            return false;
        };
        previous = window;
        let Some(existing_executable) = window_process_executable(window) else {
            continue;
        };
        if !launcher_executable_matches(&existing_executable, &current_executable.to_string_lossy())
        {
            continue;
        }

        unsafe {
            let _ = ShowWindow(window, SW_RESTORE);
            let _ = SetForegroundWindow(window);
        }
        return true;
    }
}

#[cfg(target_os = "windows")]
fn window_process_executable(window: windows::Win32::Foundation::HWND) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    let mut pid = 0_u32;
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut image_path = vec![0_u16; 32_768];
    let mut image_path_len = image_path.len() as u32;
    let query_result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(image_path.as_mut_ptr()),
            &mut image_path_len,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    query_result.ok()?;
    Some(String::from_utf16_lossy(
        &image_path[..image_path_len as usize],
    ))
}

#[cfg(not(target_os = "windows"))]
fn focus_existing_main_window() -> bool {
    false
}

#[cfg(test)]
mod release_1_6_11_tests {
    use super::{
        launcher_executable_matches, single_instance_enabled,
        use_existing_instance_before_elevation,
    };

    #[test]
    fn release_1_6_11_single_instance_waits_until_release_process_is_elevated() {
        assert!(single_instance_enabled(true, false));
        assert!(single_instance_enabled(false, true));
        assert!(!single_instance_enabled(false, false));
    }

    #[test]
    fn release_1_6_11_second_release_launch_uses_existing_window_without_uac() {
        assert!(use_existing_instance_before_elevation(false, false, true));
        assert!(!use_existing_instance_before_elevation(false, false, false));
        assert!(!use_existing_instance_before_elevation(false, true, true));
        assert!(!use_existing_instance_before_elevation(true, false, true));
    }

    #[test]
    fn release_1_6_11_pre_elevation_focus_requires_the_same_launcher_executable() {
        assert!(launcher_executable_matches(
            r"D:\Games\ZHEKARIK STRIKE.exe",
            r"d:/games/zhekarik strike.exe"
        ));
        assert!(!launcher_executable_matches(
            r"D:\Games\zhekarikstrike.exe",
            r"D:\Games\ZHEKARIK STRIKE.exe"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{startup_elevation_action, StartupElevationAction};

    #[test]
    fn release_build_relaunches_when_not_elevated() {
        assert_eq!(
            startup_elevation_action(false, false, false),
            StartupElevationAction::Relaunch
        );
    }

    #[test]
    fn release_build_rejects_a_second_unelevated_process() {
        assert_eq!(
            startup_elevation_action(false, false, true),
            StartupElevationAction::Reject
        );
    }

    #[test]
    fn elevated_release_and_debug_builds_can_continue() {
        assert_eq!(
            startup_elevation_action(false, true, false),
            StartupElevationAction::Continue
        );
        assert_eq!(
            startup_elevation_action(true, false, false),
            StartupElevationAction::Continue
        );
    }
}
