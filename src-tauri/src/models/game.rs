use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct GameData {
    pub nickname: Option<String>,
    #[serde(rename = "clanTag")]
    pub clan_tag: Option<String>,
    #[serde(rename = "launchParams")]
    pub launch_params: Option<String>,
    #[serde(rename = "gamePath")]
    pub game_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GameExistenceStatus {
    pub exists: bool,
    #[serde(rename = "gamePath")]
    pub game_path: Option<String>,
    #[serde(rename = "missingFiles")]
    pub missing_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GameProcessInfo {
    pub pid: u32,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum GameProcessStateKind {
    #[default]
    Stopped,
    Starting,
    Running,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GameProcessState {
    pub kind: GameProcessStateKind,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporaryFileRule {
    pub path: String,
    #[serde(rename = "replaceAfterMs")]
    pub replace_after_ms: u64,
    #[serde(rename = "replacementSource")]
    pub replacement_source: String,
}
