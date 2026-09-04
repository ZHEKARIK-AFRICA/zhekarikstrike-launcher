use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use sysinfo::System;
use tauri::AppHandle;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, watch, RwLock};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::{
    ContentChunk, ContentFile, ContentInventory, DrivePackManifest, ProgressEmitter, ProgressStage,
};
use crate::services::api_client::ApiClient;
use crate::services::config_service;
use crate::services::content_commit_service::{
    queue_success_cleanup, retry_background_cleanup, run_streaming_commit_verified, CommitContext,
    StagingBudget, VerifiedArtifact,
};
use crate::services::content_download_service::{decode_verified_chunk, read_verified_local_chunk};
use crate::services::content_install_service::{
    files_requiring_materialization, load_previous_manifest, local_chunk_candidates,
    materializer_worker_limits, obsolete_existing_backup_bytes, plan_obsolete_content_entries,
    replacement_journal_entries, AdaptiveMaterializerController, ChunkReadiness, IntegrityMode,
    PipelineProgress, PreparedFile,
};
use crate::services::content_journal_service::{
    atomic_json, content_root, recover_pending_content, staging_path, write_journal,
    ContentJournal, ContentJournalPhase, NoContentFsHooks,
};
use crate::services::content_pack_cache_service::PackCache;
use crate::services::content_pack_download_service::{
    download_pack_fetches, PackDownloadEvent, VerifiedPackedChunk,
};
use crate::services::content_pack_metrics::{Activity, PackMetrics, PackRunOptions, Snapshot};
use crate::services::content_pack_plan_service::{
    plan_pack_fetches, PackFetchPlan, PackTransferMode,
};
use crate::services::disk_service::ensure_disk_space;
use crate::utils::path_utils::safe_join;

const INSTALL_SAFETY_RESERVE: u64 = 2 * 1024 * 1024 * 1024;
const MIN_STAGING_BUDGET: u64 = 1024 * 1024 * 1024;
const CONTROL_WINDOW: Duration = Duration::from_secs(10);

pub fn conservative_packed_install_bytes(
    manifest: &DrivePackManifest,
    backup_bytes: u64,
) -> Result<u64, AppError> {
    manifest.validate()?;
    let largest_file = manifest
        .files
        .iter()
        .map(|file| file.size)
        .max()
        .unwrap_or_default();
    [
        backup_bytes,
        largest_file.max(MIN_STAGING_BUDGET),
        INSTALL_SAFETY_RESERVE,
    ]
    .into_iter()
    .try_fold(manifest.download_size, |total, value| {
        total
            .checked_add(value)
            .ok_or_else(|| AppError::InvalidData("content disk requirement overflow".into()))
    })
}

#[derive(Clone)]
struct StableLocalChunk {
    file: Arc<File>,
    offset: u64,
}

struct MaterializeReport {
    bytes: u64,
    waited: Duration,
}

