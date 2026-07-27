use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::{stream, StreamExt};
use sha2::{Digest, Sha256};
use tauri::AppHandle;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::{
    ContentChunk, ContentFile, ContentManifest, ProgressEmitter, ProgressPayload, ProgressStage,
};
use crate::services::api_client::ApiClient;
use crate::services::config_service;
use crate::services::content_download_service::{
    decode_verified_chunk, download_content_chunk, read_verified_local_chunk,
    verified_compressed_file,
};
use crate::services::content_journal_service::{
    atomic_json, backup_path, cleanup_transaction, content_root, journal_path,
    load_completion_state, recover_interrupted_commit, recover_pending_content,
    remove_directory_if_exists, remove_file_if_exists, staging_path, state_path, write_journal,
    ContentCompletionState, ContentJournal, ContentJournalEntry, ContentJournalPhase,
};
use crate::services::disk_service::ensure_disk_space;
use crate::utils::hash_utils::sha256_file;
use crate::utils::path_utils::safe_join;

const INSTALL_SAFETY_RESERVE: u64 = 2 * 1024 * 1024 * 1024;
const MATERIALIZATION_CONCURRENCY: usize = 2;

#[derive(Clone)]
struct LocalChunkSource {
    path: PathBuf,
    offset: u64,
}

#[derive(Clone)]
struct PreparedFile {
    file: ContentFile,
    had_original: bool,
    original_size: u64,
}

pub fn required_content_install_bytes(
    manifest: &ContentManifest,
    available_raw_chunks: &HashSet<String>,
    staged_bytes: u64,
    backup_bytes: u64,
    safety_reserve: u64,
) -> Result<u64, AppError> {
    let missing_download = manifest
        .chunks
        .iter()
        .filter(|(raw_sha, _)| !available_raw_chunks.contains(*raw_sha))
        .try_fold(0_u64, |total, (_, chunk)| {
            total
                .checked_add(chunk.compressed_size)
                .ok_or_else(|| AppError::InvalidData("content disk requirement overflow".into()))
        })?;
    [staged_bytes, backup_bytes, safety_reserve]
        .into_iter()
        .try_fold(missing_download, |total, value| {
            total
                .checked_add(value)
                .ok_or_else(|| AppError::InvalidData("content disk requirement overflow".into()))
        })
}

pub fn conservative_content_install_bytes(
    manifest: &ContentManifest,
    backup_bytes: u64,
) -> Result<u64, AppError> {
    required_content_install_bytes(
        manifest,
        &HashSet::new(),
        manifest.unpacked_size,
        backup_bytes,
        INSTALL_SAFETY_RESERVE,
    )
}

pub async fn estimate_existing_backup_bytes(
    game_path: &Path,
    manifest: &ContentManifest,
) -> Result<u64, AppError> {
    let mut total = 0_u64;
    for file in &manifest.files {
        let target = safe_join(game_path, &file.path)?;
        if let Ok(metadata) = tokio::fs::metadata(target).await {
            if !metadata.is_file() {
                return Err(AppError::InvalidData(format!(
                    "managed content path is not a file: {}",
                    file.path
                )));
            }
            total = total
                .checked_add(metadata.len())
                .ok_or_else(|| AppError::InvalidData("content backup size overflow".into()))?;
        }
    }
    Ok(total)
}

