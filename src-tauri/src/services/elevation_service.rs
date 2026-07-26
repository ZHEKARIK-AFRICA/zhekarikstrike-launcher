use std::env;
use std::ffi::OsString;
use std::path::Path;

use crate::constants::ELEVATION_ATTEMPT_ARG;
use crate::error::AppError;

pub fn elevation_attempted() -> bool {
    env::args().any(|arg| arg == ELEVATION_ATTEMPT_ARG)
        || env::var_os("ZHEKARIK_ELEVATION_ATTEMPTED").is_some()
}

#[cfg(target_os = "windows")]
pub fn is_elevated() -> Result<bool, AppError> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = HANDLE::default();
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|error| AppError::Unknown(format!("OpenProcessToken failed: {error}")))?;
    }

    let result = (|| {
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0_u32;
        unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elevation as *mut _ as *mut _),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
        }
        .map_err(|error| AppError::Unknown(format!("GetTokenInformation failed: {error}")))?;

        Ok(elevation.TokenIsElevated != 0)
    })();

    unsafe {
        let _ = CloseHandle(token);
    }

    result
}

#[cfg(not(target_os = "windows"))]
pub fn is_elevated() -> Result<bool, AppError> {
    Ok(true)
}

#[cfg(target_os = "windows")]
pub fn relaunch_as_admin() -> Result<(), AppError> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let exe = env::current_exe()?;
    let mut args: Vec<OsString> = env::args_os().skip(1).collect();
    if !args.iter().any(|arg| arg == ELEVATION_ATTEMPT_ARG) {
        args.push(OsString::from(ELEVATION_ATTEMPT_ARG));
    }

    let parameters = args
        .iter()
        .map(|arg| quote_argument(&arg.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");

    let operation = wide_str("runas");
    let file = wide_path(&exe);
    let params = wide_str(&parameters);

    let instance = unsafe {
        ShellExecuteW(
            HWND::default(),
            PCWSTR(operation.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(params.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };

    if instance.0 as isize <= 32 {
        return Err(AppError::AdminRequired);
    }

    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn relaunch_as_admin() -> Result<(), AppError> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(target_os = "windows")]
fn wide_str(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn quote_argument(value: &str) -> String {
    if value.is_empty() || value.contains([' ', '\t', '"']) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}
