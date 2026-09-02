use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::{header, Client, Response, StatusCode, Url};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::models::{DrivePackManifest, PackedContentChunk};
use crate::services::content_pack_cache_service::PackCache;
use crate::services::content_pack_controller::{
    AdaptivePackController, AttemptProgress, ControllerSample, PackSource, PressureWindow,
};
use crate::services::content_pack_plan_service::{ByteRange, PackFetchPlan, PackTransferMode};
use crate::utils::hash_utils::sha256_file;

const HEADER_TIMEOUT: Duration = Duration::from_secs(20);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REPLICA_ATTEMPTS: usize = 2;

#[derive(Debug, Clone)]
pub struct VerifiedPackedChunk {
    pub raw_sha256: String,
    pub compressed_sha256: String,
    pub path: PathBuf,
    pub offset: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
}

#[derive(Debug, Clone)]
pub enum PackDownloadEvent {
    ChunkReady(VerifiedPackedChunk),
    UsefulBytes { pack_sha256: String, bytes: u64 },
}

#[derive(Debug, Default)]
pub struct PackDownloadSummary {
    pub network_bytes: u64,
    pub chunks: HashMap<String, VerifiedPackedChunk>,
}

#[derive(Debug, Clone, Copy)]
enum AttemptClass {
    Retryable,
    Permanent,
    Integrity,
    Preempted,
}

#[derive(Debug)]
struct AttemptError {
    error: AppError,
    class: AttemptClass,
    retry_after: Option<Duration>,
    pressure: bool,
    throttled: bool,
    network_bytes: u64,
}

#[derive(Debug)]
struct JobResult {
    pack_sha256: String,
    network_bytes: u64,
    planned_bytes: u64,
    chunks: Vec<VerifiedPackedChunk>,
}

#[derive(Clone)]
struct ActiveAttempt {
    progress: AttemptProgress,
    preempt: CancellationToken,
}

#[derive(Debug, Default)]
struct PressureAccumulator {
    throttled: bool,
    failures: usize,
}

