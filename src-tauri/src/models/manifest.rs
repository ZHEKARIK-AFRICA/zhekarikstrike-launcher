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
pub struct LauncherUpdateManifest {
    pub version: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    pub platforms: Option<HashMap<String, LauncherUpdateManifestPlatform>>,
    pub url: Option<String>,
    pub signature: Option<String>,
    pub sha256: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LauncherUpdateManifestPlatform {
    pub url: String,
    pub sha256: String,
    pub signature: String,
    pub size: u64,
}

impl LauncherUpdateManifest {
    pub fn platform(&self, key: &str) -> Option<LauncherUpdateManifestPlatform> {
        if let Some(platform) = self
            .platforms
            .as_ref()
            .and_then(|platforms| platforms.get(key))
        {
            return Some(platform.clone());
        }

        Some(LauncherUpdateManifestPlatform {
            url: self.url.clone()?,
            sha256: self.sha256.clone()?,
            signature: self.signature.clone()?,
            size: self.size?,
        })
    }
}
