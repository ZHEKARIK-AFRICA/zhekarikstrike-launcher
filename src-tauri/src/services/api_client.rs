use std::collections::HashMap;
use std::time::Duration;

use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;

use crate::constants::{API_BASE_URL, DOWNLOAD_BASE_URL, MODERN_API_BASE_URL};
use crate::error::AppError;
use crate::models::{
    GameArchiveManifest, GameFileManifestEntry, GameManifest, LauncherUpdateManifest, VersionInfo,
};
use crate::utils::path_utils::normalize_manifest_path;

#[derive(Clone)]
pub struct ApiClient {
    http: Client,
    api_base_url: String,
    download_base_url: String,
}

impl ApiClient {
    pub fn new() -> Result<Self, AppError> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .pool_idle_timeout(Duration::from_secs(90))
            .user_agent("ZHEKARIK-STRIKE-Launcher/1.6.1")
            .build()?;

        Ok(Self {
            http,
            api_base_url: API_BASE_URL.to_string(),
            download_base_url: DOWNLOAD_BASE_URL.to_string(),
        })
    }

    pub fn http(&self) -> &Client {
        &self.http
    }

    pub async fn get_version_info(&self) -> Result<VersionInfo, AppError> {
        let url = format!("{}/version_number", self.api_base_url);
        Ok(self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub async fn get_full_manifest(&self) -> Result<GameManifest, AppError> {
        self.compat_manifest("/all_files").await
    }

    pub async fn get_additional_manifest(&self) -> Result<GameManifest, AppError> {
        self.compat_manifest("/additional_check").await
    }

    pub async fn get_updates_from(&self, version: &str) -> Result<GameManifest, AppError> {
        let url = format!("{}/get_updates", self.api_base_url);
        let response: CompatUpdatesResponse = self
            .http
            .get(url)
            .query(&[("from_version", version)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        self.manifest_from_map(response.updates.unwrap_or_default())
            .await
    }

    pub async fn get_exclude_files(&self) -> Result<Vec<String>, AppError> {
        #[derive(Deserialize)]
        struct ExcludeResponse {
            files: Option<Vec<String>>,
        }

        let url = format!("{}/exclude_files", self.api_base_url);
        let response: ExcludeResponse = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(response
            .files
            .unwrap_or_default()
            .into_iter()
            .map(|path| normalize_manifest_path(&path))
            .collect())
    }

    pub async fn get_launcher_update(
        &self,
        current_version: &str,
    ) -> Result<LauncherUpdateManifest, AppError> {
        let url = format!(
            "{}/launcher/update/windows/x86_64/{}",
            MODERN_API_BASE_URL, current_version
        );
        Ok(self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub async fn get_archive_manifest(&self) -> Result<GameArchiveManifest, AppError> {
        Ok(GameArchiveManifest {
            url: format!("{}/download_game_archive", self.download_base_url),
            size: Some(9_216 * 1024 * 1024),
            sha256: None,
        })
    }

    async fn compat_manifest(&self, endpoint: &str) -> Result<GameManifest, AppError> {
        let url = format!("{}{}", self.api_base_url, endpoint);
        let response: CompatFilesResponse = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        self.manifest_from_map(response.files.unwrap_or_default())
            .await
    }

    async fn manifest_from_map(
        &self,
        files: HashMap<String, CompatFileData>,
    ) -> Result<GameManifest, AppError> {
        let version = self.get_version_info().await.unwrap_or_default();

        let entries = files
            .into_iter()
            .map(|(path, data)| {
                let normalized = normalize_manifest_path(&path);
                let encoded = normalized
                    .split('/')
                    .map(url_encode_segment)
                    .collect::<Vec<_>>()
                    .join("/");

                GameFileManifestEntry {
                    path: normalized,
                    size: data.size,
                    md5: data.hash.or(data.md5),
                    sha256: data.sha256,
                    url: format!("{}/download/{}", self.download_base_url, encoded),
                    excluded_from_hash_check: false,
                    temporary: false,
                }
            })
            .collect();

        Ok(GameManifest {
            game_version: if version.game_version.is_empty() {
                "0.0.0".to_string()
            } else {
                version.game_version
            },
            generated_at: Some(Utc::now().to_rfc3339()),
            files: entries,
            archive: Some(self.get_archive_manifest().await?),
        })
    }
}

fn url_encode_segment(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct CompatFilesResponse {
    files: Option<HashMap<String, CompatFileData>>,
}

#[derive(Debug, Deserialize)]
struct CompatUpdatesResponse {
    updates: Option<HashMap<String, CompatFileData>>,
}

#[derive(Debug, Deserialize)]
struct CompatFileData {
    hash: Option<String>,
    md5: Option<String>,
    sha256: Option<String>,
    size: Option<u64>,
}