#[allow(clippy::too_many_arguments)]
pub async fn download_pack_fetches(
    client: Client,
    manifest: Arc<DrivePackManifest>,
    plans: Vec<PackFetchPlan>,
    cache: PackCache,
    operation_id: &str,
    cancellation: CancellationToken,
    events: mpsc::Sender<PackDownloadEvent>,
) -> Result<PackDownloadSummary, AppError> {
    manifest.validate()?;
    let operation_id = operation_id.to_string();
    let mut pending = VecDeque::from(plans);
    let total_backlog = pending.iter().try_fold(0_u64, |total, plan| {
        total
            .checked_add(plan_download_bytes(&manifest, plan)?)
            .ok_or_else(|| AppError::InvalidData("pack download backlog overflow".into()))
    })?;
    let remaining_backlog = Arc::new(AtomicU64::new(total_backlog));
    let useful_bytes = Arc::new(AtomicU64::new(0));
    let active_attempts = Arc::new(Mutex::new(BTreeMap::<String, ActiveAttempt>::new()));
    let pressure = Arc::new(Mutex::new(PressureAccumulator::default()));
    let mut controller = AdaptivePackController::new(6);
    let mut ticker = tokio::time::interval(Duration::from_secs(2));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut jobs = JoinSet::new();
    let mut summary = PackDownloadSummary::default();

    loop {
        while jobs.len() < controller.target() && !pending.is_empty() {
            let plan = pending.pop_front().expect("pending plan must exist");
            let client = client.clone();
            let manifest = manifest.clone();
            let cache = cache.clone();
            let operation_id = operation_id.clone();
            let cancellation = cancellation.clone();
            let events = events.clone();
            let active_attempts = active_attempts.clone();
            let pressure = pressure.clone();
            let useful_bytes = useful_bytes.clone();
            jobs.spawn(async move {
                download_pack_job(
                    client,
                    manifest,
                    cache,
                    plan,
                    &operation_id,
                    cancellation,
                    events,
                    active_attempts,
                    pressure,
                    useful_bytes,
                )
                .await
            });
        }

        if jobs.is_empty() && pending.is_empty() {
            break;
        }

        tokio::select! {
            _ = cancellation.cancelled() => {
                pending.clear();
                while jobs.join_next().await.is_some() {}
                return Err(AppError::Canceled);
            }
            _ = ticker.tick() => {
                let attempts = active_attempts
                    .lock()
                    .await
                    .values()
                    .map(|attempt| attempt.progress.clone())
                    .collect::<Vec<_>>();
                let mut current_pressure = pressure.lock().await;
                let sample = ControllerSample {
                    useful_bytes: useful_bytes.load(Ordering::Relaxed),
                    backlog_bytes: remaining_backlog.load(Ordering::Relaxed),
                    pressure: PressureWindow {
                        throttled: current_pressure.throttled,
                        timeout_or_server_errors: current_pressure.failures,
                    },
                    active_attempts: attempts,
                };
                *current_pressure = PressureAccumulator::default();
                drop(current_pressure);
                let decision = controller.observe(Instant::now(), sample);
                if decision.changed {
                    crate::logger::info(&format!(
                        "Google Drive pack concurrency changed to {}",
                        decision.target
                    ));
                }
                if let Some(preempt) = decision.preempt {
                    if let Some(attempt) = active_attempts.lock().await.get(&preempt.pack_sha256) {
                        if attempt.progress.replica_index == preempt.replica_index {
                            attempt.preempt.cancel();
                        }
                    }
                }
            }
            result = jobs.join_next(), if !jobs.is_empty() => {
                let joined = result
                    .ok_or_else(|| AppError::Unknown("pack download task disappeared".into()))?;
                let result = match joined {
                    Ok(Ok(result)) => result,
                    Ok(Err(error)) => {
                        cancellation.cancel();
                        while jobs.join_next().await.is_some() {}
                        return Err(error);
                    }
                    Err(error) => {
                        cancellation.cancel();
                        while jobs.join_next().await.is_some() {}
                        return Err(AppError::Unknown(format!("pack download task failed: {error}")));
                    }
                };
                remaining_backlog.fetch_sub(
                    result.planned_bytes,
                    Ordering::Relaxed,
                );
                summary.network_bytes = summary.network_bytes.saturating_add(result.network_bytes);
                for chunk in result.chunks {
                    summary.chunks.insert(chunk.raw_sha256.clone(), chunk);
                }
                active_attempts.lock().await.remove(&result.pack_sha256);
            }
        }
    }
    Ok(summary)
}

