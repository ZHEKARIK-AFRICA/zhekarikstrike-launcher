use std::env;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use tauri::AppHandle;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::constants::LAUNCHER_UPDATE_PUBLIC_KEY;
use crate::error::AppError;
use crate::models::{LauncherUpdateStatus, ProgressEmitter, ProgressStage};
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

pub async fn download_launcher_update(app: AppHandle) -> Result<PathBuf, AppError> {
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

    let verified_path = env::temp_dir().join(format!(
        "zhekarikstrike-launcher-{}-{}.exe",
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
        Ok::<PathBuf, AppError>(verified_path.clone())
    }
    .await;

    if result.is_err() {
        let _ = tokio::fs::remove_file(&staging_path).await;
        let _ = tokio::fs::remove_file(&verified_path).await;
    }
    result
}

pub async fn apply_launcher_update(app: AppHandle, new_launcher: &Path) -> Result<(), AppError> {
    let current = env::current_exe()?;

    if !tokio::fs::try_exists(new_launcher).await.unwrap_or(false) {
        return Err(AppError::FileSystem(format!(
            "Downloaded launcher not found: {}",
            new_launcher.display()
        )));
    }

    let old = current.with_extension("old.exe");
    let script = env::temp_dir().join("zhekarik_launcher_update.cmd");
    let commands = format!(
        "@echo off\r\nsetlocal\r\nset \"CURRENT={}\"\r\nset \"NEW={}\"\r\nset \"OLD={}\"\r\ntimeout /t 2 /nobreak >NUL\r\nif not exist \"%NEW%\" exit /b 10\r\nif exist \"%OLD%\" del /F /Q \"%OLD%\" >NUL 2>NUL\r\nmove /Y \"%CURRENT%\" \"%OLD%\" >NUL\r\nif errorlevel 1 exit /b 20\r\nmove /Y \"%NEW%\" \"%CURRENT%\" >NUL\r\nif errorlevel 1 (\r\n  if exist \"%OLD%\" move /Y \"%OLD%\" \"%CURRENT%\" >NUL\r\n  exit /b 30\r\n)\r\nstart \"\" \"%CURRENT%\"\r\nif errorlevel 1 (\r\n  del /F /Q \"%CURRENT%\" >NUL 2>NUL\r\n  if exist \"%OLD%\" move /Y \"%OLD%\" \"%CURRENT%\" >NUL\r\n  start \"\" \"%CURRENT%\"\r\n  exit /b 40\r\n)\r\nexit /b 0\r\n",
        current.display(),
        new_launcher.display(),
        old.display()
    );

    tokio::fs::write(&script, commands).await?;
    let script_arg = script.to_string_lossy().to_string();
    Command::new("cmd")
        .args(["/C", &script_arg])
        .spawn()
        .map_err(|error| AppError::Unknown(error.to_string()))?;

    app.exit(0);
    Ok(())
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
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || path_segments.len() < 6
        || path_segments[2] != "releases"
        || path_segments[3] != "download"
        || path_segments[4].is_empty()
        || path_segments[5].is_empty()
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
        extract_public_key_base64, validate_update_manifest, verify_minisign_signature_with_key,
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
                    "url": "https://github.com/example/launcher/releases/download/v1.6.1/launcher.exe",
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
                    "url": "https://github.com/example/launcher/releases/download/v1.6.1/launcher.exe",
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
}
