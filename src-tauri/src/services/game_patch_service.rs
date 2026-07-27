use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::AppError;
use crate::models::{
    GamePatchLayer, GamePatchManifest, GamePatchManifestEntry, ProgressEmitter, ProgressStage,
};
use crate::services::api_client::ApiClient;
use crate::services::config_service;
use crate::services::download_service::download_file;
use crate::utils::hash_utils::sha256_file;
use crate::utils::path_utils::safe_join;
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GamePatchRoots {
    pub game_files: PathBuf,
    pub game_files_pure: PathBuf,
}

impl GamePatchRoots {
    pub fn new(root: PathBuf) -> Self {
        Self {
            game_files: root.join(GamePatchLayer::GameFiles.as_str()),
            game_files_pure: root.join(GamePatchLayer::GameFilesPure.as_str()),
        }
    }

    pub fn for_layer(&self, layer: GamePatchLayer) -> &Path {
        match layer {
            GamePatchLayer::GameFiles => &self.game_files,
            GamePatchLayer::GameFilesPure => &self.game_files_pure,
        }
    }
}

#[derive(Debug)]
pub struct GamePatchSyncPlan {
    pub downloads: Vec<GamePatchManifestEntry>,
    pub removals: Vec<PathBuf>,
}

pub fn validate_manifest(manifest: &GamePatchManifest) -> Result<(), AppError> {
    let mut entries = HashSet::new();
    let mut layers = HashSet::new();

    for file in &manifest.files {
        if file.path.is_empty()
            || file.path.starts_with('/')
            || file.path.contains('\\')
            || file
                .path
                .split('/')
                .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
            || safe_join(Path::new("cache"), &file.path).is_err()
        {
            return Err(AppError::InvalidData(format!(
                "unsafe launcher game patch path: {}",
                file.path
            )));
        }
        if file.sha256.len() != 64
            || !file
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(AppError::InvalidData(format!(
                "invalid launcher game patch sha256: {}",
                file.path
            )));
        }
        if !entries.insert((file.layer, file.path.as_str())) {
            return Err(AppError::InvalidData(format!(
                "duplicate launcher game patch entry: {}/{}",
                file.layer.as_str(),
                file.path
            )));
        }
        layers.insert(file.layer);
    }

    if !layers.contains(&GamePatchLayer::GameFiles)
        || !layers.contains(&GamePatchLayer::GameFilesPure)
    {
        return Err(AppError::InvalidData(
            "launcher game patch manifest must contain both layers".to_string(),
        ));
    }

    Ok(())
}

