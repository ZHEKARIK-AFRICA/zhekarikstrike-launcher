use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LauncherConfig {
    #[serde(rename = "gamePath")]
    pub game_path: Option<String>,

    pub language: Option<String>,

    #[serde(rename = "gameVersion")]
    pub game_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StartupState {
    #[serde(rename = "launcherUpdateRequired")]
    pub launcher_update_required: bool,
    #[serde(rename = "gameExists")]
    pub game_exists: bool,
    #[serde(rename = "gamePath")]
    pub game_path: Option<String>,
    #[serde(rename = "language")]
    pub language: String,
}
