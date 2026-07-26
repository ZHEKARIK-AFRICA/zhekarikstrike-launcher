use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VersionInfo {
    #[serde(default)]
    pub game_version: String,
    #[serde(default)]
    pub launcher_version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LauncherUpdateStatus {
    #[serde(rename = "hasUpdate")]
    pub has_update: bool,
    #[serde(rename = "canApply")]
    pub can_apply: bool,
    #[serde(rename = "blockedReason")]
    pub blocked_reason: Option<String>,
    #[serde(rename = "currentVersion")]
    pub current_version: String,
    #[serde(rename = "latestVersion")]
    pub latest_version: String,
    #[serde(rename = "downloadSize")]
    pub download_size: Option<u64>,
}