pub async fn plan_cache_sync(
    roots: &GamePatchRoots,
    manifest: &GamePatchManifest,
) -> Result<GamePatchSyncPlan, AppError> {
    validate_manifest(manifest)?;
    let expected = manifest
        .files
        .iter()
        .map(|file| (file.layer, file.path.as_str()))
        .collect::<HashSet<_>>();
    let mut downloads = Vec::new();

    for file in &manifest.files {
        let local_path = safe_join(roots.for_layer(file.layer), &file.path)?;
        let metadata = tokio::fs::symlink_metadata(&local_path).await.ok();
        let matches = if let Some(metadata) = metadata {
            metadata.is_file()
                && metadata.len() == file.size
                && sha256_file(&local_path).await? == file.sha256
        } else {
            false
        };
        if !matches {
            downloads.push(file.clone());
        }
    }

    let mut removals = Vec::new();
    for layer in [GamePatchLayer::GameFiles, GamePatchLayer::GameFilesPure] {
        let layer_root = roots.for_layer(layer);
        if !layer_root.is_dir() {
            continue;
        }
        for entry in WalkDir::new(layer_root).follow_links(false) {
            let entry = entry.map_err(|error| AppError::FileSystem(error.to_string()))?;
            if !entry.file_type().is_file() && !entry.file_type().is_symlink() {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(layer_root)
                .map_err(|error| AppError::FileSystem(error.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            if !expected.contains(&(layer, relative.as_str())) {
                removals.push(entry.path().to_path_buf());
            }
        }
    }
    removals.sort();

    Ok(GamePatchSyncPlan {
        downloads,
        removals,
    })
}

pub async fn sync_manifest_cache(
    client: &reqwest::Client,
    roots: &GamePatchRoots,
    manifest: &GamePatchManifest,
    cancel: CancellationToken,
) -> Result<usize, AppError> {
    validate_manifest(manifest)?;
    tokio::fs::create_dir_all(&roots.game_files).await?;
    tokio::fs::create_dir_all(&roots.game_files_pure).await?;

    let plan = plan_cache_sync(roots, manifest).await?;
    for path in plan.removals {
        if cancel.is_cancelled() {
            return Err(AppError::Canceled);
        }
        tokio::fs::remove_file(path).await?;
    }

    let download_count = plan.downloads.len();
    for file in plan.downloads {
        if cancel.is_cancelled() {
            return Err(AppError::Canceled);
        }
        let target = safe_join(roots.for_layer(file.layer), &file.path)?;
        if let Ok(metadata) = tokio::fs::symlink_metadata(&target).await {
            if metadata.is_dir() {
                tokio::fs::remove_dir_all(&target).await?;
            } else if metadata.is_file() || metadata.file_type().is_symlink() {
                tokio::fs::remove_file(&target).await?;
            }
        }
        let url = ApiClient::game_patch_download_url(&file);
        let result = download_file(
            client,
            &url,
            &target,
            None,
            cancel.clone(),
            Some(file.size),
            Some(&file.sha256),
        )
        .await?;
        debug_assert_eq!(result.bytes, file.size);
    }

    let remaining = plan_cache_sync(roots, manifest).await?;
    if !remaining.downloads.is_empty() || !remaining.removals.is_empty() {
        return Err(AppError::InvalidData(
            "launcher game patch cache is incomplete after synchronization".to_string(),
        ));
    }

    Ok(download_count)
}

pub fn game_patch_roots() -> Result<GamePatchRoots, AppError> {
    Ok(GamePatchRoots::new(
        config_service::get_config_dir()?.join("game-file-cache"),
    ))
}

pub async fn sync_game_patch_cache(
    app: AppHandle,
    cancel: CancellationToken,
    event_name: &str,
    operation_id: String,
) -> Result<GamePatchRoots, AppError> {
    let progress = ProgressEmitter::new(app, event_name, operation_id);
    progress.emit_stage(ProgressStage::Checking, Some(0.0), None)?;

    let api = ApiClient::new()?;
    let manifest = api.get_game_patch_manifest().await?;
    validate_manifest(&manifest)?;
    let roots = game_patch_roots()?;
    let downloaded = sync_manifest_cache(api.http(), &roots, &manifest, cancel).await?;

    if downloaded > 0 {
        progress.emit_stage(
            ProgressStage::Download,
            Some(100.0),
            Some(format!(
                "synchronized {downloaded} launcher game patch files"
            )),
        )?;
    }
    progress.emit_stage(ProgressStage::Complete, Some(100.0), None)?;
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;

    use super::{plan_cache_sync, validate_manifest, GamePatchRoots};
    use crate::models::GamePatchManifest;

    fn valid_manifest() -> GamePatchManifest {
        serde_json::from_value(json!({
            "files": [
                {
                    "layer": "game_files",
                    "path": "csgo/pak01_dir.vpk",
                    "size": 4,
                    "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                },
                {
                    "layer": "game_files_pure",
                    "path": "!test.txt",
                    "size": 0,
                    "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                }
            ]
        }))
        .expect("fixture should deserialize")
    }

    #[test]
    fn accepts_a_complete_safe_manifest_including_empty_files() {
        validate_manifest(&valid_manifest()).expect("manifest should be valid");
    }

    #[test]
    fn rejects_unknown_layers_during_deserialization() {
        let value = json!({
            "files": [{
                "layer": "attacker_files",
                "path": "payload.bin",
                "size": 4,
                "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            }]
        });

        assert!(serde_json::from_value::<GamePatchManifest>(value).is_err());
    }

    #[test]
    fn rejects_unsafe_paths_bad_hashes_duplicates_and_missing_layers() {
        for unsafe_path in [
            "",
            "../outside.bin",
            "/outside.bin",
            r"C:\outside.bin",
            "a//b",
        ] {
            let mut manifest = valid_manifest();
            manifest.files[0].path = unsafe_path.to_string();
            assert!(
                validate_manifest(&manifest).is_err(),
                "accepted {unsafe_path}"
            );
        }

        let mut bad_hash = valid_manifest();
        bad_hash.files[0].sha256 = "A".repeat(64);
        assert!(validate_manifest(&bad_hash).is_err());

        let mut duplicate = valid_manifest();
        duplicate.files.push(duplicate.files[0].clone());
        assert!(validate_manifest(&duplicate).is_err());

        let mut missing_layer = valid_manifest();
        missing_layer
            .files
            .retain(|file| file.layer.as_str() == "game_files");
        assert!(validate_manifest(&missing_layer).is_err());
    }

    #[tokio::test]
    async fn cache_plan_downloads_missing_and_corrupt_files_and_removes_unknown_files() {
        let directory = tempdir().expect("temporary directory should exist");
        let roots = GamePatchRoots::new(directory.path().join("game-file-cache"));
        fs::create_dir_all(roots.game_files.join("csgo")).expect("regular cache should exist");
        fs::create_dir_all(roots.game_files_pure.join("csgo")).expect("pure cache should exist");
        fs::write(roots.game_files.join("csgo/exact.bin"), b"exact")
            .expect("exact fixture should exist");
        fs::write(roots.game_files.join("csgo/corrupt.bin"), b"xxxxx")
            .expect("corrupt fixture should exist");
        fs::write(roots.game_files_pure.join("csgo/unexpected.bin"), b"remove")
            .expect("unexpected fixture should exist");

        let manifest: GamePatchManifest = serde_json::from_value(json!({
            "files": [
                {
                    "layer": "game_files",
                    "path": "csgo/exact.bin",
                    "size": 5,
                    "sha256": "fa79d4746c21cd960a17b92db8976ddef95a7e20b590721f8e0fa7847a05e486"
                },
                {
                    "layer": "game_files",
                    "path": "csgo/corrupt.bin",
                    "size": 5,
                    "sha256": "27042f4e6eca7d0b2a7ee4026df2ecfa51d3339e6d122aa099118ecd8563bad9"
                },
                {
                    "layer": "game_files_pure",
                    "path": "csgo/missing.bin",
                    "size": 4,
                    "sha256": "a5e744d0164540d33b1d7ea616c28f2fa97e754a7b92643d86f546d3523ed4af"
                }
            ]
        }))
        .expect("cache manifest should deserialize");

        let plan = plan_cache_sync(&roots, &manifest)
            .await
            .expect("cache plan should succeed");

        assert_eq!(
            plan.downloads
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec!["csgo/corrupt.bin", "csgo/missing.bin"]
        );
        assert_eq!(
            plan.removals,
            vec![roots.game_files_pure.join("csgo/unexpected.bin")]
        );
    }

    #[tokio::test]
    async fn cache_plan_is_empty_when_every_file_matches() {
        let directory = tempdir().expect("temporary directory should exist");
        let roots = GamePatchRoots::new(directory.path().join("game-file-cache"));
        fs::create_dir_all(&roots.game_files).expect("regular cache should exist");
        fs::create_dir_all(&roots.game_files_pure).expect("pure cache should exist");
        fs::write(roots.game_files.join("base.bin"), b"base")
            .expect("regular fixture should exist");
        fs::write(roots.game_files_pure.join("pure.bin"), b"pure")
            .expect("pure fixture should exist");
        let manifest: GamePatchManifest = serde_json::from_value(json!({
            "files": [
                {
                    "layer": "game_files",
                    "path": "base.bin",
                    "size": 4,
                    "sha256": "cae662172fd450bb0cd710a769079c05bfc5d8e35efa6576edc7d0377afdd4a2"
                },
                {
                    "layer": "game_files_pure",
                    "path": "pure.bin",
                    "size": 4,
                    "sha256": "b8ba2ec7e90713c1043778164af3250820943c2165c9f19fa29987e016aae5dd"
                }
            ]
        }))
        .expect("cache manifest should deserialize");

        let plan = plan_cache_sync(&roots, &manifest)
            .await
            .expect("cache plan should succeed");

        assert!(plan.downloads.is_empty());
        assert!(plan.removals.is_empty());
    }
}