pub async fn install_or_update_content(
    app: AppHandle,
    game_path: PathBuf,
    api: ApiClient,
    manifest: ContentManifest,
    cancel: CancellationToken,
    event_name: &str,
    operation_id: String,
) -> Result<(), AppError> {
    manifest.validate()?;
    recover_pending_content(&game_path).await?;
    tokio::fs::create_dir_all(&game_path).await?;

    let progress = ProgressEmitter::new(app, event_name, operation_id);
    progress.emit_stage(ProgressStage::Checking, Some(0.0), None)?;
    let previous = load_previous_manifest(&game_path).await?;
    let prepared =
        files_requiring_materialization(&game_path, &manifest, previous.as_ref(), &cancel).await?;
    let required_raw = prepared
        .iter()
        .flat_map(|prepared| prepared.file.chunks.iter().cloned())
        .collect::<HashSet<_>>();
    let chunk_directory = content_root(&game_path).join("chunks");
    tokio::fs::create_dir_all(&chunk_directory).await?;

    let mut available = manifest
        .chunks
        .keys()
        .filter(|raw_sha| !required_raw.contains(*raw_sha))
        .cloned()
        .collect::<HashSet<_>>();
    for raw_sha in &required_raw {
        let chunk = manifest
            .chunks
            .get(raw_sha)
            .ok_or_else(|| AppError::InvalidData("content chunk closure changed".into()))?;
        if verified_compressed_file(&compressed_chunk_path(&chunk_directory, chunk), chunk).await? {
            available.insert(raw_sha.clone());
        }
    }

    let candidates = local_chunk_candidates(&game_path, &manifest, previous.as_ref())?;
    let mut local_sources = HashMap::new();
    for raw_sha in &required_raw {
        if available.contains(raw_sha) {
            continue;
        }
        let chunk = manifest
            .chunks
            .get(raw_sha)
            .ok_or_else(|| AppError::InvalidData("content chunk closure changed".into()))?;
        if let Some(sources) = candidates.get(raw_sha) {
            for source in sources {
                if cancel.is_cancelled() {
                    return Err(AppError::Canceled);
                }
                if read_verified_local_chunk(&source.path, source.offset, raw_sha, chunk)
                    .await?
                    .is_some()
                {
                    local_sources.insert(raw_sha.clone(), source.clone());
                    available.insert(raw_sha.clone());
                    break;
                }
            }
        }
    }

    let staged_bytes = prepared.iter().try_fold(0_u64, |total, prepared| {
        total
            .checked_add(prepared.file.size)
            .ok_or_else(|| AppError::InvalidData("content staging size overflow".into()))
    })?;
    let backup_bytes = prepared.iter().try_fold(0_u64, |total, prepared| {
        total
            .checked_add(prepared.original_size)
            .ok_or_else(|| AppError::InvalidData("content backup size overflow".into()))
    })?;
    let required_bytes = required_content_install_bytes(
        &manifest,
        &available,
        staged_bytes,
        backup_bytes,
        INSTALL_SAFETY_RESERVE,
    )?;
    ensure_disk_space(&game_path, required_bytes)?;

    download_missing_chunks(
        &api,
        &manifest,
        &required_raw,
        &available,
        &chunk_directory,
        &progress,
        cancel.clone(),
    )
    .await?;

    let transaction_id = Uuid::new_v4().to_string();
    let entries = prepared
        .iter()
        .map(|prepared| ContentJournalEntry {
            path: prepared.file.path.clone(),
            had_original: prepared.had_original,
        })
        .collect::<Vec<_>>();
    let mut journal = ContentJournal {
        schema_version: 1,
        transaction_id: transaction_id.clone(),
        release_id: manifest.release_id.clone(),
        content_sha256: manifest.content_sha256.clone(),
        phase: ContentJournalPhase::Materialize,
        files: entries,
    };
    write_journal(&game_path, &journal).await?;

    let staging = staging_path(&game_path, &transaction_id);
    let materialize_result = materialize_files(
        prepared.clone(),
        Arc::new(manifest.clone()),
        Arc::new(local_sources),
        chunk_directory.clone(),
        staging.clone(),
        progress.clone(),
        cancel.clone(),
    )
    .await;
    if let Err(error) = materialize_result {
        cleanup_transaction(&game_path, &transaction_id).await.ok();
        remove_file_if_exists(&journal_path(&game_path)).await.ok();
        return Err(error);
    }
    if cancel.is_cancelled() {
        cleanup_transaction(&game_path, &transaction_id).await.ok();
        remove_file_if_exists(&journal_path(&game_path)).await.ok();
        return Err(AppError::Canceled);
    }

    let manifest_path = content_root(&game_path)
        .join("manifests")
        .join(format!("{}.json", manifest.content_sha256));
    atomic_json(&manifest_path, &manifest).await?;
    journal.phase = ContentJournalPhase::Commit;
    write_journal(&game_path, &journal).await?;

    let commit_result = commit_staged_files(&game_path, &staging, &journal).await;
    if let Err(error) = commit_result {
        return Err(rollback_failed_install(&game_path, &journal, error).await);
    }

    let state = ContentCompletionState {
        schema_version: 1,
        content_sha256: manifest.content_sha256.clone(),
        release_id: manifest.release_id.clone(),
        game_version: manifest.game_version.clone(),
    };
    if let Err(error) = atomic_json(&state_path(&game_path), &state).await {
        return Err(rollback_failed_install(&game_path, &journal, error).await);
    }

    config_service::set_game_version(manifest.game_version).await?;
    cleanup_transaction(&game_path, &transaction_id).await.ok();
    remove_directory_if_exists(&chunk_directory).await.ok();
    remove_file_if_exists(&journal_path(&game_path)).await.ok();
    progress.emit_stage(ProgressStage::Complete, Some(100.0), None)?;
    Ok(())
}