#[allow(clippy::too_many_arguments)]
async fn download_pack_job(
    client: Client,
    manifest: Arc<DrivePackManifest>,
    cache: PackCache,
    plan: PackFetchPlan,
    operation_id: &str,
    cancellation: CancellationToken,
    events: mpsc::Sender<PackDownloadEvent>,
    active_attempts: Arc<Mutex<BTreeMap<String, ActiveAttempt>>>,
    pressure: Arc<Mutex<PressureAccumulator>>,
    useful_bytes: Arc<AtomicU64>,
) -> Result<JobResult, AppError> {
    let _claim = cache.claim(&plan.pack_sha256).await?;
    let planned_bytes = plan_download_bytes(&manifest, &plan)?;
    let pack = manifest
        .packs
        .get(&plan.pack_sha256)
        .ok_or_else(|| AppError::InvalidData("pack plan references a missing pack".into()))?;
    let first_replica = stable_replica_index(operation_id, &plan.pack_sha256);
    let replica_order = (0..pack.replica_file_ids.len())
        .map(|offset| (first_replica + offset) % pack.replica_file_ids.len())
        .collect::<Vec<_>>();
    let mut network_bytes = 0_u64;
    let mut last_error = None;
    let mut disabled_replicas = std::collections::HashSet::new();

    match &plan.mode {
        PackTransferMode::Full => {
            let verified = cache.full_path(&plan.pack_sha256)?;
            if verified_full_pack(&verified, pack.size, &plan.pack_sha256).await? {
                let chunks = full_pack_chunks(&manifest, &plan, &verified)?;
                publish_ready_chunks(&events, &chunks).await?;
                return Ok(JobResult {
                    pack_sha256: plan.pack_sha256,
                    network_bytes,
                    planned_bytes,
                    chunks,
                });
            }
            PackCache::discard(&verified).await?;
            let partial = cache.full_partial_path(&plan.pack_sha256)?;
            for replica_index in replica_order {
                if disabled_replicas.contains(&replica_index) {
                    continue;
                }
                let url = DrivePackManifest::drive_url(&pack.replica_file_ids[replica_index])?;
                for retry in 0..MAX_REPLICA_ATTEMPTS {
                    let preempt = CancellationToken::new();
                    register_attempt(
                        &active_attempts,
                        &plan.pack_sha256,
                        replica_index,
                        partial_length(&partial, pack.size).await?,
                        preempt.clone(),
                    )
                    .await;
                    match download_full_attempt(
                        &client,
                        &url,
                        &partial,
                        pack.size,
                        &plan.pack_sha256,
                        replica_index,
                        &cancellation,
                        &preempt,
                        &active_attempts,
                    )
                    .await
                    {
                        Ok(bytes) => {
                            network_bytes = network_bytes.saturating_add(bytes);
                            PackCache::promote(&partial, &verified).await?;
                            publish_useful_bytes(&events, &useful_bytes, &plan.pack_sha256, bytes)
                                .await?;
                            let chunks = full_pack_chunks(&manifest, &plan, &verified)?;
                            publish_ready_chunks(&events, &chunks).await?;
                            return Ok(JobResult {
                                pack_sha256: plan.pack_sha256,
                                network_bytes,
                                planned_bytes,
                                chunks,
                            });
                        }
                        Err(failure) => {
                            network_bytes = network_bytes.saturating_add(failure.network_bytes);
                            if matches!(&failure.error, AppError::Canceled) {
                                return Err(AppError::Canceled);
                            }
                            record_pressure(&pressure, &failure).await;
                            let retryable = matches!(failure.class, AttemptClass::Retryable);
                            let preempted = matches!(failure.class, AttemptClass::Preempted);
                            if matches!(failure.class, AttemptClass::Integrity) {
                                PackCache::discard(&partial).await?;
                            }
                            if matches!(
                                failure.class,
                                AttemptClass::Permanent | AttemptClass::Integrity
                            ) {
                                disabled_replicas.insert(replica_index);
                            }
                            let retry_after = failure.retry_after;
                            last_error = Some(failure.error);
                            if preempted {
                                break;
                            }
                            if retryable && retry + 1 < MAX_REPLICA_ATTEMPTS {
                                wait_retry(&cancellation, retry_after, retry).await?;
                                continue;
                            }
                            break;
                        }
                    }
                }
            }
        }
        PackTransferMode::Ranges(ranges) => {
            let mut ready = Vec::new();
            for range in ranges {
                let verified = cache.range_path(&plan.pack_sha256, *range)?;
                if !verified_range(&verified, *range, &manifest, &plan.required_chunks).await? {
                    PackCache::discard(&verified).await?;
                    let partial = cache.range_partial_path(&plan.pack_sha256, *range)?;
                    PackCache::discard(&partial).await?;
                    let mut downloaded = false;
                    for &replica_index in &replica_order {
                        if disabled_replicas.contains(&replica_index) {
                            continue;
                        }
                        let url =
                            DrivePackManifest::drive_url(&pack.replica_file_ids[replica_index])?;
                        for retry in 0..MAX_REPLICA_ATTEMPTS {
                            let preempt = CancellationToken::new();
                            register_attempt(
                                &active_attempts,
                                &plan.pack_sha256,
                                replica_index,
                                range.start,
                                preempt.clone(),
                            )
                            .await;
                            match download_range_attempt(
                                &client,
                                &url,
                                &partial,
                                *range,
                                pack.size,
                                &plan.pack_sha256,
                                replica_index,
                                &cancellation,
                                &preempt,
                                &active_attempts,
                            )
                            .await
                            {
                                Ok(bytes) => {
                                    network_bytes = network_bytes.saturating_add(bytes);
                                    if !verified_range(
                                        &partial,
                                        *range,
                                        &manifest,
                                        &plan.required_chunks,
                                    )
                                    .await?
                                    {
                                        PackCache::discard(&partial).await?;
                                        last_error = Some(AppError::InvalidData(
                                            "downloaded pack range failed chunk verification"
                                                .into(),
                                        ));
                                        disabled_replicas.insert(replica_index);
                                        break;
                                    }
                                    PackCache::promote(&partial, &verified).await?;
                                    publish_useful_bytes(
                                        &events,
                                        &useful_bytes,
                                        &plan.pack_sha256,
                                        bytes,
                                    )
                                    .await?;
                                    downloaded = true;
                                    break;
                                }
                                Err(failure) => {
                                    network_bytes =
                                        network_bytes.saturating_add(failure.network_bytes);
                                    if matches!(&failure.error, AppError::Canceled) {
                                        PackCache::discard(&partial).await?;
                                        return Err(AppError::Canceled);
                                    }
                                    record_pressure(&pressure, &failure).await;
                                    let retryable =
                                        matches!(failure.class, AttemptClass::Retryable);
                                    let preempted =
                                        matches!(failure.class, AttemptClass::Preempted);
                                    let retry_after = failure.retry_after;
                                    if matches!(
                                        failure.class,
                                        AttemptClass::Permanent | AttemptClass::Integrity
                                    ) {
                                        disabled_replicas.insert(replica_index);
                                    }
                                    last_error = Some(failure.error);
                                    PackCache::discard(&partial).await?;
                                    if preempted {
                                        break;
                                    }
                                    if retryable && retry + 1 < MAX_REPLICA_ATTEMPTS {
                                        wait_retry(&cancellation, retry_after, retry).await?;
                                        continue;
                                    }
                                    break;
                                }
                            }
                        }
                        if downloaded {
                            break;
                        }
                    }
                    if !downloaded {
                        return Err(last_error.unwrap_or_else(|| {
                            AppError::Network("all Google Drive pack replicas failed".into())
                        }));
                    }
                }
                let chunks = range_chunks(&manifest, &plan, *range, &verified)?;
                publish_ready_chunks(&events, &chunks).await?;
                ready.extend(chunks);
            }
            return Ok(JobResult {
                pack_sha256: plan.pack_sha256,
                network_bytes,
                planned_bytes,
                chunks: ready,
            });
        }
    }

    Err(last_error
        .unwrap_or_else(|| AppError::Network("all Google Drive pack replicas failed".into())))
}

