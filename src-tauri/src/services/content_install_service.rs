use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use sysinfo::System;
use tauri::AppHandle;
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::{
    ContentChunk, ContentFile, ContentManifest, ContentMirrorIndex, ProgressEmitter,
    ProgressPayload, ProgressStage,
};
use crate::services::api_client::ApiClient;
use crate::services::config_service;
use crate::services::content_download_service::{
    decode_verified_chunk, download_content_chunk_with_fallback, read_verified_local_chunk,
    verified_compressed_file, DriveCircuitBreaker,
};
use crate::services::content_journal_service::{
    atomic_json, backup_path, cleanup_transaction, content_root, journal_path,
    load_completion_state, recover_interrupted_commit, recover_pending_content,
    remove_empty_obsolete_directories, remove_file_if_exists, staging_path, state_path,
    write_journal, ContentCompletionState, ContentJournal, ContentJournalAction,
    ContentJournalEntry, ContentJournalPhase,
};
use crate::services::disk_service::ensure_disk_space;
use crate::services::verify_hash_service::{
    find_content_hash_mismatches, ContentHashTask, VerifyHashProgress,
};
use crate::utils::path_utils::safe_join;

const INSTALL_SAFETY_RESERVE: u64 = 2 * 1024 * 1024 * 1024;
const DOWNLOAD_CONTROL_WINDOW: Duration = Duration::from_secs(10);
const DOWNLOAD_READY_BACKLOG_LIMIT: u64 = 256 * 1024 * 1024;
const MATERIALIZER_MEMORY_RESERVE: u64 = 1024 * 1024 * 1024;
const MATERIALIZER_MEMORY_PER_WORKER: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) enum ChunkReadiness {
    Pending,
    Ready,
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) struct AdaptiveDownloadController {
    current: usize,
    max: usize,
    trial_baseline: Option<f64>,
    cooldown_windows: u8,
}

impl AdaptiveDownloadController {
    pub(crate) fn new(initial: usize, max: usize) -> Self {
        Self {
            current: initial.clamp(1, max.max(1)),
            max: max.max(1),
            trial_baseline: None,
            cooldown_windows: 0,
        }
    }

    pub(crate) fn current(&self) -> usize {
        self.current
    }

    pub(crate) fn observe_window(
        &mut self,
        throughput: f64,
        had_error: bool,
        throttled: bool,
        ready_backlog: u64,
    ) {
        let previous = self.current;
        if had_error || throttled {
            self.current = (self.current / 2).max(1);
            self.trial_baseline = None;
            self.cooldown_windows = 3;
        } else if self
            .trial_baseline
            .is_some_and(|baseline| throughput < baseline * 0.9)
        {
            self.current = self.current.saturating_sub(1).max(1);
            self.trial_baseline = None;
            self.cooldown_windows = 3;
        } else if self.cooldown_windows > 0 {
            self.cooldown_windows -= 1;
            self.trial_baseline = None;
        } else if self.current < self.max && ready_backlog < DOWNLOAD_READY_BACKLOG_LIMIT {
            self.trial_baseline = Some(throughput);
            self.current += 1;
        } else {
            self.trial_baseline = None;
        }
        if previous != self.current {
            crate::logger::info(&format!(
                "content download concurrency changed from {previous} to {}",
                self.current
            ));
        }
    }
}

#[derive(Debug)]
struct AdaptiveMaterializerController {
    current: usize,
    max: usize,
    trial_baseline: Option<f64>,
    cooldown_windows: u8,
}

impl AdaptiveMaterializerController {
    fn new(initial: usize, max: usize) -> Self {
        Self {
            current: initial.clamp(1, max.max(1)),
            max: max.max(1),
            trial_baseline: None,
            cooldown_windows: 0,
        }
    }

    fn observe(
        &mut self,
        throughput: f64,
        cpu_percent: f32,
        available_memory: u64,
        wait_ratio: f64,
    ) {
        let previous = self.current;
        let trial_regressed = self
            .trial_baseline
            .is_some_and(|baseline| throughput < baseline * 0.9);
        if cpu_percent > 90.0 || available_memory < 512 * 1024 * 1024 || trial_regressed {
            self.current = self.current.saturating_sub(1).max(1);
            self.trial_baseline = None;
            self.cooldown_windows = 3;
        } else if self.cooldown_windows > 0 {
            self.cooldown_windows -= 1;
        } else if self.current < self.max
            && wait_ratio < 0.2
            && cpu_percent < 80.0
            && available_memory > MATERIALIZER_MEMORY_RESERVE
        {
            self.trial_baseline = Some(throughput);
            self.current += 1;
        }
        if previous != self.current {
            crate::logger::info(&format!(
                "content materializer concurrency changed from {previous} to {}",
                self.current
            ));
        }
    }
}