async fn rollback_failed_install(
    game_path: &Path,
    journal: &ContentJournal,
    operation_error: AppError,
) -> AppError {
    if let Err(rollback_error) = recover_interrupted_commit(game_path, journal).await {
        return AppError::FileSystem(format!(
            "content install failed ({operation_error}); rollback also failed ({rollback_error}); recovery data was preserved"
        ));
    }
    if let Err(cleanup_error) = cleanup_transaction(game_path, &journal.transaction_id).await {
        return AppError::FileSystem(format!(
            "content install failed ({operation_error}); rollback completed but transaction cleanup failed ({cleanup_error}); recovery journal was preserved"
        ));
    }
    remove_file_if_exists(&journal_path(game_path)).await.ok();
    operation_error
}

async fn load_previous_manifest(game_path: &Path) -> Result<Option<ContentManifest>, AppError> {
    let state = match load_completion_state(game_path).await {
        Ok(Some(state)) if state.schema_version == 1 => state,
        Ok(_) | Err(_) => return Ok(None),
    };
    let path = content_root(game_path)
        .join("manifests")
        .join(format!("{}.json", state.content_sha256));
    let Ok(bytes) = tokio::fs::read(path).await else {
        return Ok(None);
    };
    let Ok(manifest) = serde_json::from_slice::<ContentManifest>(&bytes) else {
        return Ok(None);
    };
    if manifest.validate().is_err()
        || manifest.content_sha256 != state.content_sha256
        || manifest.release_id != state.release_id
        || manifest.game_version != state.game_version
    {
        return Ok(None);
    }
    Ok(Some(manifest))
}

