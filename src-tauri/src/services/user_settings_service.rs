/// Files owned by the player after packaged defaults have been installed.
/// This local policy cannot be relaxed by stale publisher exclusion flags.
pub(crate) fn is_user_settings_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let name = if let Some(name) = normalized.strip_prefix("csgo/cfg/") {
        name
    } else if let Some(name) = normalized
        .strip_prefix("csgo/")
        .or_else(|| normalized.strip_prefix("cfg/"))
    {
        return matches!(name, "video.txt" | "videodefaults.txt" | "video.cfg");
    } else {
        let parts = normalized.split('/').collect::<Vec<_>>();
        if parts.len() != 6
            || parts[0] != "userdata"
            || parts[1].is_empty()
            || !parts[1].bytes().all(|byte| byte.is_ascii_digit())
            || parts[2..5] != ["730", "local", "cfg"]
        {
            return false;
        }
        return matches!(
            parts[5],
            "config.cfg"
                | "autoexec.cfg"
                | "video.txt"
                | "videodefaults.txt"
                | "userconfig.cfg"
                | "joystick.cfg"
        );
    };
    matches!(
        name,
        "config.cfg"
            | "autoexec.cfg"
            | "video.txt"
            | "videodefaults.txt"
            | "userconfig.cfg"
            | "joystick.cfg"
    )
}