async fn download_full_attempt(
    client: &Client,
    url: &Url,
    partial: &Path,
    pack_size: u64,
    pack_sha256: &str,
    replica_index: usize,
    cancellation: &CancellationToken,
    preempt: &CancellationToken,
    active_attempts: &Mutex<BTreeMap<String, ActiveAttempt>>,
) -> Result<u64, AttemptError> {
    let (offset, mut hasher) = inspect_full_partial(partial, pack_size).await?;
    if offset == pack_size {
        if hex::encode(hasher.finalize()) == pack_sha256 {
            return Ok(0);
        }
        PackCache::discard(partial).await.map_err(fatal)?;
        return Err(integrity("completed pack partial failed SHA-256", 0));
    }
    let mut request = client
        .get(url.clone())
        .header(header::ACCEPT_ENCODING, "identity");
    if offset > 0 {
        request = request.header(header::RANGE, format!("bytes={offset}-"));
    }
    let started = Instant::now();
    let response = send_attempt(request, cancellation, preempt).await?;
    update_header_latency(active_attempts, pack_sha256, started.elapsed()).await;
    validate_common_response(&response, url, pack_sha256)?;
    if offset == 0 {
        validate_full_headers(&response, pack_size)?;
    } else {
        validate_range_headers(
            &response,
            ByteRange {
                start: offset,
                end_inclusive: pack_size - 1,
            },
            pack_size,
        )?;
    }
    stream_response(
        response,
        partial,
        offset,
        pack_size,
        0,
        pack_sha256,
        replica_index,
        cancellation,
        preempt,
        active_attempts,
        &mut hasher,
    )
    .await?;
    if hex::encode(hasher.finalize()) != pack_sha256 {
        return Err(integrity(
            "downloaded Google Drive pack failed SHA-256",
            pack_size.saturating_sub(offset),
        ));
    }
    Ok(pack_size.saturating_sub(offset))
}

