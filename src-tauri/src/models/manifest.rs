use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GameManifest {
    pub game_version: String,
    pub generated_at: Option<String>,
    pub files: Vec<GameFileManifestEntry>,
    pub archive: Option<GameArchiveManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameFileManifestEntry {
    pub path: String,
    pub size: Option<u64>,
    pub md5: Option<String>,
    pub sha256: Option<String>,
    pub url: String,
    pub excluded_from_hash_check: bool,
    pub temporary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameArchiveManifest {
    pub url: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LauncherUpdateManifest {
    pub version: String,
    pub notes: String,
    pub pub_date: String,
    pub platforms: HashMap<String, LauncherUpdateManifestPlatform>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LauncherUpdateManifestPlatform {
    pub url: String,
    pub sha256: String,
    pub signature: String,
    pub size: u64,
}

impl LauncherUpdateManifest {
    pub fn platform(&self, key: &str) -> Option<&LauncherUpdateManifestPlatform> {
        self.platforms.get(key)
    }
}