pub(crate) fn materializer_worker_limits(
    logical_cpu_count: usize,
    available_memory: u64,
) -> (usize, usize) {
    let cpu_cap = (logical_cpu_count / 2).clamp(1, 6);
    let memory_cap = (available_memory.saturating_sub(MATERIALIZER_MEMORY_RESERVE)
        / MATERIALIZER_MEMORY_PER_WORKER)
        .clamp(1, 6) as usize;
    let maximum = cpu_cap.min(memory_cap).max(1);
    (2.min(maximum), maximum)
}

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

struct PipelineProgressState {
    started: Instant,
    downloaded: u64,
    materialized: u64,
    last_progress: f64,
}

#[derive(Clone)]
struct PipelineProgress {
    emitter: ProgressEmitter,
    download_total: u64,
    materialize_total: u64,
    state: Arc<Mutex<PipelineProgressState>>,
}

impl PipelineProgress {
    fn new(emitter: ProgressEmitter, download_total: u64, materialize_total: u64) -> Self {
        Self {
            emitter,
            download_total,
            materialize_total,
            state: Arc::new(Mutex::new(PipelineProgressState {
                started: Instant::now(),
                downloaded: 0,
                materialized: 0,
                last_progress: 0.0,
            })),
        }
    }

    fn add_downloaded(&self, bytes: u64) -> Result<(), AppError> {
        let mut state = self
            .state
            .lock()
            .expect("content progress mutex should not be poisoned");
        state.downloaded = state.downloaded.saturating_add(bytes);
        self.emit_locked(&mut state, None)
    }

    fn add_materialized(&self, bytes: u64, current_file: &str) -> Result<(), AppError> {
        let mut state = self
            .state
            .lock()
            .expect("content progress mutex should not be poisoned");
        state.materialized = state.materialized.saturating_add(bytes);
        self.emit_locked(&mut state, Some(current_file.to_string()))
    }

    fn emit_locked(
        &self,
        state: &mut PipelineProgressState,
        current_file: Option<String>,
    ) -> Result<(), AppError> {
        let work_total = self
            .download_total
            .saturating_add(self.materialize_total)
            .max(1);
        let completed = state.downloaded.saturating_add(state.materialized);
        let calculated = completed as f64 / work_total as f64 * 100.0;
        state.last_progress = state.last_progress.max(calculated).min(99.9);
        let elapsed = state.started.elapsed().as_secs_f64().max(0.001);
        let network_speed = state.downloaded as f64 / elapsed;
        let materialize_speed = state.materialized as f64 / elapsed;
        let network_eta = if network_speed > 0.0 {
            self.download_total.saturating_sub(state.downloaded) as f64 / network_speed
        } else {
            0.0
        };
        let materialize_eta = if materialize_speed > 0.0 {
            self.materialize_total.saturating_sub(state.materialized) as f64 / materialize_speed
        } else {
            0.0
        };
        let mut payload = ProgressPayload::new(
            self.emitter.operation_id().to_string(),
            ProgressStage::Install,
        );
        payload.progress = Some(state.last_progress);
        payload.current_file = current_file;
        payload.downloaded_bytes = Some(state.downloaded);
        payload.total_bytes = Some(self.download_total);
        payload.speed_bytes_per_sec = Some(network_speed);
        payload.time_remaining_sec = Some(network_eta.max(materialize_eta));
        self.emitter.emit(payload)
    }
}