#[allow(clippy::too_many_arguments)]
async fn download_range_attempt(
    client: &Client,
    url: &Url,
    partial: &Path,
    range: ByteRange,
    pack_size: u64,
    pack_sha256: &str,
    replica_index: usize,
    cancellation: &CancellationToken,
    preempt: &CancellationToken,
    active_attempts: &Mutex<BTreeMap<String, ActiveAttempt>>,
) -> Result<u64, AttemptError> {
    let request = client
        .get(url.clone())
        .header(header::ACCEPT_ENCODING, "identity")
        .header(
            header::RANGE,
            format!("bytes={}-{}", range.start, range.end_inclusive),
        );
    let started = Instant::now();
    let response = send_attempt(request, cancellation, preempt).await?;
    update_header_latency(active_attempts, pack_sha256, started.elapsed()).await;
    validate_common_response(&response, url, pack_sha256)?;
    validate_range_headers(&response, range, pack_size)?;
    let mut unused_hasher = Sha256::new();
    stream_response(
        response,
        partial,
        0,
        range.len().map_err(fatal)?,
        range.start,
        pack_sha256,
        replica_index,
        cancellation,
        preempt,
        active_attempts,
        &mut unused_hasher,
    )
    .await?;
    range.len().map_err(fatal)
}

async fn send_attempt(
    request: reqwest::RequestBuilder,
    cancellation: &CancellationToken,
    preempt: &CancellationToken,
) -> Result<Response, AttemptError> {
    tokio::select! {
        _ = cancellation.cancelled() => Err(cancelled()),
        _ = preempt.cancelled() => Err(preempted()),
        result = tokio::time::timeout(HEADER_TIMEOUT, request.send()) => match result {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => {
                let pressure = error.is_timeout() || error.is_connect();
                Err(retryable(AppError::from(error), None, pressure, false, 0))
            }
            Err(_) => Err(retryable(
                AppError::Network("Google Drive pack timed out waiting for headers".into()),
                None,
                true,
                false,
                0,
            )),
        }
    }
}

fn validate_common_response(
    response: &Response,
    requested_url: &Url,
    pack_sha256: &str,
) -> Result<(), AttemptError> {
    crate::logger::info(&format!(
        "Google Drive pack {pack_sha256} negotiated {:?}",
        response.version()
    ));
    if response.url() != requested_url || response.status().is_redirection() {
        return Err(permanent("Google Drive pack redirect was rejected"));
    }
    let status = response.status();
    if matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) {
        return Err(permanent(format!(
            "Google Drive pack request failed with HTTP {status}"
        )));
    }
    if matches!(
        status,
        StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
    {
        return Err(retryable(
            AppError::Network(format!(
                "Google Drive pack request failed with HTTP {status}"
            )),
            retry_after(response),
            true,
            status == StatusCode::TOO_MANY_REQUESTS,
            0,
        ));
    }
    if response.headers().contains_key(header::CONTENT_ENCODING) {
        return Err(permanent(
            "Google Drive pack response used unexpected content encoding",
        ));
    }
    Ok(())
}

fn validate_full_headers(response: &Response, expected_size: u64) -> Result<(), AttemptError> {
    if response.status() != StatusCode::OK || content_length(response) != Some(expected_size) {
        return Err(permanent("invalid full Google Drive pack response"));
    }
    Ok(())
}

