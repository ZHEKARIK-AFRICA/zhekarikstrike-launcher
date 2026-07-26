use std::env;
use std::io::Read;
use std::path::Path;

use chrono::DateTime;
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use tauri::AppHandle;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::constants::{LAUNCHER_UPDATE_GITHUB_REPOSITORY, LAUNCHER_UPDATE_PUBLIC_KEY};
use crate::error::AppError;
use crate::models::{LauncherUpdateStatus, ProgressEmitter, ProgressStage, VerifiedLauncherUpdate};
use crate::services::api_client::ApiClient;
use crate::services::download_service::download_file;
use crate::utils::hash_utils::sha256_file;

pub async fn check_launcher_update(
    current_version: &str,
) -> Result<LauncherUpdateStatus, AppError> {
    let api = ApiClient::new()?;
    let manifest = api.get_launcher_update(current_version).await?;
    validate_update_manifest(&manifest, current_version, LAUNCHER_UPDATE_PUBLIC_KEY)
}

pub async fn download_launcher_update(app: AppHandle) -> Result<VerifiedLauncherUpdate, AppError> {
    let api = ApiClient::new()?;
    let current_version = env!("CARGO_PKG_VERSION");
    let manifest = api.get_launcher_update(current_version).await?;
    let status = validate_update_manifest(&manifest, current_version, LAUNCHER_UPDATE_PUBLIC_KEY)?;
    if !status.has_update {
        return Err(AppError::InvalidData(
            "launcher update is not newer than the current version".to_string(),
        ));
    }
    let platform = manifest
        .platform("windows-x86_64")
        .cloned()
        .ok_or_else(|| {
            AppError::InvalidData(
                "launcher update manifest is missing windows-x86_64 data".to_string(),
            )
        })?;

    let current = env::current_exe()?;
    let update_directory = current.parent().ok_or_else(|| {
        AppError::FileSystem("launcher executable has no parent directory".to_string())
    })?;
    let verified_path = update_directory.join(format!(
        ".zhekarikstrike-launcher-{}-{}.exe",
        manifest.version,
        Uuid::new_v4()
    ));
    let staging_path = verified_path.with_extension("exe.part");
    let progress =
        ProgressEmitter::new(app, "launcher-update-progress", Uuid::new_v4().to_string());
    let result = async {
        let download = download_file(
            api.http(),
            &platform.url,
            &staging_path,
            Some(progress.clone()),
            CancellationToken::new(),
            None,
            Some(&platform.sha256),
        )
        .await?;

        if download.bytes != platform.size {
            return Err(AppError::InvalidData(format!(
                "launcher update size mismatch: expected {}, received {}",
                platform.size, download.bytes
            )));
        }

        let actual = sha256_file(&download.path).await?;
        if actual != platform.sha256 {
            return Err(AppError::InvalidData(
                "launcher update hash mismatch".to_string(),
            ));
        }

        verify_minisign_signature(&download.path, &platform.signature)?;
        tokio::fs::rename(&download.path, &verified_path).await?;
        progress.emit_stage(ProgressStage::Complete, Some(100.0), None)?;
        Ok::<VerifiedLauncherUpdate, AppError>(VerifiedLauncherUpdate {
            path: verified_path.clone(),
            sha256: platform.sha256.clone(),
            signature: platform.signature.clone(),
            size: platform.size,
        })
    }
    .await;

    if result.is_err() {
        let _ = tokio::fs::remove_file(&staging_path).await;
        let _ = tokio::fs::remove_file(&verified_path).await;
    }
    result
}

