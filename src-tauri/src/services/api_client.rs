use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::Deserialize;

use crate::constants::MODERN_API_BASE_URL;
use crate::error::AppError;
use crate::models::{
    validate_game_path, ContentManifest, ContentMirrorIndex, DrivePackManifest, GameManifest,
    GamePatchManifest, GamePatchManifestEntry, LauncherUpdateManifest,
};

const METADATA_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct ApiClient {
    http: Client,
    direct_http: Client,
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

        let direct_http = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .pool_idle_timeout(Duration::from_secs(90))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!(
                "ZHEKARIK-STRIKE-Launcher/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()?;

        Ok(Self { http, direct_http })
    }

    pub fn http(&self) -> &Client {
        &self.http
    }

    pub fn direct_http(&self) -> &Client {
        &self.direct_http
    }

    pub async fn get_full_manifest(&self) -> Result<GameManifest, AppError> {
        let manifest = self.fetch_manifest(Self::full_manifest_url()).await?;
        manifest.validate_complete()?;
        Ok(manifest)
    }

    pub async fn get_content_manifest(&self) -> Result<Option<ContentManifest>, AppError> {
        let response = self
            .metadata_get(Self::content_manifest_url())
            .send()
            .await?;
        let status = response.status();
        let body = response.bytes().await?;
        parse_content_manifest_response(status, &body)
    }

    pub async fn get_compatible_pack_manifest(
        &self,
    ) -> Result<Option<DrivePackManifest>, AppError> {
        let response = self
            .metadata_get(Self::content_pack_manifest_url())
            .send()
            .await?;
        let status = response.status();
        let body = response.bytes().await?;
        let Some(manifest) = parse_content_pack_manifest_response(status, &body)? else {
            return Ok(None);
        };

        let canonical = self.get_updates_from(&manifest.game_version).await?;
        if !content_version_matches_canonical(&manifest.game_version, &canonical.game_version) {
            return Err(AppError::InvalidData(format!(
                "content v3 game version {} does not match canonical v1 version {}",
                manifest.game_version, canonical.game_version
            )));
        }
        Ok(Some(manifest))
    }

    pub async fn get_compatible_content_manifest(
        &self,
    ) -> Result<Option<ContentManifest>, AppError> {
        let Some(manifest) = self.get_content_manifest().await? else {
            return Ok(None);
        };

        match self.get_updates_from(&manifest.game_version).await {
            Ok(canonical)
                if content_version_matches_canonical(
                    &manifest.game_version,
                    &canonical.game_version,
                ) =>
            {
                Ok(Some(manifest))
            }
            Ok(canonical) => {
                crate::logger::warn(&format!(
                    "content v2 game version {} does not match canonical v1 version {}; using v1",
                    manifest.game_version, canonical.game_version
                ));
                Ok(None)
            }
            Err(error) => {
                crate::logger::warn(&format!(
                    "content v2 compatibility could not be verified; using v1 ({error})"
                ));
                Ok(None)
            }
        }
    }

    pub async fn get_content_drive_mirror(
        &self,
        manifest: &ContentManifest,
    ) -> Result<Option<ContentMirrorIndex>, AppError> {
        let response = self
            .metadata_get(&Self::content_drive_mirror_url(&manifest.content_sha256))
            .send()
            .await?;
        let status = response.status();
        let body = response.bytes().await?;
        parse_content_mirror_response(status, &body, manifest)
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

    pub fn content_manifest_url() -> &'static str {
        "https://api.zhekarik.africa/launcher/game/v2/manifest"
    }

    pub fn content_pack_manifest_url() -> &'static str {
        "https://api.zhekarik.africa/launcher/game/v3/manifest"
    }

    pub fn content_drive_mirror_url(content_sha256: &str) -> String {
        format!(
            "https://api.zhekarik.africa/launcher/game/v2/mirrors/google-drive/{content_sha256}"
        )
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
        let response = self.metadata_get(url).send().await?.error_for_status()?;
        let bytes = response.bytes().await?;
        parse_game_manifest_body(&bytes)
    }

    fn metadata_get(&self, url: &str) -> reqwest::RequestBuilder {
        self.http.get(url).timeout(METADATA_REQUEST_TIMEOUT)
    }
}

fn parse_game_manifest_body(body: &[u8]) -> Result<GameManifest, AppError> {
    serde_json::from_slice(body).map_err(AppError::from)
}

fn content_version_matches_canonical(content_version: &str, canonical_version: &str) -> bool {
    content_version == canonical_version
}

pub(crate) fn parse_content_mirror_response(
    status: StatusCode,
    body: &[u8],
    manifest: &ContentManifest,
) -> Result<Option<ContentMirrorIndex>, AppError> {
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(AppError::Network(format!(
            "content mirror request failed with HTTP {status}"
        )));
    }
    let mirror: ContentMirrorIndex = serde_json::from_slice(body)
        .map_err(|error| AppError::InvalidData(format!("invalid content mirror: {error}")))?;
    mirror.validate(manifest)?;
    Ok(Some(mirror))
}

pub(crate) fn parse_content_manifest_response(
    status: StatusCode,
    body: &[u8],
) -> Result<Option<ContentManifest>, AppError> {
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(AppError::Network(format!(
            "content manifest request failed with HTTP {status}"
        )));
    }
    let manifest: ContentManifest = serde_json::from_slice(body)
        .map_err(|error| AppError::InvalidData(format!("invalid content manifest: {error}")))?;
    manifest.validate()?;
    Ok(Some(manifest))
}

pub(crate) fn parse_content_pack_manifest_response(
    status: StatusCode,
    body: &[u8],
) -> Result<Option<DrivePackManifest>, AppError> {
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(AppError::Network(format!(
            "content pack manifest request failed with HTTP {status}"
        )));
    }
    let manifest: DrivePackManifest = serde_json::from_slice(body).map_err(|error| {
        AppError::InvalidData(format!("invalid content pack manifest: {error}"))
    })?;
    manifest.validate()?;
    Ok(Some(manifest))
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
    use super::{content_version_matches_canonical, parse_game_manifest_body, ApiClient};
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

    #[test]
    fn release_1_6_11_v2_requires_the_canonical_v1_game_version() {
        assert!(content_version_matches_canonical("1.0.3.4", "1.0.3.4"));
        assert!(!content_version_matches_canonical("1.0.3.4", "1.0.3.16"));
    }

    #[test]
    fn malformed_v1_manifest_body_is_invalid_data_not_a_network_error() {
        let error = parse_game_manifest_body(b"not json")
            .expect_err("malformed manifest body should be rejected");

        assert_eq!(error.code(), "invalid-data");
    }
}