fn validate_range_headers(
    response: &Response,
    range: ByteRange,
    pack_size: u64,
) -> Result<(), AttemptError> {
    let expected_length = range.len().map_err(fatal)?;
    let expected_content_range = format!(
        "bytes {}-{}/{}",
        range.start, range.end_inclusive, pack_size
    );
    let actual_content_range = response
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok());
    if response.status() != StatusCode::PARTIAL_CONTENT
        || content_length(response) != Some(expected_length)
        || actual_content_range != Some(expected_content_range.as_str())
    {
        return Err(permanent("invalid ranged Google Drive pack response"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn stream_response(
    response: Response,
    partial: &Path,
    offset: u64,
    expected_size: u64,
    reported_base_offset: u64,
    pack_sha256: &str,
    replica_index: usize,
    cancellation: &CancellationToken,
    preempt: &CancellationToken,
    active_attempts: &Mutex<BTreeMap<String, ActiveAttempt>>,
    hasher: &mut Sha256,
) -> Result<(), AttemptError> {
    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).write(true);
    if offset > 0 {
        options.append(true);
    } else {
        options.truncate(true);
    }
    let mut output = options.open(partial).await.map_err(fatal)?;
    let mut written = offset;
    let mut network_bytes = 0_u64;
    let mut stream = response.bytes_stream();
    loop {
        let next = tokio::select! {
            _ = cancellation.cancelled() => return Err(cancelled_with_bytes(network_bytes)),
            _ = preempt.cancelled() => return Err(preempted_with_bytes(network_bytes)),
            next = tokio::time::timeout(IDLE_TIMEOUT, stream.next()) => match next {
                Ok(next) => next,
                Err(_) => return Err(retryable(
                    AppError::Network("Google Drive pack download stalled".into()),
                    None,
                    true,
                    false,
                    network_bytes,
                )),
            }
        };
        let Some(next) = next else { break };
        let bytes =
            next.map_err(|error| retryable(error.into(), None, true, false, network_bytes))?;
        written = written
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| integrity("Google Drive pack response size overflow", network_bytes))?;
        if written > expected_size {
            return Err(integrity(
                "Google Drive pack response exceeded its declared size",
                network_bytes,
            ));
        }
        output.write_all(&bytes).await.map_err(fatal)?;
        hasher.update(&bytes);
        network_bytes = network_bytes.saturating_add(bytes.len() as u64);
        update_attempt_progress(
            active_attempts,
            pack_sha256,
            replica_index,
            reported_base_offset.saturating_add(written),
            bytes.len() as u64,
        )
        .await;
    }
    output.flush().await.map_err(fatal)?;
    output.sync_all().await.map_err(fatal)?;
    if written != expected_size {
        return Err(retryable(
            AppError::Network("Google Drive pack response ended before exact EOF".into()),
            None,
            true,
            false,
            network_bytes,
        ));
    }
    Ok(())
}

async fn inspect_full_partial(
    partial: &Path,
    pack_size: u64,
) -> Result<(u64, Sha256), AttemptError> {
    let length = partial_length(partial, pack_size).await.map_err(fatal)?;
    let mut hasher = Sha256::new();
    if length == 0 {
        return Ok((0, hasher));
    }
    let mut file = tokio::fs::File::open(partial).await.map_err(fatal)?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).await.map_err(fatal)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok((length, hasher))
}

async fn partial_length(path: &Path, maximum: u64) -> Result<u64, AppError> {
    let Some(length) = PackCache::regular_file_size(path).await? else {
        return Ok(0);
    };
    if length > maximum {
        PackCache::discard(path).await?;
        return Ok(0);
    }
    Ok(length)
}