pub async fn apply_launcher_update(
    app: AppHandle,
    update: &VerifiedLauncherUpdate,
) -> Result<(), AppError> {
    let current = env::current_exe()?;
    validate_update_artifact(update).await?;

    let old = current.with_extension("old.exe");
    let script_directory = current.parent().ok_or_else(|| {
        AppError::FileSystem("launcher executable has no parent directory".to_string())
    })?;
    let script = script_directory.join(format!(".zhekarik-launcher-update-{}.cmd", Uuid::new_v4()));
    let commands = "@echo off\r\nsetlocal\r\nset \"EXIT_CODE=0\"\r\ntimeout /t 2 /nobreak >NUL\r\nif not exist \"%ZHEKARIK_UPDATE_NEW%\" (set \"EXIT_CODE=10\" & goto cleanup)\r\nif exist \"%ZHEKARIK_UPDATE_OLD%\" del /F /Q \"%ZHEKARIK_UPDATE_OLD%\" >NUL 2>NUL\r\nmove /Y \"%ZHEKARIK_UPDATE_CURRENT%\" \"%ZHEKARIK_UPDATE_OLD%\" >NUL\r\nif errorlevel 1 (set \"EXIT_CODE=20\" & goto cleanup)\r\nmove /Y \"%ZHEKARIK_UPDATE_NEW%\" \"%ZHEKARIK_UPDATE_CURRENT%\" >NUL\r\nif errorlevel 1 (\r\n  if exist \"%ZHEKARIK_UPDATE_OLD%\" move /Y \"%ZHEKARIK_UPDATE_OLD%\" \"%ZHEKARIK_UPDATE_CURRENT%\" >NUL\r\n  set \"EXIT_CODE=30\"\r\n  goto cleanup\r\n)\r\nstart \"\" \"%ZHEKARIK_UPDATE_CURRENT%\"\r\nif errorlevel 1 (\r\n  del /F /Q \"%ZHEKARIK_UPDATE_CURRENT%\" >NUL 2>NUL\r\n  if exist \"%ZHEKARIK_UPDATE_OLD%\" move /Y \"%ZHEKARIK_UPDATE_OLD%\" \"%ZHEKARIK_UPDATE_CURRENT%\" >NUL\r\n  start \"\" \"%ZHEKARIK_UPDATE_CURRENT%\"\r\n  set \"EXIT_CODE=40\"\r\n)\r\n:cleanup\r\ndel /F /Q \"%~f0\" >NUL 2>NUL\r\nexit /b %EXIT_CODE%\r\n";

    tokio::fs::write(&script, commands).await?;
    let script_arg = format!("\"{}\"", script.display());
    Command::new("cmd")
        .args(["/D", "/S", "/C", &script_arg])
        .env("ZHEKARIK_UPDATE_CURRENT", &current)
        .env("ZHEKARIK_UPDATE_NEW", &update.path)
        .env("ZHEKARIK_UPDATE_OLD", &old)
        .spawn()
        .map_err(|error| AppError::Unknown(error.to_string()))?;

    app.exit(0);
    Ok(())
}

async fn validate_update_artifact(update: &VerifiedLauncherUpdate) -> Result<(), AppError> {
    validate_update_artifact_with_key(
        &update.path,
        update.size,
        &update.sha256,
        &update.signature,
        LAUNCHER_UPDATE_PUBLIC_KEY,
    )
    .await
}