#[allow(clippy::too_many_arguments)]
pub async fn install_or_update_packed_content(
    app: AppHandle,
    game_path: PathBuf,
    api: ApiClient,
    manifest: DrivePackManifest,
    integrity_mode: IntegrityMode,
    cancel: CancellationToken,
    event_name: &str,
    operation_id: String,
) -> Result<(), AppError> {
    manifest.validate()?;
    recover_pending_content(&game_path).await?;
    retry_background_cleanup(&game_path).await?;
    tokio::fs::create_dir_all(&game_path).await?;

    let inventory = ContentInventory::from_v3(&manifest)?;
    let transport_neutral = inventory.as_v2_manifest();
    let progress = ProgressEmitter::new(app, event_name, operation_id.clone());
    progress.emit_stage(ProgressStage::Checking, Some(0.0), None)?;
    let previous = load_previous_manifest(&game_path).await?;
    let obsolete = plan_obsolete_content_entries(
        &game_path,
        previous.as_ref(),
        &transport_neutral,
        &NoContentFsHooks,
    )
    .await?;
    let prepared = files_requiring_materialization(
        &game_path,
        &transport_neutral,
        previous.as_ref(),
        integrity_mode,
        &progress,
        &cancel,
    )
    .await?;

    if prepared.is_empty() && obsolete.is_empty() {
        persist_compatibility_manifest(&game_path, &inventory).await?;
        crate::services::content_inventory_service::save_content_inventory(&game_path, &inventory)
            .await?;
        config_service::set_game_version(inventory.game_version).await?;
        progress.emit_stage(ProgressStage::Complete, Some(100.0), None)?;
        return Ok(());
    }

    let required_order = ordered_required_chunks(&prepared);
    let candidates = local_chunk_candidates(&game_path, &transport_neutral, previous.as_ref())?;
    let mut selected_sources = HashMap::new();
    for raw_sha in &required_order {
        let chunk = transport_neutral
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
                    selected_sources.insert(raw_sha.clone(), source.clone());
                    break;
                }
            }
        }
    }
    let stable_sources = open_stable_sources(selected_sources)?;
    let missing_order = required_order
        .iter()
        .filter(|raw_sha| !stable_sources.contains_key(*raw_sha))
        .cloned()
        .collect::<Vec<_>>();
    let plans = plan_pack_fetches(&manifest, &missing_order)?;
    let planned_download = planned_download_bytes(&manifest, &plans)?;
    let largest_file = prepared
        .iter()
        .map(|prepared| prepared.file.size)
        .max()
        .unwrap_or_default();
    let staging_limit = largest_file.max(MIN_STAGING_BUDGET);
    let replacement_backup = prepared.iter().try_fold(0_u64, |total, prepared| {
        total
            .checked_add(prepared.original_size)
            .ok_or_else(|| AppError::InvalidData("content backup size overflow".into()))
    })?;
    let obsolete_backup = obsolete_existing_backup_bytes(&game_path, &obsolete).await?;
    let required_disk = [
        replacement_backup,
        obsolete_backup,
        staging_limit,
        INSTALL_SAFETY_RESERVE,
    ]
    .into_iter()
    .try_fold(planned_download, |total, value| {
        total
            .checked_add(value)
            .ok_or_else(|| AppError::InvalidData("content disk requirement overflow".into()))
    })?;
    ensure_disk_space(&game_path, required_disk)?;

    let transaction_id = Uuid::new_v4().to_string();
    let cache = PackCache::new(&game_path, &manifest.content_sha256, &transaction_id).await?;
    let resuming = cache.has_resume_data().await?;
    let entries = replacement_journal_entries(&game_path, &prepared, &NoContentFsHooks)
        .await?
        .into_iter()
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
    persist_compatibility_manifest(&game_path, &inventory).await?;
    write_journal(&game_path, &journal).await?;
    journal.phase = ContentJournalPhase::StreamingCommit;
    write_journal(&game_path, &journal).await?;

    progress.emit_stage(ProgressStage::Install, Some(0.0), None)?;
    let materialize_total = prepared.iter().try_fold(0_u64, |total, prepared| {
        total
            .checked_add(prepared.file.size)
            .ok_or_else(|| AppError::InvalidData("content materialization size overflow".into()))
    })?;
    let pipeline_progress =
        PipelineProgress::new(progress.clone(), planned_download, materialize_total)
            .with_status_message(resuming.then(|| "resume".to_string()));
    let staging_budget = StagingBudget::new(staging_limit)?;
    let pipeline_cancel = cancel.child_token();
    let result = run_packed_pipeline(
        &api,
        Arc::new(manifest.clone()),
        plans,
        prepared,
        stable_sources,
        cache,
        staging_path(&game_path, &transaction_id),
        CommitContext {
            game_path: game_path.clone(),
            journal: journal.clone(),
            inventory: inventory.clone(),
            staging_budget: staging_budget.clone(),
            committed: mpsc::channel(1).0,
        },
        staging_budget,
        pipeline_progress,
        pipeline_cancel,
        operation_id.clone(),
        PackRunOptions::new(&operation_id),
    )
    .await;
    let state = match result {
        Ok(state) => state,
        Err(error) => return Err(error),
    };

    if let Err(error) = config_service::set_game_version(state.game_version.clone()).await {
        crate::logger::warn(&format!(
            "committed packed content but could not save its display version: {error}"
        ));
    }
    if let Err(error) =
        queue_success_cleanup(&game_path, &transaction_id, &manifest.content_sha256).await
    {
        crate::logger::warn(&format!(
            "packed content is committed but deferred cleanup could not be queued: {error}"
        ));
    }
    progress.emit_stage(ProgressStage::Install, Some(100.0), None)?;
    progress.emit_stage(ProgressStage::Complete, Some(100.0), None)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_packed_pipeline(
    api: &ApiClient,
    manifest: Arc<DrivePackManifest>,
    plans: Vec<PackFetchPlan>,
    prepared: Vec<PreparedFile>,
    stable_sources: HashMap<String, StableLocalChunk>,
    cache: PackCache,
    staging: PathBuf,
    mut commit_context: CommitContext,
    budget: StagingBudget,
    progress: PipelineProgress,
    cancellation: CancellationToken,
    operation_id: String,
    options: PackRunOptions,
) -> Result<crate::services::content_journal_service::ContentCompletionState, AppError> {
    let metrics = options.metrics.clone();
    metrics.pipeline_started();
    let ready_backlog = Arc::new(AtomicU64::new(0));
    let monitor_backlog = ready_backlog.clone();
    let monitor_budget = budget.clone();
    let final_backlog = ready_backlog.clone();
    let final_budget = budget.clone();
    let monitor_stop = CancellationToken::new();
    let monitor_token = monitor_stop.clone();
    let monitor_metrics = metrics.clone();
    let monitor = tokio::spawn(async move {
        let mut previous = Snapshot::default();
        let mut timer =
            tokio::time::interval_at(tokio::time::Instant::now() + CONTROL_WINDOW, CONTROL_WINDOW);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = monitor_token.cancelled() => break,
                _ = timer.tick() => {
                    monitor_metrics.queues(monitor_backlog.load(Ordering::Relaxed), monitor_budget.available_bytes().await);
                    monitor_metrics.log_sample(&mut previous);
                },
            }
        }
    });
    let mut readiness = HashMap::new();
    for raw_sha in prepared
        .iter()
        .flat_map(|prepared| prepared.file.chunks.iter().cloned())
        .collect::<HashSet<_>>()
    {
        let initial = if stable_sources.contains_key(&raw_sha) {
            ChunkReadiness::Ready
        } else {
            ChunkReadiness::Pending
        };
        readiness.insert(raw_sha, watch::channel(initial).0);
    }
    let readiness = Arc::new(readiness);
    let packed_chunks = Arc::new(RwLock::new(HashMap::new()));
    let consumed_chunks = Arc::new(Mutex::new(HashSet::new()));
    let remaining = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
    for file in &prepared {
        for raw in &file.file.chunks {
            if !stable_sources.contains_key(raw) {
                *remaining
                    .lock()
                    .expect("chunk consumers")
                    .entry(raw.clone())
                    .or_default() += 1;
            }
        }
    }
    let (download_done_tx, download_done_rx) = tokio::sync::oneshot::channel();
    let (pack_events_tx, pack_events_rx) = mpsc::channel(64);
    let (artifacts_tx, artifacts_rx) = mpsc::channel(2);
    let (committed_tx, committed_rx) = mpsc::channel(16);
    commit_context.committed = committed_tx;

    let download_cancel = cancellation.clone();
    // The dispatcher adds verified chunks and materializers subtract their
    // first consumption. Share this queue, not the remaining download plan.
    let download_ready_backlog = ready_backlog.clone();
    let download = async {
        let result = download_pack_fetches(
            api.direct_http().clone(),
            manifest.clone(),
            plans,
            cache,
            &operation_id,
            download_cancel.clone(),
            pack_events_tx,
            download_ready_backlog,
            options,
        )
        .await;
        if result.is_err() {
            download_cancel.cancel();
        }
        let _ = download_done_tx.send(result.is_ok());
        result.map(|_| ())
    };
    let dispatch_cancel = cancellation.clone();
    let dispatch = async {
        let result = dispatch_pack_events(
            pack_events_rx,
            manifest.clone(),
            packed_chunks.clone(),
            readiness.clone(),
            ready_backlog.clone(),
            progress.clone(),
            dispatch_cancel.clone(),
            remaining.clone(),
            metrics.clone(),
        )
        .await;
        if result.is_err() {
            dispatch_cancel.cancel();
        }
        result
    };
    let materialize_cancel = cancellation.clone();
    let materialize = async {
        let result = materialize_packed_files(
            prepared,
            ContentInventory::from_v3(&manifest)?.chunks,
            Arc::new(stable_sources),
            packed_chunks.clone(),
            readiness.clone(),
            ready_backlog.clone(),
            consumed_chunks,
            staging,
            artifacts_tx,
            budget,
            materialize_cancel.clone(),
            metrics.clone(),
            remaining.clone(),
        )
        .await;
        if result.is_err() {
            materialize_cancel.cancel();
        }
        result
    };
    let commit_cancel = cancellation.clone();
    let commit = run_streaming_commit_verified(
        commit_context,
        artifacts_rx,
        commit_cancel,
        Some(download_done_rx),
    );
    let progress_events =
        consume_committed_progress(committed_rx, progress.clone(), metrics.clone());

    let (download_result, dispatch_result, materialize_result, commit_result, progress_result) =
        tokio::join!(download, dispatch, materialize, commit, progress_events);
    monitor_stop.cancel();
    let _ = monitor.await;
    metrics.queues(
        final_backlog.load(Ordering::Relaxed),
        final_budget.available_bytes().await,
    );
    metrics.finish(
        if download_result.is_ok()
            && dispatch_result.is_ok()
            && materialize_result.is_ok()
            && commit_result.is_ok()
            && progress_result.is_ok()
        {
            "complete"
        } else {
            "failed_or_canceled"
        },
    );
    let mut errors = Vec::new();
    if let Err(error) = download_result {
        errors.push(error);
    }
    if let Err(error) = materialize_result {
        errors.push(error);
    }
    if let Err(error) = dispatch_result {
        errors.push(error);
    }
    if let Err(error) = progress_result {
        errors.push(error);
    }
    let state = match commit_result {
        Ok(state) => Some(state),
        Err(error) => {
            errors.push(error);
            None
        }
    };
    if let Some(index) = errors
        .iter()
        .position(|error| !matches!(error, AppError::Canceled))
    {
        return Err(errors.swap_remove(index));
    }
    if let Some(error) = errors.pop() {
        return Err(error);
    }
    state.ok_or_else(|| AppError::Unknown("streaming commit produced no state".into()))
}