async fn verified_full_pack(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<bool, AppError> {
    if PackCache::regular_file_size(path).await? != Some(expected_size) {
        return Ok(false);
    }
    Ok(sha256_file(path).await? == expected_sha256)
}

async fn verified_range(
    path: &Path,
    range: ByteRange,
    manifest: &DrivePackManifest,
    required_chunks: &[String],
) -> Result<bool, AppError> {
    let expected_size = range.len()?;
    if PackCache::regular_file_size(path).await? != Some(expected_size) {
        return Ok(false);
    }
    let bytes = tokio::fs::read(path).await?;
    for raw_sha in required_chunks {
        let chunk = &manifest.chunks[raw_sha];
        if !range.contains(chunk.offset, chunk.compressed_size) {
            continue;
        }
        let start = usize::try_from(chunk.offset - range.start)
            .map_err(|_| AppError::InvalidData("pack range offset is too large".into()))?;
        let length = usize::try_from(chunk.compressed_size)
            .map_err(|_| AppError::InvalidData("pack chunk is too large".into()))?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| AppError::InvalidData("pack range slice overflow".into()))?;
        if end > bytes.len()
            || hex::encode(Sha256::digest(&bytes[start..end])) != chunk.compressed_sha256
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn full_pack_chunks(
    manifest: &DrivePackManifest,
    plan: &PackFetchPlan,
    path: &Path,
) -> Result<Vec<VerifiedPackedChunk>, AppError> {
    plan.required_chunks
        .iter()
        .map(|raw_sha| packed_chunk_location(raw_sha, &manifest.chunks[raw_sha], path, 0))
        .collect()
}

fn range_chunks(
    manifest: &DrivePackManifest,
    plan: &PackFetchPlan,
    range: ByteRange,
    path: &Path,
) -> Result<Vec<VerifiedPackedChunk>, AppError> {
    plan.required_chunks
        .iter()
        .filter_map(|raw_sha| {
            let chunk = &manifest.chunks[raw_sha];
            range
                .contains(chunk.offset, chunk.compressed_size)
                .then(|| packed_chunk_location(raw_sha, chunk, path, range.start))
        })
        .collect()
}

fn packed_chunk_location(
    raw_sha: &str,
    chunk: &PackedContentChunk,
    path: &Path,
    base_offset: u64,
) -> Result<VerifiedPackedChunk, AppError> {
    Ok(VerifiedPackedChunk {
        raw_sha256: raw_sha.to_string(),
        compressed_sha256: chunk.compressed_sha256.clone(),
        path: path.to_path_buf(),
        offset: chunk
            .offset
            .checked_sub(base_offset)
            .ok_or_else(|| AppError::InvalidData("pack chunk precedes cached range".into()))?,
        compressed_size: chunk.compressed_size,
        uncompressed_size: chunk.uncompressed_size,
    })
}

async fn publish_ready_chunks(
    events: &mpsc::Sender<PackDownloadEvent>,
    chunks: &[VerifiedPackedChunk],
) -> Result<(), AppError> {
    for chunk in chunks {
        events
            .send(PackDownloadEvent::ChunkReady(chunk.clone()))
            .await
            .map_err(|_| AppError::Canceled)?;
    }
    Ok(())
}

async fn publish_useful_bytes(
    events: &mpsc::Sender<PackDownloadEvent>,
    useful_bytes: &AtomicU64,
    pack_sha256: &str,
    bytes: u64,
) -> Result<(), AppError> {
    if bytes == 0 {
        return Ok(());
    }
    useful_bytes.fetch_add(bytes, Ordering::Relaxed);
    events
        .send(PackDownloadEvent::UsefulBytes {
            pack_sha256: pack_sha256.to_string(),
            bytes,
        })
        .await
        .map_err(|_| AppError::Canceled)
}

fn plan_download_bytes(
    manifest: &DrivePackManifest,
    plan: &PackFetchPlan,
) -> Result<u64, AppError> {
    match &plan.mode {
        PackTransferMode::Full => Ok(manifest.packs[&plan.pack_sha256].size),
        PackTransferMode::Ranges(ranges) => ranges.iter().try_fold(0_u64, |total, range| {
            total
                .checked_add(range.len()?)
                .ok_or_else(|| AppError::InvalidData("pack plan size overflow".into()))
        }),
    }
}

fn stable_replica_index(operation_id: &str, pack_sha256: &str) -> usize {
    let mut hasher = Sha256::new();
    hasher.update(operation_id.as_bytes());
    hasher.update(b":");
    hasher.update(pack_sha256.as_bytes());
    usize::from(hasher.finalize()[0]) % 3
}

async fn register_attempt(
    attempts: &Mutex<BTreeMap<String, ActiveAttempt>>,
    pack_sha256: &str,
    replica_index: usize,
    current_offset: u64,
    preempt: CancellationToken,
) {
    attempts.lock().await.insert(
        pack_sha256.to_string(),
        ActiveAttempt {
            progress: AttemptProgress {
                source: PackSource::GoogleDrive,
                pack_sha256: pack_sha256.to_string(),
                replica_index,
                current_offset,
                useful_bytes: 0,
                header_latency: None,
                last_progress_at: Instant::now(),
            },
            preempt,
        },
    );
}

async fn update_header_latency(
    attempts: &Mutex<BTreeMap<String, ActiveAttempt>>,
    pack_sha256: &str,
    latency: Duration,
) {
    if let Some(attempt) = attempts.lock().await.get_mut(pack_sha256) {
        attempt.progress.header_latency = Some(latency);
    }
}

async fn update_attempt_progress(
    attempts: &Mutex<BTreeMap<String, ActiveAttempt>>,
    pack_sha256: &str,
    replica_index: usize,
    current_offset: u64,
    useful: u64,
) {
    if let Some(attempt) = attempts.lock().await.get_mut(pack_sha256) {
        if attempt.progress.replica_index == replica_index {
            attempt.progress.current_offset = current_offset;
            attempt.progress.useful_bytes = attempt.progress.useful_bytes.saturating_add(useful);
            attempt.progress.last_progress_at = Instant::now();
        }
    }
}

async fn record_pressure(pressure: &Mutex<PressureAccumulator>, failure: &AttemptError) {
    if failure.pressure {
        let mut pressure = pressure.lock().await;
        pressure.failures = pressure.failures.saturating_add(1);
        pressure.throttled |= failure.throttled;
    }
}

async fn wait_retry(
    cancellation: &CancellationToken,
    retry_after: Option<Duration>,
    retry: usize,
) -> Result<(), AppError> {
    let delay = retry_after
        .unwrap_or_else(|| Duration::from_secs((retry + 1) as u64))
        .min(Duration::from_secs(30));
    tokio::select! {
        _ = cancellation.cancelled() => Err(AppError::Canceled),
        _ = tokio::time::sleep(delay) => Ok(()),
    }
}

fn content_length(response: &Response) -> Option<u64> {
    response
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

fn retry_after(response: &Response) -> Option<Duration> {
    response
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.min(30)))
}

fn retryable(
    error: AppError,
    retry_after: Option<Duration>,
    pressure: bool,
    throttled: bool,
    network_bytes: u64,
) -> AttemptError {
    AttemptError {
        error,
        class: AttemptClass::Retryable,
        retry_after,
        pressure,
        throttled,
        network_bytes,
    }
}

fn permanent(message: impl Into<String>) -> AttemptError {
    AttemptError {
        error: AppError::Network(message.into()),
        class: AttemptClass::Permanent,
        retry_after: None,
        pressure: false,
        throttled: false,
        network_bytes: 0,
    }
}

fn integrity(message: impl Into<String>, network_bytes: u64) -> AttemptError {
    AttemptError {
        error: AppError::InvalidData(message.into()),
        class: AttemptClass::Integrity,
        retry_after: None,
        pressure: false,
        throttled: false,
        network_bytes,
    }
}

fn fatal(error: impl Into<AppError>) -> AttemptError {
    AttemptError {
        error: error.into(),
        class: AttemptClass::Permanent,
        retry_after: None,
        pressure: false,
        throttled: false,
        network_bytes: 0,
    }
}

fn cancelled() -> AttemptError {
    AttemptError {
        error: AppError::Canceled,
        class: AttemptClass::Permanent,
        retry_after: None,
        pressure: false,
        throttled: false,
        network_bytes: 0,
    }
}

fn cancelled_with_bytes(network_bytes: u64) -> AttemptError {
    let mut error = cancelled();
    error.network_bytes = network_bytes;
    error
}

fn preempted() -> AttemptError {
    AttemptError {
        error: AppError::Network("slow Google Drive pack attempt was rotated".into()),
        class: AttemptClass::Preempted,
        retry_after: None,
        pressure: false,
        throttled: false,
        network_bytes: 0,
    }
}

fn preempted_with_bytes(network_bytes: u64) -> AttemptError {
    let mut error = preempted();
    error.network_bytes = network_bytes;
    error
}
