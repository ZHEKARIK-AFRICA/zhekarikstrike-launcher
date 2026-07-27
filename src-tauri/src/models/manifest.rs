use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::DateTime;
use serde::{Deserialize, Serialize};

use crate::constants::MODERN_API_BASE_URL;
use crate::error::AppError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum GamePatchLayer {
    GameFiles,
    GameFilesPure,
}

impl GamePatchLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GameFiles => "game_files",
            Self::GameFilesPure => "game_files_pure",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GamePatchManifest {
    pub files: Vec<GamePatchManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GamePatchManifestEntry {
    pub layer: GamePatchLayer,
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameManifest {
    pub game_version: String,
    pub generated_at: String,
    pub files: Vec<GameFileManifestEntry>,
    pub archive: GameArchiveManifest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameFileManifestEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub url: String,
    pub excluded_from_hash_check: bool,
    pub temporary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameArchiveManifest {
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub unpacked_size: u64,
}

impl GameManifest {
    pub fn validate_complete(&self) -> Result<(), AppError> {
        self.validate(true)
    }

    pub fn validate_subset(&self) -> Result<(), AppError> {
        self.validate(false)
    }

    pub fn validate_update(&self) -> Result<(), AppError> {
        self.validate(!self.files.is_empty())
    }

    fn validate(&self, complete: bool) -> Result<(), AppError> {
        if !is_game_version(&self.game_version) {
            return invalid_manifest("game_version must be X.Y.Z or X.Y.Z.W");
        }
        DateTime::parse_from_rfc3339(&self.generated_at)
            .map_err(|_| AppError::InvalidData("game manifest generated_at is invalid".into()))?;
        validate_sha256(&self.archive.sha256, "archive")?;
        if self.archive.size == 0 || self.archive.unpacked_size == 0 {
            return invalid_manifest("archive sizes must be greater than zero");
        }
        let expected_archive_url = format!("{MODERN_API_BASE_URL}/launcher/game/archive");
        if self.archive.url != expected_archive_url {
            return invalid_manifest("archive URL is outside the trusted HTTPS API");
        }
        if complete && self.files.is_empty() {
            return invalid_manifest("complete manifest has no files");
        }

        let mut paths = HashSet::new();
        let mut unpacked_size = 0_u64;
        let mut has_revloader = false;
        for file in &self.files {
            validate_game_path(&file.path)?;
            let key = file.path.to_lowercase();
            if !paths.insert(key) {
                return invalid_manifest("manifest contains duplicate file paths");
            }
            validate_sha256(&file.sha256, &file.path)?;
            let expected_url = expected_game_file_url(&file.path);
            if file.url != expected_url {
                return invalid_manifest("file URL is outside the trusted HTTPS API");
            }
            unpacked_size = unpacked_size
                .checked_add(file.size)
                .ok_or_else(|| AppError::InvalidData("game manifest size overflow".into()))?;
            has_revloader |= file.path == "RevLoader.exe";
        }

        if complete && !has_revloader {
            return invalid_manifest("complete manifest does not contain RevLoader.exe");
        }
        if complete && unpacked_size != self.archive.unpacked_size {
            return invalid_manifest("archive unpacked size does not match the file manifest");
        }
        Ok(())
    }
}

pub fn validate_game_path(path: &str) -> Result<(), AppError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains(':')
    {
        return invalid_manifest("game manifest contains an unsafe path");
    }

    for part in path.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part.chars().any(|character| {
                character.is_control() || matches!(character, '<' | '>' | '"' | '|' | '?' | '*')
            })
            || is_windows_device_name(part)
        {
            return invalid_manifest("game manifest contains an unsafe path");
        }
    }
    Ok(())
}

pub fn expected_game_file_url(path: &str) -> String {
    let encoded = path
        .split('/')
        .map(url_encode_segment)
        .collect::<Vec<_>>()
        .join("/");
    format!("{MODERN_API_BASE_URL}/launcher/game/files/{encoded}")
}

fn url_encode_segment(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (*byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn is_game_version(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    matches!(parts.len(), 3 | 4)
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn validate_sha256(value: &str, label: &str) -> Result<(), AppError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid_manifest(&format!("invalid sha256 for {label}"));
    }
    Ok(())
}

fn is_windows_device_name(part: &str) -> bool {
    let stem = part
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

fn invalid_manifest<T>(message: &str) -> Result<T, AppError> {
    Err(AppError::InvalidData(message.to_string()))
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

#[derive(Debug, Clone)]
pub struct VerifiedLauncherUpdate {
    pub path: PathBuf,
    pub sha256: String,
    pub signature: String,
    pub size: u64,
}

impl LauncherUpdateManifest {
    pub fn platform(&self, key: &str) -> Option<&LauncherUpdateManifestPlatform> {
        self.platforms.get(key)
    }
}

#[cfg(test)]
mod game_client_tests {
    use super::GameManifest;

    fn manifest_json(path: &str, url: &str, sha256: &str) -> String {
        serde_json::json!({
            "game_version": "1.0.3.4",
            "generated_at": "2026-07-27T00:00:00Z",
            "files": [{
                "path": path,
                "size": 4,
                "sha256": sha256,
                "url": url,
                "excluded_from_hash_check": false,
                "temporary": false
            }],
            "archive": {
                "url": "https://api.zhekarik.africa/launcher/game/archive",
                "size": 10,
                "sha256": "a".repeat(64),
                "unpacked_size": 4
            }
        })
        .to_string()
    }

    #[test]
    fn complete_manifest_requires_strict_https_paths_hashes_and_revloader() {
        let valid: GameManifest = serde_json::from_str(&manifest_json(
            "RevLoader.exe",
            "https://api.zhekarik.africa/launcher/game/files/RevLoader.exe",
            &"b".repeat(64),
        ))
        .expect("valid manifest should deserialize");
        valid
            .validate_complete()
            .expect("valid complete manifest should pass");

        for (path, url) in [
            (
                "../RevLoader.exe",
                "https://api.zhekarik.africa/launcher/game/files/../RevLoader.exe",
            ),
            (
                "RevLoader.exe",
                "http://api.zhekarik.africa/launcher/game/files/RevLoader.exe",
            ),
            (
                "RevLoader.exe",
                "https://evil.example/launcher/game/files/RevLoader.exe",
            ),
        ] {
            let manifest: GameManifest =
                serde_json::from_str(&manifest_json(path, url, &"b".repeat(64)))
                    .expect("shape should deserialize before semantic validation");
            assert!(manifest.validate_complete().is_err());
        }

        let bad_hash: GameManifest = serde_json::from_str(&manifest_json(
            "RevLoader.exe",
            "https://api.zhekarik.africa/launcher/game/files/RevLoader.exe",
            &"B".repeat(64),
        ))
        .expect("shape should deserialize before semantic validation");
        assert!(bad_hash.validate_complete().is_err());
    }

    #[test]
    fn complete_manifest_rejects_case_collisions_and_incomplete_archive() {
        let mut document: serde_json::Value = serde_json::from_str(&manifest_json(
            "RevLoader.exe",
            "https://api.zhekarik.africa/launcher/game/files/RevLoader.exe",
            &"b".repeat(64),
        ))
        .unwrap();
        let duplicate = serde_json::json!({
            "path": "revloader.EXE",
            "size": 4,
            "sha256": "c".repeat(64),
            "url": "https://api.zhekarik.africa/launcher/game/files/revloader.EXE",
            "excluded_from_hash_check": false,
            "temporary": false
        });
        document["files"].as_array_mut().unwrap().push(duplicate);
        document["archive"]["unpacked_size"] = serde_json::json!(8);
        let duplicate: GameManifest = serde_json::from_value(document).unwrap();
        assert!(duplicate.validate_complete().is_err());

        let mut incomplete: serde_json::Value = serde_json::from_str(&manifest_json(
            "RevLoader.exe",
            "https://api.zhekarik.africa/launcher/game/files/RevLoader.exe",
            &"b".repeat(64),
        ))
        .unwrap();
        incomplete["archive"]
            .as_object_mut()
            .unwrap()
            .remove("sha256");
        assert!(serde_json::from_value::<GameManifest>(incomplete).is_err());
    }
}