async fn files_requiring_materialization(
    game_path: &Path,
    manifest: &ContentManifest,
    previous: Option<&ContentManifest>,
    cancel: &CancellationToken,
) -> Result<Vec<PreparedFile>, AppError> {
    let previous_files = previous
        .map(|previous| {
            previous
                .files
                .iter()
                .map(|file| (file.path.to_ascii_lowercase(), file))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut prepared = Vec::new();
    for file in &manifest.files {
        if cancel.is_cancelled() {
            return Err(AppError::Canceled);
        }
        let target = safe_join(game_path, &file.path)?;
        let metadata = tokio::fs::metadata(&target).await.ok();
        if metadata
            .as_ref()
            .is_some_and(|metadata| !metadata.is_file())
        {
            return Err(AppError::InvalidData(format!(
                "managed content path is not a file: {}",
                file.path
            )));
        }
        let had_original = metadata.is_some();
        let original_size = metadata.as_ref().map_or(0, |metadata| metadata.len());
        if had_original && (file.excluded_from_hash_check || file.temporary) {
            continue;
        }
        let known_unchanged = previous_files
            .get(&file.path.to_ascii_lowercase())
            .is_some_and(|old| old.size == file.size && old.sha256 == file.sha256);
        if had_original && original_size == file.size && known_unchanged {
            continue;
        }
        if had_original && original_size == file.size && sha256_file(&target).await? == file.sha256
        {
            continue;
        }
        prepared.push(PreparedFile {
            file: file.clone(),
            had_original,
            original_size,
        });
    }
    Ok(prepared)
}

fn local_chunk_candidates(
    game_path: &Path,
    manifest: &ContentManifest,
    previous: Option<&ContentManifest>,
) -> Result<HashMap<String, Vec<LocalChunkSource>>, AppError> {
    let mut candidates: HashMap<String, Vec<LocalChunkSource>> = HashMap::new();
    add_manifest_sources(&mut candidates, game_path, manifest)?;
    if let Some(previous) = previous {
        add_manifest_sources(&mut candidates, game_path, previous)?;
    }
    Ok(candidates)
}

fn add_manifest_sources(
    candidates: &mut HashMap<String, Vec<LocalChunkSource>>,
    game_path: &Path,
    manifest: &ContentManifest,
) -> Result<(), AppError> {
    for file in &manifest.files {
        let path = safe_join(game_path, &file.path)?;
        let mut offset = 0_u64;
        for raw_sha in &file.chunks {
            let chunk = manifest
                .chunks
                .get(raw_sha)
                .ok_or_else(|| AppError::InvalidData("content chunk closure changed".into()))?;
            candidates
                .entry(raw_sha.clone())
                .or_default()
                .push(LocalChunkSource {
                    path: path.clone(),
                    offset,
                });
            offset = offset
                .checked_add(chunk.uncompressed_size)
                .ok_or_else(|| AppError::InvalidData("content source offset overflow".into()))?;
        }
    }
    Ok(())
}

async fn download_missing_chunks(
    api: &ApiClient,
    manifest: &ContentManifest,
    required_raw: &HashSet<String>,
    available: &HashSet<String>,
    chunk_directory: &Path,
    progress: &ProgressEmitter,
    cancel: CancellationToken,
) -> Result<(), AppError> {
    let tasks = required_raw
        .iter()
        .filter(|raw_sha| !available.contains(*raw_sha))
        .map(|raw_sha| {
            let chunk = manifest
                .chunks
                .get(raw_sha)
                .expect("validated manifest contains every chunk")
                .clone();
            (raw_sha.clone(), chunk)
        })
        .collect::<Vec<_>>();
    if tasks.is_empty() {
        return Ok(());
    }
    progress.emit_stage(ProgressStage::Download, Some(0.0), None)?;
    let total = tasks
        .iter()
        .map(|(_, chunk)| chunk.compressed_size)
        .sum::<u64>();
    let batch_cancel = cancel.child_token();
    let client = api.http().clone();
    let base = manifest.delivery.chunk_base_url.clone();
    let concurrency = manifest.delivery.recommended_concurrency;
    let directory = chunk_directory.to_path_buf();
    let mut downloads = stream::iter(tasks.into_iter().map(|(_, chunk)| {
        let client = client.clone();
        let url = format!("{base}/{}.zst", chunk.compressed_sha256);
        let target = compressed_chunk_path(&directory, &chunk);
        let token = batch_cancel.clone();
        async move {
            let size = chunk.compressed_size;
            (
                size,
                download_content_chunk(&client, &url, &target, &chunk, token).await,
            )
        }
    }))
    .buffer_unordered(concurrency);
    let mut completed = 0_u64;
    let mut first_error = None;
    while let Some((size, result)) = downloads.next().await {
        match result {
            Ok(()) if first_error.is_none() => {
                completed = completed.saturating_add(size);
                let mut payload = ProgressPayload::new(
                    progress.operation_id().to_string(),
                    ProgressStage::Download,
                );
                payload.downloaded_bytes = Some(completed);
                payload.total_bytes = Some(total);
                payload.progress = Some(completed as f64 / total.max(1) as f64 * 100.0);
                progress.emit(payload)?;
            }
            Err(error) if first_error.is_none() => {
                batch_cancel.cancel();
                first_error = Some(error);
            }
            _ => {}
        }
    }
    if cancel.is_cancelled() {
        return Err(AppError::Canceled);
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(())
}

async fn materialize_files(
    prepared: Vec<PreparedFile>,
    manifest: Arc<ContentManifest>,
    local_sources: Arc<HashMap<String, LocalChunkSource>>,
    chunk_directory: PathBuf,
    staging: PathBuf,
    progress: ProgressEmitter,
    cancel: CancellationToken,
) -> Result<(), AppError> {
    progress.emit_stage(ProgressStage::Extract, Some(0.0), None)?;
    let total = prepared.len().max(1);
    let batch_cancel = cancel.child_token();
    let mut work = stream::iter(prepared.into_iter().map(|prepared| {
        let manifest = manifest.clone();
        let sources = local_sources.clone();
        let chunks = chunk_directory.clone();
        let staging = staging.clone();
        let token = batch_cancel.clone();
        async move {
            let path = prepared.file.path.clone();
            let result = materialize_file(
                &prepared.file,
                &manifest,
                &sources,
                &chunks,
                &staging,
                token,
            )
            .await;
            (path, result)
        }
    }))
    .buffer_unordered(MATERIALIZATION_CONCURRENCY);
    let mut completed = 0_usize;
    let mut first_error = None;
    while let Some((path, result)) = work.next().await {
        match result {
            Ok(()) if first_error.is_none() => {
                completed += 1;
                progress.emit_stage(
                    ProgressStage::Extract,
                    Some(completed as f64 / total as f64 * 100.0),
                    Some(path),
                )?;
            }
            Err(error) if first_error.is_none() => {
                batch_cancel.cancel();
                first_error = Some(error);
            }
            _ => {}
        }
    }
    if cancel.is_cancelled() {
        return Err(AppError::Canceled);
    }
    first_error.map_or(Ok(()), Err)
}

async fn materialize_file(
    file: &ContentFile,
    manifest: &ContentManifest,
    local_sources: &HashMap<String, LocalChunkSource>,
    chunk_directory: &Path,
    staging: &Path,
    cancel: CancellationToken,
) -> Result<(), AppError> {
    let target = safe_join(staging, &file.path)?;
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut output = tokio::fs::File::create(&target).await?;
    let mut hasher = Sha256::new();
    let mut written = 0_u64;
    for raw_sha in &file.chunks {
        if cancel.is_cancelled() {
            return Err(AppError::Canceled);
        }
        let chunk = manifest
            .chunks
            .get(raw_sha)
            .ok_or_else(|| AppError::InvalidData("content chunk closure changed".into()))?;
        let raw = if let Some(source) = local_sources.get(raw_sha) {
            read_verified_local_chunk(&source.path, source.offset, raw_sha, chunk)
                .await?
                .ok_or_else(|| AppError::InvalidData("reused local content chunk changed".into()))?
        } else {
            let compressed = tokio::fs::read(compressed_chunk_path(chunk_directory, chunk)).await?;
            decode_verified_chunk(&compressed, raw_sha, chunk)?
        };
        output.write_all(&raw).await?;
        hasher.update(&raw);
        written = written
            .checked_add(raw.len() as u64)
            .ok_or_else(|| AppError::InvalidData("materialized content size overflow".into()))?;
    }
    output.flush().await?;
    output.sync_all().await?;
    if written != file.size || hex::encode(hasher.finalize()) != file.sha256 {
        return Err(AppError::InvalidData(format!(
            "materialized content file failed verification: {}",
            file.path
        )));
    }
    Ok(())
}

async fn commit_staged_files(
    game_path: &Path,
    staging: &Path,
    journal: &ContentJournal,
) -> Result<(), AppError> {
    let backup = backup_path(game_path, &journal.transaction_id);
    for entry in &journal.files {
        let target = safe_join(game_path, &entry.path)?;
        let staged = safe_join(staging, &entry.path)?;
        if entry.had_original {
            let backup_file = safe_join(&backup, &entry.path)?;
            if let Some(parent) = backup_file.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::rename(&target, &backup_file).await?;
        }
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::rename(&staged, &target).await?;
    }
    Ok(())
}

fn compressed_chunk_path(directory: &Path, chunk: &ContentChunk) -> PathBuf {
    directory.join(format!("{}.zst", chunk.compressed_sha256))
}