async fn dispatch_pack_events(
    mut events: mpsc::Receiver<PackDownloadEvent>,
    manifest: Arc<DrivePackManifest>,
    locations: Arc<RwLock<HashMap<String, VerifiedPackedChunk>>>,
    readiness: Arc<HashMap<String, watch::Sender<ChunkReadiness>>>,
    ready_backlog: Arc<AtomicU64>,
    progress: PipelineProgress,
    cancellation: CancellationToken,
    remaining: Arc<Mutex<HashMap<String, usize>>>,
    metrics: PackMetrics,
) -> Result<(), AppError> {
    let mut published = HashSet::new();
    loop {
        let event = tokio::select! {
            _ = cancellation.cancelled() => return Ok(()),
            event = events.recv() => event,
        };
        let Some(event) = event else { return Ok(()) };
        match event {
            PackDownloadEvent::Forecast { previous, next } => {
                progress.revise_download(previous, next)?
            }
            PackDownloadEvent::UsefulBytes { pack_sha256, bytes } => {
                crate::models::validate_sha256(&pack_sha256, "pack progress")?;
                progress.add_downloaded(bytes)?;
            }
            PackDownloadEvent::ChunkReady(chunk) => {
                let raw_sha = chunk.raw_sha256.clone();
                if !published.insert(raw_sha.clone()) {
                    continue;
                }
                let count = *remaining
                    .lock()
                    .expect("chunk consumers")
                    .get(&raw_sha)
                    .unwrap_or(&0);
                if count == 0 {
                    continue;
                }
                let inserted = locations
                    .write()
                    .await
                    .insert(raw_sha.clone(), chunk)
                    .is_none();
                if inserted {
                    ready_backlog
                        .fetch_add(manifest.chunks[&raw_sha].compressed_size, Ordering::Relaxed);
                    metrics.raw_ready(
                        manifest.chunks[&raw_sha]
                            .uncompressed_size
                            .saturating_mul(count as u64),
                    );
                }
                readiness
                    .get(&raw_sha)
                    .ok_or_else(|| AppError::InvalidData("missing packed chunk readiness".into()))?
                    .send_replace(ChunkReadiness::Ready);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_packed_files(
    prepared: Vec<PreparedFile>,
    chunks: std::collections::BTreeMap<String, ContentChunk>,
    stable_sources: Arc<HashMap<String, StableLocalChunk>>,
    packed_chunks: Arc<RwLock<HashMap<String, VerifiedPackedChunk>>>,
    readiness: Arc<HashMap<String, watch::Sender<ChunkReadiness>>>,
    ready_backlog: Arc<AtomicU64>,
    consumed_chunks: Arc<Mutex<HashSet<String>>>,
    staging: PathBuf,
    artifacts: mpsc::Sender<VerifiedArtifact>,
    budget: StagingBudget,
    cancellation: CancellationToken,
    metrics: PackMetrics,
    remaining: Arc<Mutex<HashMap<String, usize>>>,
) -> Result<(), AppError> {
    let logical_cpus = std::thread::available_parallelism().map_or(1, usize::from);
    let mut system = System::new_all();
    let (initial, maximum) = materializer_worker_limits(logical_cpus, system.available_memory());
    let mut controller = AdaptiveMaterializerController::new(initial, maximum);
    metrics.materializer(
        initial,
        maximum,
        system.global_cpu_usage(),
        system.available_memory(),
    );
    let chunks = Arc::new(chunks);
    let mut pending = VecDeque::from(prepared);
    let mut running = JoinSet::new();
    let mut first_error = None;
    let mut window_started = Instant::now();
    let mut window_bytes = 0_u64;
    let mut window_waited = Duration::ZERO;

    loop {
        while first_error.is_none() && running.len() < controller.current() {
            let Some(prepared) = pending.pop_front() else {
                break;
            };
            let task_chunks = chunks.clone();
            let task_sources = stable_sources.clone();
            let task_packed = packed_chunks.clone();
            let task_readiness = readiness.clone();
            let task_backlog = ready_backlog.clone();
            let task_consumed = consumed_chunks.clone();
            let task_staging = staging.clone();
            let task_artifacts = artifacts.clone();
            let task_budget = budget.clone();
            let task_cancel = cancellation.clone();
            let task_metrics = metrics.clone();
            let task_remaining = remaining.clone();
            running.spawn(async move {
                let _activity = task_metrics.activity(Activity::Materializer);
                let budget_started = Instant::now();
                task_budget
                    .reserve(prepared.file.size, &task_cancel)
                    .await?;
                task_metrics.waited(true, budget_started.elapsed());
                let result = materialize_packed_file(
                    &prepared.file,
                    &task_chunks,
                    &task_sources,
                    &task_packed,
                    &task_readiness,
                    &task_backlog,
                    &task_consumed,
                    &task_staging,
                    task_cancel,
                    &task_metrics,
                    &task_remaining,
                )
                .await;
                match result {
                    Ok((artifact, report)) => {
                        let size = artifact.size;
                        if task_artifacts.send(artifact).await.is_err() {
                            task_budget.release(size).await;
                            return Err(AppError::Canceled);
                        }
                        Ok(report)
                    }
                    Err(error) => {
                        task_budget.release(prepared.file.size).await;
                        Err(error)
                    }
                }
            });
        }
        if running.is_empty() {
            break;
        }
        let joined = tokio::select! {
            _ = cancellation.cancelled(), if first_error.is_none() => {
                first_error = Some(AppError::Canceled);
                pending.clear();
                running.join_next().await
            }
            joined = running.join_next() => joined,
        };
        let Some(joined) = joined else { break };
        match joined.unwrap_or_else(|error| {
            Err(AppError::Unknown(format!(
                "packed materializer task failed: {error}"
            )))
        }) {
            Ok(report) if first_error.is_none() => {
                window_bytes = window_bytes.saturating_add(report.bytes);
                window_waited = window_waited.saturating_add(report.waited);
            }
            Ok(_) => {}
            Err(error) if first_error.is_none() => {
                first_error = Some(error);
                pending.clear();
                cancellation.cancel();
            }
            Err(_) => {}
        }
        if window_started.elapsed() >= CONTROL_WINDOW {
            let elapsed = window_started.elapsed().as_secs_f64().max(0.001);
            system.refresh_cpu_usage();
            system.refresh_memory();
            controller.observe(
                window_bytes as f64 / elapsed,
                system.global_cpu_usage(),
                system.available_memory(),
                (window_waited.as_secs_f64() / elapsed / controller.current() as f64).min(1.0),
            );
            metrics.materializer(
                controller.current(),
                maximum,
                system.global_cpu_usage(),
                system.available_memory(),
            );
            window_started = Instant::now();
            window_bytes = 0;
            window_waited = Duration::ZERO;
        }
    }
    drop(artifacts);
    metrics.phase_finished(false);
    first_error.map_or(Ok(()), Err)
}

#[allow(clippy::too_many_arguments)]
async fn materialize_packed_file(
    file: &ContentFile,
    chunks: &std::collections::BTreeMap<String, ContentChunk>,
    stable_sources: &HashMap<String, StableLocalChunk>,
    packed_chunks: &RwLock<HashMap<String, VerifiedPackedChunk>>,
    readiness: &HashMap<String, watch::Sender<ChunkReadiness>>,
    ready_backlog: &AtomicU64,
    consumed_chunks: &Mutex<HashSet<String>>,
    staging: &Path,
    cancellation: CancellationToken,
    metrics: &PackMetrics,
    remaining: &Mutex<HashMap<String, usize>>,
) -> Result<(VerifiedArtifact, MaterializeReport), AppError> {
    let target = safe_join(staging, &file.path)?;
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .await?;
    let mut hasher = content_pack_core::integrity::StreamSha::new();
    let mut written = 0_u64;
    let mut waited = Duration::ZERO;
    for raw_sha in &file.chunks {
        if cancellation.is_cancelled() {
            return Err(AppError::Canceled);
        }
        let chunk = chunks
            .get(raw_sha)
            .ok_or_else(|| AppError::InvalidData("content chunk closure changed".into()))?;
        let mut work_started = Instant::now();
        let packed = !stable_sources.contains_key(raw_sha);
        let raw = if let Some(source) = stable_sources.get(raw_sha) {
            read_stable_local_chunk(source.clone(), raw_sha.clone(), chunk.clone()).await?
        } else {
            let wait_started = Instant::now();
            waited = waited.saturating_add(
                crate::services::content_install_service::wait_until_chunk_ready(
                    readiness
                        .get(raw_sha)
                        .ok_or_else(|| {
                            AppError::InvalidData("missing packed chunk readiness".into())
                        })?
                        .subscribe(),
                    &cancellation,
                )
                .await?,
            );
            metrics.waited(false, wait_started.elapsed());
            work_started = Instant::now();
            let first_consumer = consumed_chunks
                .lock()
                .expect("packed consumed chunk mutex should not be poisoned")
                .insert(raw_sha.clone());
            if first_consumer {
                ready_backlog.fetch_sub(
                    chunk
                        .compressed_size
                        .min(ready_backlog.load(Ordering::Relaxed)),
                    Ordering::Relaxed,
                );
            }
            let mut locations = packed_chunks.write().await;
            let location = locations.get(raw_sha).cloned().ok_or_else(|| {
                AppError::InvalidData("ready packed chunk has no cache location".into())
            })?;
            let last_consumer = {
                let mut counts = remaining.lock().expect("chunk consumers");
                let count = counts
                    .get_mut(raw_sha)
                    .ok_or_else(|| AppError::InvalidData("missing chunk consumer count".into()))?;
                *count = count
                    .checked_sub(1)
                    .ok_or_else(|| AppError::InvalidData("chunk consumer underflow".into()))?;
                *count == 0
            };
            if last_consumer {
                locations.remove(raw_sha);
            }
            drop(locations);
            read_packed_chunk(location, raw_sha.clone(), chunk.clone()).await?
        };
        output.write_all(&raw).await?;
        hasher.update(&raw);
        metrics.materialized(raw.len() as u64);
        metrics.worked(work_started.elapsed());
        if packed {
            metrics.raw_consumed(raw.len() as u64);
        }
        written = written
            .checked_add(raw.len() as u64)
            .ok_or_else(|| AppError::InvalidData("materialized content size overflow".into()))?;
    }
    output.flush().await?;
    output.sync_all().await?;
    if written != file.size || hasher.finish() != file.sha256 {
        return Err(AppError::InvalidData(format!(
            "materialized packed content file failed verification: {}",
            file.path
        )));
    }
    Ok((
        VerifiedArtifact {
            relative_path: PathBuf::from(file.path.replace('/', std::path::MAIN_SEPARATOR_STR)),
            temporary_path: target,
            size: file.size,
            sha256: file.sha256.clone(),
        },
        MaterializeReport {
            bytes: written,
            waited,
        },
    ))
}

pub(crate) async fn read_packed_chunk(
    location: VerifiedPackedChunk,
    raw_sha: String,
    chunk: ContentChunk,
) -> Result<Vec<u8>, AppError> {
    if location.raw_sha256 != raw_sha
        || location.compressed_sha256 != chunk.compressed_sha256
        || location.compressed_size != chunk.compressed_size
        || location.uncompressed_size != chunk.uncompressed_size
    {
        return Err(AppError::InvalidData(
            "packed content cache location identity changed".into(),
        ));
    }
    #[cfg(test)]
    if location.baseline_read {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let mut input = tokio::fs::File::open(location.source.baseline_path()).await?;
        input
            .seek(std::io::SeekFrom::Start(location.offset))
            .await?;
        let mut compressed = vec![
            0;
            usize::try_from(location.compressed_size).map_err(|_| {
                AppError::InvalidData("pack chunk too large".into())
            })?
        ];
        input.read_exact(&mut compressed).await?;
        return tokio::task::spawn_blocking(move || {
            decode_verified_chunk(&compressed, &raw_sha, &chunk)
        })
        .await
        .map_err(|error| AppError::Unknown(format!("packed zstd worker failed: {error}")))?;
    }
    tokio::task::spawn_blocking(move || {
        let length = usize::try_from(location.compressed_size)
            .map_err(|_| AppError::InvalidData("packed content chunk is too large".into()))?;
        let mut compressed = vec![0_u8; length];
        location
            .source
            .read_exact_at(&mut compressed, location.offset)?;
        decode_verified_chunk(&compressed, &raw_sha, &chunk)
    })
    .await
    .map_err(|error| AppError::Unknown(format!("packed zstd worker failed: {error}")))?
}

async fn read_stable_local_chunk(
    source: StableLocalChunk,
    raw_sha: String,
    chunk: ContentChunk,
) -> Result<Vec<u8>, AppError> {
    tokio::task::spawn_blocking(move || {
        let length = usize::try_from(chunk.uncompressed_size)
            .map_err(|_| AppError::InvalidData("local content chunk is too large".into()))?;
        let mut raw = vec![0_u8; length];
        read_exact_at(&source.file, &mut raw, source.offset)?;
        if hex::encode(Sha256::digest(&raw)) != raw_sha {
            return Err(AppError::InvalidData(
                "stable local content chunk changed".into(),
            ));
        }
        Ok(raw)
    })
    .await
    .map_err(|error| AppError::Unknown(format!("local content reuse worker failed: {error}")))?
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut buffer: &mut [u8], mut offset: u64) -> Result<(), AppError> {
    use std::os::windows::fs::FileExt;
    while !buffer.is_empty() {
        let read = file.seek_read(buffer, offset)?;
        if read == 0 {
            return Err(AppError::InvalidData(
                "stable local content source ended early".into(),
            ));
        }
        offset = offset
            .checked_add(read as u64)
            .ok_or_else(|| AppError::InvalidData("local content offset overflow".into()))?;
        buffer = &mut buffer[read..];
    }
    Ok(())
}

#[cfg(unix)]
fn read_exact_at(file: &File, mut buffer: &mut [u8], mut offset: u64) -> Result<(), AppError> {
    use std::os::unix::fs::FileExt;
    while !buffer.is_empty() {
        let read = file.read_at(buffer, offset)?;
        if read == 0 {
            return Err(AppError::InvalidData(
                "stable local content source ended early".into(),
            ));
        }
        offset = offset
            .checked_add(read as u64)
            .ok_or_else(|| AppError::InvalidData("local content offset overflow".into()))?;
        buffer = &mut buffer[read..];
    }
    Ok(())
}

fn open_stable_sources(
    selected: HashMap<String, crate::services::content_install_service::LocalChunkSource>,
) -> Result<HashMap<String, StableLocalChunk>, AppError> {
    let mut files = HashMap::<PathBuf, Arc<File>>::new();
    let mut stable = HashMap::new();
    for (raw_sha, source) in selected {
        let file = if let Some(file) = files.get(&source.path) {
            file.clone()
        } else {
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                const FILE_SHARE_ALL: u32 = 0x0000_0001 | 0x0000_0002 | 0x0000_0004;
                options.share_mode(FILE_SHARE_ALL);
            }
            let file = Arc::new(options.open(&source.path)?);
            files.insert(source.path.clone(), file.clone());
            file
        };
        stable.insert(
            raw_sha,
            StableLocalChunk {
                file,
                offset: source.offset,
            },
        );
    }
    Ok(stable)
}

fn ordered_required_chunks(prepared: &[PreparedFile]) -> Vec<String> {
    let mut seen = HashSet::new();
    prepared
        .iter()
        .flat_map(|prepared| prepared.file.chunks.iter())
        .filter(|raw_sha| seen.insert((*raw_sha).clone()))
        .cloned()
        .collect()
}

fn planned_download_bytes(
    manifest: &DrivePackManifest,
    plans: &[PackFetchPlan],
) -> Result<u64, AppError> {
    plans.iter().try_fold(0_u64, |total, plan| {
        let bytes = match &plan.mode {
            PackTransferMode::Full => manifest.packs[&plan.pack_sha256].size,
            PackTransferMode::Ranges(ranges) => ranges.iter().try_fold(0_u64, |sum, range| {
                sum.checked_add(range.len()?)
                    .ok_or_else(|| AppError::InvalidData("pack range size overflow".into()))
            })?,
        };
        total
            .checked_add(bytes)
            .ok_or_else(|| AppError::InvalidData("pack download size overflow".into()))
    })
}

async fn persist_compatibility_manifest(
    game_path: &Path,
    inventory: &ContentInventory,
) -> Result<(), AppError> {
    let path = content_root(game_path)
        .join("manifests")
        .join(format!("{}.json", inventory.content_sha256));
    atomic_json(&path, &inventory.as_v2_manifest()).await
}

async fn consume_committed_progress(
    mut committed: mpsc::Receiver<u64>,
    progress: PipelineProgress,
    metrics: PackMetrics,
) -> Result<(), AppError> {
    while let Some(bytes) = committed.recv().await {
        metrics.committed(bytes);
        if let Err(error) = progress.add_materialized(bytes, "") {
            crate::logger::warn(&format!("could not emit packed commit progress: {error}"));
        }
    }
    Ok(())
}

/// Test-only entry below launcher lifecycle/config mutation. The caller owns a
/// disposable marked directory; the full server manifest remains unchanged.
#[cfg(test)]
pub(crate) async fn run_packed_probe(
    api: &ApiClient,
    manifest: Arc<DrivePackManifest>,
    game_path: PathBuf,
    files: Vec<crate::models::ContentFile>,
    options: PackRunOptions,
    cancellation: CancellationToken,
) -> Result<(), AppError> {
    run_packed_probe_scenario(
        api,
        manifest,
        game_path,
        files,
        options,
        cancellation,
        "461ea0b9-971f-4b65-bdea-0a7495a0b813",
        false,
    )
    .await
}

#[cfg(test)]
pub(crate) async fn run_packed_probe_scenario(
    api: &ApiClient,
    manifest: Arc<DrivePackManifest>,
    game_path: PathBuf,
    files: Vec<crate::models::ContentFile>,
    options: PackRunOptions,
    cancellation: CancellationToken,
    seed: &str,
    repair: bool,
) -> Result<(), AppError> {
    manifest.validate()?;
    if tokio::fs::try_exists(&game_path).await? {
        return Err(AppError::InvalidData(
            "probe requires a new empty game directory".into(),
        ));
    }
    let inventory = ContentInventory::from_v3(&manifest)?;
    let mut prepared = files
        .into_iter()
        .map(|file| PreparedFile {
            file,
            had_original: false,
            original_size: 0,
        })
        .collect::<Vec<_>>();
    let plans = plan_pack_fetches(&manifest, &ordered_required_chunks(&prepared))?;
    let planned = planned_download_bytes(&manifest, &plans)?;
    let materialized = prepared.iter().map(|p| p.file.size).sum::<u64>();
    let staging_limit = prepared
        .iter()
        .map(|p| p.file.size)
        .max()
        .unwrap_or(0)
        .max(MIN_STAGING_BUDGET);
    let disk_required = planned
        .checked_add(materialized)
        .and_then(|n| n.checked_add(staging_limit))
        .and_then(|n| n.checked_add(INSTALL_SAFETY_RESERVE))
        .ok_or_else(|| AppError::InvalidData("probe disk arithmetic overflow".into()))?;
    ensure_disk_space(
        game_path
            .parent()
            .ok_or_else(|| AppError::InvalidData("probe parent missing".into()))?,
        disk_required,
    )?;
    tokio::fs::create_dir(&game_path).await?;
    if repair {
        for item in &mut prepared {
            let target = safe_join(&game_path, &item.file.path)?;
            tokio::fs::create_dir_all(target.parent().unwrap()).await?;
            tokio::fs::write(&target, b"probe-corruption").await?;
            // The same SHA reader used by normal integrity verification; only
            // this explicit subset is examined, the server manifest stays whole.
            let actual = crate::utils::hash_utils::sha256_file(&target).await?;
            if actual == item.file.sha256 {
                return Err(AppError::InvalidData(
                    "repair fixture was not corrupt".into(),
                ));
            }
            item.had_original = true;
            item.original_size = tokio::fs::metadata(&target).await?.len();
        }
        tokio::fs::write(game_path.join("probe-user-preserved.txt"), b"preserve").await?;
    }
    let transaction_id = Uuid::new_v4().to_string();
    let cache = PackCache::new(&game_path, &manifest.content_sha256, &transaction_id).await?;
    let journal = ContentJournal {
        schema_version: 2,
        transaction_id: transaction_id.clone(),
        release_id: manifest.release_id.clone(),
        content_sha256: manifest.content_sha256.clone(),
        phase: ContentJournalPhase::StreamingCommit,
        files: replacement_journal_entries(&game_path, &prepared, &NoContentFsHooks).await?,
    };
    persist_compatibility_manifest(&game_path, &inventory).await?;
    write_journal(&game_path, &journal).await?;
    let budget = StagingBudget::new(staging_limit)?;
    // Same seed in isolated A/B roots selects the same initial Drive replicas.
    let operation = seed.to_string();
    let progress =
        PipelineProgress::new(ProgressEmitter::headless(&operation), planned, materialized);
    run_packed_pipeline(
        api,
        manifest,
        plans,
        prepared,
        HashMap::new(),
        cache,
        staging_path(&game_path, &transaction_id),
        CommitContext {
            game_path,
            journal,
            inventory,
            staging_budget: budget.clone(),
            committed: mpsc::channel(1).0,
        },
        budget,
        progress,
        cancellation,
        operation,
        options,
    )
    .await?;
    Ok(())
}