pub fn required_content_install_bytes(
    manifest: &ContentManifest,
    available_raw_chunks: &HashSet<String>,
    staged_bytes: u64,
    replacement_backup_bytes: u64,
    obsolete_backup_bytes: u64,
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
    [
        staged_bytes,
        replacement_backup_bytes,
        obsolete_backup_bytes,
        safety_reserve,
    ]
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
        0,
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
    let obsolete = load_obsolete_content_entries(game_path, manifest).await?;
    total = total
        .checked_add(obsolete_existing_backup_bytes(game_path, &obsolete).await?)
        .ok_or_else(|| AppError::InvalidData("content backup size overflow".into()))?;
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
    let mirror = match api.get_content_drive_mirror(&manifest).await {
        Ok(mirror) => mirror,
        Err(error) => {
            crate::logger::warn(&format!(
                "optional Google Drive content mirror is unavailable; using Oracle ({error})"
            ));
            None
        }
    };
    recover_pending_content(&game_path).await?;
    tokio::fs::create_dir_all(&game_path).await?;

    let progress = ProgressEmitter::new(app, event_name, operation_id);
    progress.emit_stage(ProgressStage::Checking, Some(0.0), None)?;
    let previous = load_previous_manifest(&game_path).await?;
    let obsolete = obsolete_content_entries(previous.as_ref(), &manifest)?;
    let prepared = files_requiring_materialization(
        &game_path,
        &manifest,
        previous.as_ref(),
        &progress,
        &cancel,
    )
    .await?;
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
    let replacement_backup_bytes = prepared.iter().try_fold(0_u64, |total, prepared| {
        total
            .checked_add(prepared.original_size)
            .ok_or_else(|| AppError::InvalidData("content backup size overflow".into()))
    })?;
    let obsolete_backup_bytes = obsolete_existing_backup_bytes(&game_path, &obsolete).await?;
    let required_bytes = required_content_install_bytes(
        &manifest,
        &available,
        staged_bytes,
        replacement_backup_bytes,
        obsolete_backup_bytes,
        INSTALL_SAFETY_RESERVE,
    )?;
    ensure_disk_space(&game_path, required_bytes)?;

    let transaction_id = Uuid::new_v4().to_string();
    let entries = prepared
        .iter()
        .map(|prepared| ContentJournalEntry {
            path: prepared.file.path.clone(),
            action: ContentJournalAction::Replace,
            had_original: prepared.had_original,
        })
        .chain(obsolete)
        .collect::<Vec<_>>();
    let mut journal = ContentJournal {
        schema_version: 2,
        transaction_id: transaction_id.clone(),
        release_id: manifest.release_id.clone(),
        content_sha256: manifest.content_sha256.clone(),
        phase: ContentJournalPhase::Materialize,
        files: entries,
    };
    write_journal(&game_path, &journal).await?;

    let staging = staging_path(&game_path, &transaction_id);
    let pipeline_result = if prepared.is_empty() {
        Ok(())
    } else {
        run_content_pipeline(
            &api,
            prepared.clone(),
            Arc::new(manifest.clone()),
            mirror.map(Arc::new),
            required_raw,
            available,
            Arc::new(local_sources),
            chunk_directory.clone(),
            staging.clone(),
            progress.clone(),
            cancel.clone(),
        )
        .await
    };
    if let Err(error) = pipeline_result {
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
        transaction_id: Some(transaction_id.clone()),
        content_sha256: manifest.content_sha256.clone(),
        release_id: manifest.release_id.clone(),
        game_version: manifest.game_version.clone(),
    };
    if let Err(error) = atomic_json(&state_path(&game_path), &state).await {
        return Err(rollback_failed_install(&game_path, &journal, error).await);
    }

    if let Err(error) = config_service::set_game_version(manifest.game_version).await {
        crate::logger::warn(&format!(
            "committed content but could not save its display version: {error}"
        ));
    }
    if let Err(error) = finalize_committed_transaction(&game_path, &journal).await {
        crate::logger::warn(&format!(
            "committed content but could not clean its transaction; recovery will retry: {error}"
        ));
    }
    if !prepared.is_empty() {
        progress.emit_stage(ProgressStage::Install, Some(100.0), None)?;
    }
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

pub(crate) async fn load_obsolete_content_entries(
    game_path: &Path,
    manifest: &ContentManifest,
) -> Result<Vec<ContentJournalEntry>, AppError> {
    manifest.validate()?;
    let previous = load_previous_manifest(game_path).await?;
    obsolete_content_entries(previous.as_ref(), manifest)
}

fn obsolete_content_entries(
    previous: Option<&ContentManifest>,
    manifest: &ContentManifest,
) -> Result<Vec<ContentJournalEntry>, AppError> {
    let Some(previous) = previous else {
        return Ok(Vec::new());
    };
    previous.validate()?;
    manifest.validate()?;
    let retained = manifest
        .files
        .iter()
        .map(|file| file.path.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    Ok(previous
        .files
        .iter()
        .filter(|file| !retained.contains(&file.path.to_ascii_lowercase()))
        .map(|file| ContentJournalEntry {
            path: file.path.clone(),
            action: ContentJournalAction::Remove,
            had_original: false,
        })
        .collect())
}

async fn obsolete_existing_backup_bytes(
    game_path: &Path,
    entries: &[ContentJournalEntry],
) -> Result<u64, AppError> {
    let mut total = 0_u64;
    for entry in entries
        .iter()
        .filter(|entry| entry.action == ContentJournalAction::Remove)
    {
        let target = safe_join(game_path, &entry.path)?;
        match tokio::fs::symlink_metadata(target).await {
            Ok(metadata) if !metadata.is_dir() => {
                total = total
                    .checked_add(metadata.len())
                    .ok_or_else(|| AppError::InvalidData("content backup size overflow".into()))?;
            }
            Ok(_) => {
                return Err(AppError::InvalidData(format!(
                    "managed obsolete content path is not a file: {}",
                    entry.path
                )))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(total)
}

async fn files_requiring_materialization(
    game_path: &Path,
    manifest: &ContentManifest,
    previous: Option<&ContentManifest>,
    progress: &ProgressEmitter,
    cancel: &CancellationToken,
) -> Result<Vec<PreparedFile>, AppError> {
    enum Disposition {
        Ready,
        Materialize(PreparedFile),
        Hash(PreparedFile),
    }

    let previous_files = previous
        .map(|previous| {
            previous
                .files
                .iter()
                .map(|file| (file.path.to_ascii_lowercase(), file))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let total_files = manifest.files.len().max(1);
    let mut dispositions = Vec::with_capacity(manifest.files.len());
    let mut hash_tasks = Vec::new();
    for (index, file) in manifest.files.iter().enumerate() {
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
        emit_content_check_progress(
            progress,
            ((index + 1) as f64 / total_files as f64) * 5.0,
            None,
            None,
        )?;
        if had_original && (file.excluded_from_hash_check || file.temporary) {
            dispositions.push(Disposition::Ready);
            continue;
        }
        let known_unchanged = previous_files
            .get(&file.path.to_ascii_lowercase())
            .is_some_and(|old| old.size == file.size && old.sha256 == file.sha256);
        if had_original && original_size == file.size && known_unchanged {
            dispositions.push(Disposition::Ready);
            continue;
        }
        let prepared = PreparedFile {
            file: file.clone(),
            had_original,
            original_size,
        };
        if had_original && original_size == file.size {
            hash_tasks.push(ContentHashTask {
                path: file.path.clone(),
                size: file.size,
                expected_sha256: file.sha256.clone(),
                local_path: target,
            });
            dispositions.push(Disposition::Hash(prepared));
        } else {
            dispositions.push(Disposition::Materialize(prepared));
        }
    }

    let hash_progress_emitter = progress.clone();
    let hash_progress = Arc::new(move |update: VerifyHashProgress| {
        let ratio = if update.total_bytes == 0 {
            1.0
        } else {
            update.completed_bytes as f64 / update.total_bytes as f64
        };
        if let Err(error) = emit_content_check_progress(
            &hash_progress_emitter,
            5.0 + ratio.clamp(0.0, 1.0) * 95.0,
            Some(update.speed_bytes_per_sec),
            update.time_remaining_sec,
        ) {
            crate::logger::warn(&format!("failed to emit content scan progress: {error}"));
        }
    });
    let mismatches = find_content_hash_mismatches(hash_tasks, cancel.clone(), hash_progress)
        .await?
        .into_iter()
        .collect::<HashSet<_>>();

    progress.emit_stage(ProgressStage::Checking, Some(100.0), None)?;
    Ok(dispositions
        .into_iter()
        .filter_map(|disposition| match disposition {
            Disposition::Ready => None,
            Disposition::Materialize(prepared) => Some(prepared),
            Disposition::Hash(prepared) if mismatches.contains(&prepared.file.path) => {
                Some(prepared)
            }
            Disposition::Hash(_) => None,
        })
        .collect())
}

fn emit_content_check_progress(
    progress: &ProgressEmitter,
    percentage: f64,
    speed_bytes_per_sec: Option<f64>,
    time_remaining_sec: Option<f64>,
) -> Result<(), AppError> {
    let mut payload =
        ProgressPayload::new(progress.operation_id().to_string(), ProgressStage::Checking);
    payload.progress = Some(percentage.clamp(0.0, 100.0));
    payload.speed_bytes_per_sec = speed_bytes_per_sec;
    payload.time_remaining_sec = time_remaining_sec;
    progress.emit(payload)
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

#[allow(clippy::too_many_arguments)]
async fn run_content_pipeline(
    api: &ApiClient,
    prepared: Vec<PreparedFile>,
    manifest: Arc<ContentManifest>,
    mirror: Option<Arc<ContentMirrorIndex>>,
    required_raw: HashSet<String>,
    available: HashSet<String>,
    local_sources: Arc<HashMap<String, LocalChunkSource>>,
    chunk_directory: PathBuf,
    staging: PathBuf,
    emitter: ProgressEmitter,
    cancel: CancellationToken,
) -> Result<(), AppError> {
    let downloads = ordered_missing_chunks(&prepared, &manifest, &available)?;
    let download_total = downloads
        .iter()
        .map(|(_, chunk)| chunk.compressed_size)
        .sum::<u64>();
    let materialize_total = prepared.iter().try_fold(0_u64, |total, prepared| {
        total
            .checked_add(prepared.file.size)
            .ok_or_else(|| AppError::InvalidData("content pipeline size overflow".into()))
    })?;
    emitter.emit_stage(ProgressStage::Install, Some(0.0), None)?;
    let progress = PipelineProgress::new(emitter, download_total, materialize_total);

    let mut readiness = HashMap::new();
    for raw_sha in required_raw {
        let state = if available.contains(&raw_sha) {
            ChunkReadiness::Ready
        } else {
            ChunkReadiness::Pending
        };
        let (sender, _) = watch::channel(state);
        readiness.insert(raw_sha, sender);
    }
    let readiness = Arc::new(readiness);
    let ready_backlog = Arc::new(AtomicU64::new(0));
    let consumed_chunks = Arc::new(Mutex::new(HashSet::new()));
    let pipeline_cancel = cancel.child_token();

    let download_future = download_pipeline(
        api.clone(),
        downloads,
        manifest.clone(),
        mirror,
        chunk_directory.clone(),
        readiness.clone(),
        ready_backlog.clone(),
        progress.clone(),
        pipeline_cancel.clone(),
    );
    let materialize_future = materialize_pipeline(
        prepared,
        manifest,
        local_sources,
        chunk_directory,
        staging,
        readiness,
        ready_backlog,
        consumed_chunks,
        progress,
        pipeline_cancel.clone(),
    );
    let (download_result, materialize_result) = tokio::join!(download_future, materialize_future);
    if cancel.is_cancelled() {
        return Err(AppError::Canceled);
    }
    match (download_result, materialize_result) {
        (Err(error), _) if !matches!(error, AppError::Canceled) => Err(error),
        (_, Err(error)) if !matches!(error, AppError::Canceled) => Err(error),
        (Err(error), _) => Err(error),
        (_, result) => result,
    }
}

fn ordered_missing_chunks(
    prepared: &[PreparedFile],
    manifest: &ContentManifest,
    available: &HashSet<String>,
) -> Result<Vec<(String, ContentChunk)>, AppError> {
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for prepared in prepared {
        for raw_sha in &prepared.file.chunks {
            if available.contains(raw_sha) || !seen.insert(raw_sha.clone()) {
                continue;
            }
            let chunk = manifest
                .chunks
                .get(raw_sha)
                .ok_or_else(|| AppError::InvalidData("content chunk closure changed".into()))?;
            ordered.push((raw_sha.clone(), chunk.clone()));
        }
    }
    Ok(ordered)
}

#[allow(clippy::too_many_arguments)]
async fn download_pipeline(
    api: ApiClient,
    downloads: Vec<(String, ContentChunk)>,
    manifest: Arc<ContentManifest>,
    mirror: Option<Arc<ContentMirrorIndex>>,
    chunk_directory: PathBuf,
    readiness: Arc<HashMap<String, watch::Sender<ChunkReadiness>>>,
    ready_backlog: Arc<AtomicU64>,
    progress: PipelineProgress,
    cancel: CancellationToken,
) -> Result<(), AppError> {
    let (initial, maximum) = mirror.as_ref().map_or(
        (
            manifest.delivery.recommended_concurrency,
            manifest.delivery.recommended_concurrency,
        ),
        |mirror| (mirror.initial_concurrency, mirror.max_concurrency),
    );
    let mut controller = AdaptiveDownloadController::new(initial, maximum);
    let circuit = DriveCircuitBreaker::default();
    let mut pending = VecDeque::from(downloads);
    let mut running = JoinSet::new();
    let mut first_error = None;
    let mut window_started = Instant::now();
    let mut window_bytes = 0_u64;
    let mut window_had_error = false;
    let mut window_throttled = false;

    loop {
        while first_error.is_none() && running.len() < controller.current() {
            let Some((raw_sha, chunk)) = pending.pop_front() else {
                break;
            };
            let drive_url = mirror
                .as_ref()
                .map(|mirror| mirror.chunk_url(&chunk.compressed_sha256))
                .transpose()?;
            let oracle_url = format!(
                "{}/{}.zst",
                manifest.delivery.chunk_base_url, chunk.compressed_sha256
            );
            let target = compressed_chunk_path(&chunk_directory, &chunk);
            let task_api = api.clone();
            let task_circuit = circuit.clone();
            let task_cancel = cancel.clone();
            running.spawn(async move {
                let result = download_content_chunk_with_fallback(
                    task_api.direct_http(),
                    task_api.http(),
                    drive_url.as_deref(),
                    &oracle_url,
                    &target,
                    &chunk,
                    &task_circuit,
                    task_cancel,
                )
                .await;
                (raw_sha, chunk, result)
            });
        }
        if running.is_empty() {
            break;
        }
        let joined = if first_error.is_some() {
            running.join_next().await
        } else {
            tokio::select! {
                _ = cancel.cancelled() => {
                    first_error = Some(AppError::Canceled);
                    pending.clear();
                    running.join_next().await
                }
                joined = running.join_next() => joined,
            }
        };
        let Some(joined) = joined else {
            break;
        };
        let (raw_sha, chunk, result) = joined
            .map_err(|error| AppError::Unknown(format!("content download task failed: {error}")))?;
        match result {
            Ok(report) if first_error.is_none() => {
                ready_backlog.fetch_add(chunk.compressed_size, Ordering::Relaxed);
                readiness
                    .get(&raw_sha)
                    .ok_or_else(|| AppError::InvalidData("missing chunk readiness state".into()))?
                    .send_replace(ChunkReadiness::Ready);
                progress.add_downloaded(report.network_bytes)?;
                window_bytes = window_bytes.saturating_add(report.network_bytes);
                window_had_error |= report.drive_failed;
                window_throttled |= report.drive_throttled;
                if report.drive_throttled {
                    let elapsed = window_started.elapsed().as_secs_f64().max(0.001);
                    controller.observe_window(
                        window_bytes as f64 / elapsed,
                        true,
                        true,
                        ready_backlog.load(Ordering::Relaxed),
                    );
                    window_started = Instant::now();
                    window_bytes = 0;
                    window_had_error = false;
                    window_throttled = false;
                }
            }
            Ok(_) => {}
            Err(error) if first_error.is_none() => {
                if let Some(sender) = readiness.get(&raw_sha) {
                    sender.send_replace(ChunkReadiness::Failed(error.to_string()));
                }
                first_error = Some(error);
                pending.clear();
                cancel.cancel();
            }
            Err(_) => {}
        }
        if window_started.elapsed() >= DOWNLOAD_CONTROL_WINDOW {
            let elapsed = window_started.elapsed().as_secs_f64().max(0.001);
            controller.observe_window(
                window_bytes as f64 / elapsed,
                window_had_error,
                window_throttled,
                ready_backlog.load(Ordering::Relaxed),
            );
            window_started = Instant::now();
            window_bytes = 0;
            window_had_error = false;
            window_throttled = false;
        }
    }
    if cancel.is_cancelled() && first_error.is_none() {
        return Err(AppError::Canceled);
    }
    first_error.map_or(Ok(()), Err)
}

struct MaterializeReport {
    bytes: u64,
    waited: Duration,
}

#[allow(clippy::too_many_arguments)]
async fn materialize_pipeline(
    prepared: Vec<PreparedFile>,
    manifest: Arc<ContentManifest>,
    local_sources: Arc<HashMap<String, LocalChunkSource>>,
    chunk_directory: PathBuf,
    staging: PathBuf,
    readiness: Arc<HashMap<String, watch::Sender<ChunkReadiness>>>,
    ready_backlog: Arc<AtomicU64>,
    consumed_chunks: Arc<Mutex<HashSet<String>>>,
    progress: PipelineProgress,
    cancel: CancellationToken,
) -> Result<(), AppError> {
    let logical_cpus = std::thread::available_parallelism().map_or(1, usize::from);
    let mut system = System::new_all();
    let (initial, maximum) = materializer_worker_limits(logical_cpus, system.available_memory());
    let mut controller = AdaptiveMaterializerController::new(initial, maximum);
    let mut pending = VecDeque::from(prepared);
    let mut running = JoinSet::new();
    let mut first_error = None;
    let mut window_started = Instant::now();
    let mut window_bytes = 0_u64;
    let mut window_waited = Duration::ZERO;

    loop {
        while first_error.is_none() && running.len() < controller.current {
            let Some(prepared) = pending.pop_front() else {
                break;
            };
            let task_manifest = manifest.clone();
            let task_sources = local_sources.clone();
            let task_chunks = chunk_directory.clone();
            let task_staging = staging.clone();
            let task_readiness = readiness.clone();
            let task_backlog = ready_backlog.clone();
            let task_consumed = consumed_chunks.clone();
            let task_progress = progress.clone();
            let task_cancel = cancel.clone();
            running.spawn(async move {
                let path = prepared.file.path.clone();
                let result = materialize_file(
                    &prepared.file,
                    &task_manifest,
                    &task_sources,
                    &task_chunks,
                    &task_staging,
                    &task_readiness,
                    &task_backlog,
                    &task_consumed,
                    &task_progress,
                    task_cancel,
                )
                .await;
                (path, result)
            });
        }
        if running.is_empty() {
            break;
        }
        let joined = if first_error.is_some() {
            running.join_next().await
        } else {
            tokio::select! {
                _ = cancel.cancelled() => {
                    first_error = Some(AppError::Canceled);
                    pending.clear();
                    running.join_next().await
                }
                joined = running.join_next() => joined,
            }
        };
        let Some(joined) = joined else {
            break;
        };
        let (_, result) = joined.map_err(|error| {
            AppError::Unknown(format!("content materializer task failed: {error}"))
        })?;
        match result {
            Ok(report) if first_error.is_none() => {
                window_bytes = window_bytes.saturating_add(report.bytes);
                window_waited = window_waited.saturating_add(report.waited);
            }
            Ok(_) => {}
            Err(error) if first_error.is_none() => {
                first_error = Some(error);
                pending.clear();
                cancel.cancel();
            }
            Err(_) => {}
        }
        if window_started.elapsed() >= DOWNLOAD_CONTROL_WINDOW {
            let elapsed = window_started.elapsed().as_secs_f64().max(0.001);
            system.refresh_cpu_usage();
            system.refresh_memory();
            controller.observe(
                window_bytes as f64 / elapsed,
                system.global_cpu_usage(),
                system.available_memory(),
                (window_waited.as_secs_f64() / elapsed / controller.current as f64).min(1.0),
            );
            window_started = Instant::now();
            window_bytes = 0;
            window_waited = Duration::ZERO;
        }
    }
    if cancel.is_cancelled() && first_error.is_none() {
        return Err(AppError::Canceled);
    }
    first_error.map_or(Ok(()), Err)
}

#[allow(clippy::too_many_arguments)]
async fn materialize_file(
    file: &ContentFile,
    manifest: &ContentManifest,
    local_sources: &HashMap<String, LocalChunkSource>,
    chunk_directory: &Path,
    staging: &Path,
    readiness: &HashMap<String, watch::Sender<ChunkReadiness>>,
    ready_backlog: &AtomicU64,
    consumed_chunks: &Mutex<HashSet<String>>,
    progress: &PipelineProgress,
    cancel: CancellationToken,
) -> Result<MaterializeReport, AppError> {
    let target = safe_join(staging, &file.path)?;
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut output = tokio::fs::File::create(&target).await?;
    let mut hasher = Sha256::new();
    let mut written = 0_u64;
    let mut waited = Duration::ZERO;
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
            waited = waited.saturating_add(
                wait_until_chunk_ready(
                    readiness
                        .get(raw_sha)
                        .ok_or_else(|| {
                            AppError::InvalidData("missing chunk readiness state".into())
                        })?
                        .subscribe(),
                    &cancel,
                )
                .await?,
            );
            let first_consumer = consumed_chunks
                .lock()
                .expect("consumed chunk mutex should not be poisoned")
                .insert(raw_sha.clone());
            if first_consumer {
                ready_backlog.fetch_sub(
                    chunk
                        .compressed_size
                        .min(ready_backlog.load(Ordering::Relaxed)),
                    Ordering::Relaxed,
                );
            }
            let compressed = tokio::fs::read(compressed_chunk_path(chunk_directory, chunk)).await?;
            let expected_raw = raw_sha.clone();
            let owned_chunk = chunk.clone();
            tokio::task::spawn_blocking(move || {
                decode_verified_chunk(&compressed, &expected_raw, &owned_chunk)
            })
            .await
            .map_err(|error| AppError::Unknown(format!("zstd worker failed: {error}")))??
        };
        output.write_all(&raw).await?;
        hasher.update(&raw);
        written = written
            .checked_add(raw.len() as u64)
            .ok_or_else(|| AppError::InvalidData("materialized content size overflow".into()))?;
        progress.add_materialized(raw.len() as u64, &file.path)?;
    }
    output.flush().await?;
    output.sync_all().await?;
    if written != file.size || hex::encode(hasher.finalize()) != file.sha256 {
        return Err(AppError::InvalidData(format!(
            "materialized content file failed verification: {}",
            file.path
        )));
    }
    Ok(MaterializeReport {
        bytes: written,
        waited,
    })
}

pub(crate) async fn wait_until_chunk_ready(
    mut readiness: watch::Receiver<ChunkReadiness>,
    cancel: &CancellationToken,
) -> Result<Duration, AppError> {
    let started = Instant::now();
    loop {
        match readiness.borrow().clone() {
            ChunkReadiness::Ready => return Ok(started.elapsed()),
            ChunkReadiness::Failed(message) => return Err(AppError::Network(message)),
            ChunkReadiness::Pending => {}
        }
        tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::Canceled),
            changed = readiness.changed() => {
                changed.map_err(|_| AppError::Network(
                    "content chunk readiness channel closed".into(),
                ))?;
            }
        }
    }
}

pub(crate) async fn cleanup_obsolete_directories(
    game_path: &Path,
    journal: &ContentJournal,
) -> Result<(), AppError> {
    remove_empty_obsolete_directories(game_path, journal).await
}

async fn finalize_committed_transaction(
    game_path: &Path,
    journal: &ContentJournal,
) -> Result<(), AppError> {
    cleanup_obsolete_directories(game_path, journal).await?;
    cleanup_transaction(game_path, &journal.transaction_id).await?;
    remove_file_if_exists(&journal_path(game_path)).await
}

pub(crate) async fn commit_staged_files(
    game_path: &Path,
    staging: &Path,
    journal: &ContentJournal,
) -> Result<(), AppError> {
    let backup = backup_path(game_path, &journal.transaction_id);
    for entry in &journal.files {
        let target = safe_join(game_path, &entry.path)?;
        let backup_file = safe_join(&backup, &entry.path)?;
        match entry.action {
            ContentJournalAction::Replace => {
                let staged = safe_join(staging, &entry.path)?;
                if entry.had_original {
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
            ContentJournalAction::Remove => match tokio::fs::symlink_metadata(&target).await {
                Ok(metadata) if !metadata.is_dir() => {
                    if let Some(parent) = backup_file.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::rename(&target, &backup_file).await?;
                }
                Ok(_) => {
                    return Err(AppError::InvalidData(format!(
                        "managed obsolete content path is not a file: {}",
                        entry.path
                    )))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            },
        }
    }
    Ok(())
}

fn compressed_chunk_path(directory: &Path, chunk: &ContentChunk) -> PathBuf {
    directory.join(format!("{}.zst", chunk.compressed_sha256))
}