async fn validate_update_artifact_with_key(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
    signature: &str,
    public_key: &str,
) -> Result<(), AppError> {
    let metadata = tokio::fs::metadata(path).await.map_err(|error| {
        AppError::FileSystem(format!(
            "downloaded launcher is unavailable at {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() || metadata.len() != expected_size {
        return Err(AppError::InvalidData(
            "downloaded launcher size changed after verification".to_string(),
        ));
    }

    let actual_sha256 = sha256_file(path).await?;
    if actual_sha256 != expected_sha256 {
        return Err(AppError::InvalidData(
            "downloaded launcher hash changed after verification".to_string(),
        ));
    }
    verify_minisign_signature_with_key(path, signature, public_key)
}

fn validate_update_manifest(
    manifest: &crate::models::LauncherUpdateManifest,
    current_version: &str,
    public_key_text: &str,
) -> Result<LauncherUpdateStatus, AppError> {
    let latest = Version::parse(&manifest.version).map_err(|error| {
        AppError::InvalidData(format!("invalid launcher update version: {error}"))
    })?;
    let current = Version::parse(current_version)
        .map_err(|error| AppError::InvalidData(format!("invalid current version: {error}")))?;
    DateTime::parse_from_rfc3339(&manifest.pub_date).map_err(|error| {
        AppError::InvalidData(format!("invalid launcher update publication date: {error}"))
    })?;

    let platform = manifest.platform("windows-x86_64").ok_or_else(|| {
        AppError::InvalidData("launcher update manifest is missing windows-x86_64 data".to_string())
    })?;
    let url = reqwest::Url::parse(&platform.url)
        .map_err(|error| AppError::InvalidData(format!("invalid launcher update url: {error}")))?;
    let path_segments = url
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();
    let (expected_owner, expected_repository) = LAUNCHER_UPDATE_GITHUB_REPOSITORY
        .split_once('/')
        .expect("launcher update GitHub repository constant must be owner/repository");
    let expected_tag = format!("v{}", manifest.version);
    let expected_asset = format!("ZHEKARIK-STRIKE_{}_windows-x86_64.exe", manifest.version);
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || path_segments.len() != 6
        || path_segments[0] != expected_owner
        || path_segments[1] != expected_repository
        || path_segments[2] != "releases"
        || path_segments[3] != "download"
        || path_segments[4] != expected_tag
        || path_segments[5] != expected_asset
    {
        return Err(AppError::InvalidData(
            "launcher update must use a GitHub HTTPS release asset URL".to_string(),
        ));
    }
    if platform.size == 0 {
        return Err(AppError::InvalidData(
            "launcher update size must be greater than zero".to_string(),
        ));
    }
    if platform.sha256.len() != 64
        || !platform
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AppError::InvalidData(
            "launcher update sha256 must be lowercase hexadecimal".to_string(),
        ));
    }
    Signature::decode(&platform.signature).map_err(|error| {
        AppError::InvalidData(format!("invalid launcher update signature: {error}"))
    })?;
    let public_key = extract_public_key_base64(public_key_text)?;
    PublicKey::from_base64(&public_key).map_err(|error| {
        AppError::InvalidData(format!("invalid launcher update public key: {error}"))
    })?;

    let has_update = latest > current;
    Ok(LauncherUpdateStatus {
        has_update,
        can_apply: has_update,
        blocked_reason: None,
        current_version: current_version.to_string(),
        latest_version: manifest.version.clone(),
        download_size: Some(platform.size),
    })
}

fn extract_public_key_base64(public_key_text: &str) -> Result<String, AppError> {
    public_key_text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("untrusted comment:"))
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::InvalidData("launcher update public key is empty".to_string()))
}

fn verify_minisign_signature(path: &Path, signature_text: &str) -> Result<(), AppError> {
    verify_minisign_signature_with_key(path, signature_text, LAUNCHER_UPDATE_PUBLIC_KEY)
}

