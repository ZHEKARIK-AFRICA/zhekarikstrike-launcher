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

#[cfg(test)]
mod tests {
    use super::LauncherConfig;

    #[test]
    fn reads_legacy_config_without_version_or_language() {
        let config: LauncherConfig =
            serde_json::from_str(r#"{"gamePath":"D:\\Games\\ZHEKARIKSTRIKE"}"#)
                .expect("legacy config should deserialize");

        assert_eq!(
            config.game_path.as_deref(),
            Some("D:\\Games\\ZHEKARIKSTRIKE")
        );
        assert_eq!(config.game_version, None);
        assert_eq!(config.language, None);
    }

    #[test]
    fn preserves_existing_config_keys() {
        let config: LauncherConfig = serde_json::from_str(
            r#"{"gamePath":"D:\\Games\\ZS","gameVersion":"1.6.0","language":"en"}"#,
        )
        .expect("current config should deserialize");
        let serialized = serde_json::to_value(config).expect("config should serialize");

        assert_eq!(serialized["gamePath"], "D:\\Games\\ZS");
        assert_eq!(serialized["gameVersion"], "1.6.0");
        assert_eq!(serialized["language"], "en");
    }
}
