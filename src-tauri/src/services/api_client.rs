use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;

use crate::constants::MODERN_API_BASE_URL;
use crate::error::AppError;
use crate::models::{
    validate_game_path, GameManifest, GamePatchManifest, GamePatchManifestEntry,
    LauncherUpdateManifest,
};

const METADATA_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct ApiClient {
    http: Client,
}

impl ApiClient {
    pub fn new() -> Result<Self, AppError> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .pool_idle_timeout(Duration::from_secs(90))
            .user_agent(concat!(
                "ZHEKARIK-STRIKE-Launcher/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()?;

        Ok(Self { http })
    }

    pub fn http(&self) -> &Client {
        &self.http
    }

    pub async fn get_full_manifest(&self) -> Result<GameManifest, AppError> {
        let manifest = self.fetch_manifest(Self::full_manifest_url()).await?;
        manifest.validate_complete()?;
        Ok(manifest)
    }

    pub async fn get_additional_manifest(&self) -> Result<GameManifest, AppError> {
        let manifest = self.fetch_manifest(Self::additional_manifest_url()).await?;
        manifest.validate_subset()?;
        Ok(manifest)
    }

    pub async fn get_updates_from(&self, version: &str) -> Result<GameManifest, AppError> {
        let manifest: GameManifest = self
            .metadata_get(Self::updates_url())
            .query(&[("from_version", version)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        manifest.validate_update()?;
        Ok(manifest)
    }

    pub async fn get_exclude_files(&self) -> Result<Vec<String>, AppError> {
        #[derive(Deserialize)]
        struct ExcludeResponse {
            files: Option<Vec<String>>,
        }

        let response: ExcludeResponse = self
            .metadata_get(Self::excludes_url())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let files = response.files.unwrap_or_default();
        for path in &files {
            validate_game_path(path)?;
        }
        Ok(files)
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
            .metadata_get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub async fn get_game_patch_manifest(&self) -> Result<GamePatchManifest, AppError> {
        let url = format!("{MODERN_API_BASE_URL}/launcher/game-files/manifest");
        Ok(self
            .metadata_get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub fn game_patch_download_url(file: &GamePatchManifestEntry) -> String {
        let encoded = file
            .path
            .split('/')
            .map(url_encode_segment)
            .collect::<Vec<_>>()
            .join("/");
        format!(
            "{MODERN_API_BASE_URL}/launcher/game-files/{}/{}",
            file.layer.as_str(),
            encoded
        )
    }

    pub fn full_manifest_url() -> &'static str {
        "https://api.zhekarik.africa/launcher/game/manifest"
    }

    pub fn additional_manifest_url() -> &'static str {
        "https://api.zhekarik.africa/launcher/game/additional"
    }

    pub fn updates_url() -> &'static str {
        "https://api.zhekarik.africa/launcher/game/updates"
    }

    pub fn excludes_url() -> &'static str {
        "https://api.zhekarik.africa/launcher/game/excludes"
    }

    async fn fetch_manifest(&self, url: &str) -> Result<GameManifest, AppError> {
        Ok(self
            .metadata_get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    fn metadata_get(&self, url: &str) -> reqwest::RequestBuilder {
        self.http.get(url).timeout(METADATA_REQUEST_TIMEOUT)
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

#[cfg(test)]
mod tests {
    use super::ApiClient;
    use crate::models::{GamePatchLayer, GamePatchManifestEntry};

    #[test]
    fn game_patch_download_url_uses_the_fixed_backend_origin_and_encoded_segments() {
        let file = GamePatchManifestEntry {
            layer: GamePatchLayer::GameFilesPure,
            path: "csgo/scripts/items game.txt".to_string(),
            size: 4,
            sha256: "a".repeat(64),
        };

        assert_eq!(
            ApiClient::game_patch_download_url(&file),
            "https://api.zhekarik.africa/launcher/game-files/game_files_pure/csgo/scripts/items%20game.txt"
        );
    }

    #[test]
    fn game_client_api_uses_only_the_public_https_contract() {
        assert_eq!(
            ApiClient::full_manifest_url(),
            "https://api.zhekarik.africa/launcher/game/manifest"
        );
        assert_eq!(
            ApiClient::additional_manifest_url(),
            "https://api.zhekarik.africa/launcher/game/additional"
        );
        assert_eq!(
            ApiClient::updates_url(),
            "https://api.zhekarik.africa/launcher/game/updates"
        );
        assert_eq!(
            ApiClient::excludes_url(),
            "https://api.zhekarik.africa/launcher/game/excludes"
        );
    }
}