fn verify_minisign_signature_with_key(
    path: &Path,
    signature_text: &str,
    public_key_text: &str,
) -> Result<(), AppError> {
    let public_key_base64 = extract_public_key_base64(public_key_text)?;
    let public_key = PublicKey::from_base64(&public_key_base64).map_err(|error| {
        AppError::InvalidData(format!("invalid launcher update public key: {error}"))
    })?;
    let signature = Signature::decode(signature_text).map_err(|error| {
        AppError::InvalidData(format!("invalid launcher update signature: {error}"))
    })?;
    let mut verifier = public_key.verify_stream(&signature).map_err(|error| {
        AppError::InvalidData(format!("launcher update signature setup failed: {error}"))
    })?;

    let mut file = std::fs::File::open(path)?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        verifier.update(&buffer[..read]);
    }

    verifier.finalize().map_err(|error| {
        AppError::InvalidData(format!("launcher update signature mismatch: {error}"))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use minisign_verify::PublicKey;
    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        extract_public_key_base64, validate_update_artifact_with_key, validate_update_manifest,
        verify_minisign_signature_with_key,
    };
    use crate::models::LauncherUpdateManifest;

    const TEST_PUBLIC_KEY: &str = "untrusted comment: minisign public key E7620F1842B4E81F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3\n";
    const TEST_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1633700835\tfile:test\tprehashed\nwLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==\n";

    fn manifest(version: &str) -> LauncherUpdateManifest {
        serde_json::from_value(json!({
            "version": version,
            "notes": "release notes",
            "pub_date": "2026-07-26T12:00:00Z",
            "platforms": {
                "windows-x86_64": {
                    "url": format!("https://github.com/d3affy/zhekarikstrike-launcher/releases/download/v{version}/ZHEKARIK-STRIKE_{version}_windows-x86_64.exe"),
                    "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                    "signature": TEST_SIGNATURE,
                    "size": 4
                }
            }
        }))
        .expect("fixture manifest should deserialize")
    }

    #[test]
    fn extracts_base64_from_full_minisign_public_key() {
        assert_eq!(
            extract_public_key_base64(TEST_PUBLIC_KEY).expect("public key should parse"),
            "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3"
        );
    }

    #[test]
    fn embedded_launcher_public_key_is_valid() {
        let encoded = extract_public_key_base64(crate::constants::LAUNCHER_UPDATE_PUBLIC_KEY)
            .expect("embedded public key should have minisign format");
        PublicKey::from_base64(&encoded).expect("embedded public key should decode");
    }

    #[test]
    fn accepts_a_complete_newer_launcher_manifest() {
        let status = validate_update_manifest(&manifest("1.6.1"), "1.6.0", TEST_PUBLIC_KEY)
            .expect("manifest should be valid");

        assert!(status.has_update);
        assert!(status.can_apply);
        assert_eq!(status.download_size, Some(4));
    }

    #[test]
    fn accepts_a_complete_old_launcher_manifest_without_an_update() {
        let status = validate_update_manifest(&manifest("1.5.9"), "1.6.0", TEST_PUBLIC_KEY)
            .expect("old signed manifest is still valid");

        assert!(!status.has_update);
        assert!(!status.can_apply);
    }

    #[test]
    fn rejects_malformed_and_incomplete_launcher_manifests() {
        assert!(
            validate_update_manifest(&manifest("not-semver"), "1.6.0", TEST_PUBLIC_KEY).is_err()
        );

        let incomplete: LauncherUpdateManifest = serde_json::from_value(json!({
            "version": "1.6.1",
            "notes": "",
            "pub_date": "2026-07-26T12:00:00Z",
            "platforms": {
                "windows-x86_64": {
                    "url": "https://github.com/d3affy/zhekarikstrike-launcher/releases/download/v1.6.1/ZHEKARIK-STRIKE_1.6.1_windows-x86_64.exe",
                    "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                    "signature": "",
                    "size": 4
                }
            }
        }))
        .expect("fixture shape should deserialize");

        assert!(validate_update_manifest(&incomplete, "1.6.0", TEST_PUBLIC_KEY).is_err());

        let mut unsafe_url = manifest("1.6.1");
        unsafe_url
            .platforms
            .get_mut("windows-x86_64")
            .expect("platform should exist")
            .url = "https://github.com/example/launcher/raw/main/launcher.exe".to_string();
        assert!(validate_update_manifest(&unsafe_url, "1.6.0", TEST_PUBLIC_KEY).is_err());

        let mut replayed_artifact = manifest("1.6.1");
        replayed_artifact
            .platforms
            .get_mut("windows-x86_64")
            .expect("platform should exist")
            .url = "https://github.com/d3affy/zhekarikstrike-launcher/releases/download/v1.6.0/ZHEKARIK-STRIKE_1.6.0_windows-x86_64.exe".to_string();
        assert!(validate_update_manifest(&replayed_artifact, "1.6.0", TEST_PUBLIC_KEY).is_err());

        assert!(serde_json::from_value::<LauncherUpdateManifest>(json!({
            "version": "1.6.1",
            "notes": "",
            "pub_date": "2026-07-26T12:00:00Z",
            "legacy_url": "https://example.invalid/unsigned.exe",
            "platforms": {}
        }))
        .is_err());
    }

    #[test]
    fn verifies_signature_and_rejects_tampered_artifact() {
        let directory = tempdir().expect("temp directory should be created");
        let artifact = directory.path().join("launcher.exe");
        fs::write(&artifact, b"test").expect("fixture should be written");

        verify_minisign_signature_with_key(&artifact, TEST_SIGNATURE, TEST_PUBLIC_KEY)
            .expect("known signature should verify");

        fs::write(&artifact, b"tampered").expect("fixture should be replaced");
        assert!(
            verify_minisign_signature_with_key(&artifact, TEST_SIGNATURE, TEST_PUBLIC_KEY).is_err()
        );
    }

    #[tokio::test]
    async fn revalidates_the_selected_artifact_before_applying_it() {
        let directory = tempdir().expect("temp directory should be created");
        let artifact = directory.path().join("launcher.exe");
        fs::write(&artifact, b"test").expect("fixture should be written");

        validate_update_artifact_with_key(
            &artifact,
            4,
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            TEST_SIGNATURE,
            TEST_PUBLIC_KEY,
        )
        .await
        .expect("downloaded artifact should still be valid");

        fs::write(&artifact, b"tampered after download").expect("fixture should be replaced");
        assert!(validate_update_artifact_with_key(
            &artifact,
            4,
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            TEST_SIGNATURE,
            TEST_PUBLIC_KEY,
        )
        .await
        .is_err());
    }
}
