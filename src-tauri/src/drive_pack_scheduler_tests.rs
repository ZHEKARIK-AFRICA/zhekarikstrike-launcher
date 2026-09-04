//! Controlled transport, actual production scheduler/tasks/hash verification.
use crate::models::{ContentFile, DrivePack, DrivePackManifest, PackedContentChunk};
use crate::services::content_pack_cache_service::PackCache;
use crate::services::content_pack_download_service::download_pack_fetches;
use crate::services::content_pack_metrics::{PackMetrics, PackRunOptions};
use crate::services::content_pack_plan_service::plan_pack_fetches;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

const MIB: u64 = 1024 * 1024;
type Bodies = Arc<BTreeMap<String, Arc<Vec<u8>>>>;
fn scratch() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("drive-pack-unit-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap()
}
fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn fixture() -> (Arc<DrivePackManifest>, Bodies) {
    let mut manifest = crate::drive_pack_tests::two_chunk_manifest();
    manifest.files.clear();
    manifest.chunks.clear();
    manifest.packs.clear();
    let mut bodies = BTreeMap::new();
    let mut raw = vec![0_u8; 8 * MIB as usize];
    let mut seed = 0x647fa732db98765_u64;
    for bytes in raw.chunks_exact_mut(8) {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        bytes.copy_from_slice(&seed.to_le_bytes());
    }
    for i in 0..32_u64 {
        raw[..8].copy_from_slice(&i.to_le_bytes());
        let mut encoder = zstd::stream::Encoder::new(Vec::new(), 6).unwrap();
        encoder.include_checksum(true).unwrap();
        encoder.write_all(&raw).unwrap();
        let body = Arc::new(encoder.finish().unwrap());
        let compressed = digest(&body);
        let raw_sha = digest(&raw);
        let replicas = (0..3)
            .map(|r| format!("probe_file_{i:04}_{r:04}"))
            .collect::<Vec<_>>();
        for replica in &replicas {
            bodies.insert(replica.clone(), body.clone());
        }
        manifest.packs.insert(
            compressed.clone(),
            DrivePack {
                size: body.len() as u64,
                replica_file_ids: replicas,
            },
        );
        manifest.chunks.insert(
            raw_sha.clone(),
            PackedContentChunk {
                uncompressed_size: raw.len() as u64,
                compressed_size: body.len() as u64,
                compressed_sha256: compressed.clone(),
                pack_sha256: compressed,
                offset: 0,
            },
        );
        manifest.files.push(ContentFile {
            path: if i == 0 {
                "RevLoader.exe".into()
            } else {
                format!("data/{i}.bin")
            },
            size: raw.len() as u64,
            sha256: raw_sha.clone(),
            excluded_from_hash_check: false,
            temporary: false,
            additional_check: false,
            chunks: vec![raw_sha],
        });
    }
    manifest.download_size = manifest.packs.values().map(|p| p.size).sum();
    manifest.unpacked_size = manifest.files.iter().map(|f| f.size).sum();
    let manifest = crate::drive_pack_tests::finish_manifest(manifest);
    manifest.validate().unwrap();
    (Arc::new(manifest), Arc::new(bodies))
}
struct CountGuard(Arc<AtomicUsize>);
impl Drop for CountGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, SeqCst);
    }
}
struct Peer {
    url: reqwest::Url,
    gate: Arc<Semaphore>,
    throttled: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    pause_at: Arc<AtomicU64>,
    pause_first_only: Arc<AtomicBool>,
    body_gate: Arc<Semaphore>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl Peer {
    async fn start(bodies: Bodies) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = reqwest::Url::parse(&format!(
            "http://{}/download",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let gate = Arc::new(Semaphore::new(0));
        let throttled = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let stop = CancellationToken::new();
        let pause_at = Arc::new(AtomicU64::new(u64::MAX));
        let pause_first_only = Arc::new(AtomicBool::new(false));
        let first_only = pause_first_only.clone();
        let body_gate = Arc::new(Semaphore::new(0));
        let (pause, body_permits) = (pause_at.clone(), body_gate.clone());
        let (tg, tt, ta, tp, ts) = (
            gate.clone(),
            throttled.clone(),
            active.clone(),
            peak.clone(),
            stop.clone(),
        );
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    _ = ts.cancelled() => break,
                    Some(_) = connections.join_next(), if !connections.is_empty() => {},
                    accepted = listener.accept() => {
                        let (mut socket, _) = accepted.unwrap();
                        let (bodies, gate, throttled, active, peak, stop) = (bodies.clone(), tg.clone(), tt.clone(), ta.clone(), tp.clone(), ts.clone());
                        let (pause,body_permits)=(pause.clone(),body_permits.clone());
                        let first_only=first_only.clone();
                        connections.spawn(async move {
                            let mut request = Vec::new();
                            loop {
                                let mut bytes = [0; 1024];
                                let size = tokio::select! { _ = stop.cancelled() => return, n = socket.read(&mut bytes) => n.unwrap_or(0) };
                                if size == 0 { return; }
                                request.extend_from_slice(&bytes[..size]);
                                if request.ends_with(b"\r\n\r\n") { break; }
                                assert!(request.len() < 16384);
                            }
                            let request = String::from_utf8(request).unwrap();
                            let relative = request.split_whitespace().nth(1).unwrap();
                            let parsed = reqwest::Url::parse(&format!("http://127.0.0.1{relative}")).unwrap();
                            let id = parsed.query_pairs().find(|(k,_)| k == "id").unwrap().1.into_owned();
                            let body = &bodies[&id];
                            let range = request.lines().find_map(|line|line.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_owned)).unwrap();
                            let (start, end) = range.split_once('-').unwrap();
                            let (start, end) = (start.parse::<usize>().unwrap(), end.parse::<usize>().unwrap());
                            let count = active.fetch_add(1, SeqCst) + 1; peak.fetch_max(count, SeqCst);
                            let _active = CountGuard(active);
                            let permit = tokio::select! { _ = stop.cancelled() => return, p = gate.acquire() => p.unwrap() };
                            permit.forget();
                            if throttled.swap(false, SeqCst) {
                                let _ = socket.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                                return;
                            }
                            let header = format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n", end-start+1, body.len());
                            if socket.write_all(header.as_bytes()).await.is_err() { return; }
                            let split=if start>0 && first_only.load(SeqCst) {end+1} else {(pause.load(SeqCst) as usize).clamp(start,end+1)};
                            if split>start && tokio::select! { _=stop.cancelled()=>return, result=socket.write_all(&body[start..split])=>result }.is_err(){return;}
                            if split<=end {
                                let permit=tokio::select!{_=stop.cancelled()=>return,p=body_permits.acquire()=>p.unwrap()};permit.forget();
                                let _=tokio::select!{_=stop.cancelled()=>return,result=socket.write_all(&body[split..=end])=>result};
                            }
                        });
                    }
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        Self {
            url,
            gate,
            throttled,
            active,
            peak,
            pause_at,
            pause_first_only,
            body_gate,
            stop,
            task,
        }
    }
    async fn shutdown(self) {
        self.stop.cancel();
        self.task.await.unwrap();
        assert_eq!(self.active.load(SeqCst), 0);
    }
}
async fn until(mut condition: impl FnMut() -> bool, label: &str) {
    tokio::time::timeout(Duration::from_secs(40), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {label}"));
}
fn reason(metrics: &PackMetrics, reason: &str) -> bool {
    metrics
        .history()
        .iter()
        .any(|s| s.controller_reason == reason)
}

fn pipeline_fixture() -> (Arc<DrivePackManifest>, Bodies, u64) {
    let mut manifest = crate::drive_pack_tests::two_chunk_manifest();
    manifest.files.clear();
    manifest.chunks.clear();
    manifest.packs.clear();
    let mut pack = Vec::new();
    let mut boundary = 0;
    for i in 0..3 {
        let mut seed = 72361579123_u64 + i;
        let mut raw = vec![0u8; 8 * MIB as usize];
        for block in raw.chunks_exact_mut(8) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            block.copy_from_slice(&seed.to_le_bytes());
        }
        let mut encoder = zstd::stream::Encoder::new(Vec::new(), 6).unwrap();
        encoder.include_checksum(true).unwrap();
        encoder.write_all(&raw).unwrap();
        let compressed = encoder.finish().unwrap();
        let raw_sha = digest(&raw);
        manifest.chunks.insert(
            raw_sha.clone(),
            PackedContentChunk {
                uncompressed_size: raw.len() as u64,
                compressed_size: compressed.len() as u64,
                compressed_sha256: digest(&compressed),
                pack_sha256: String::new(),
                offset: pack.len() as u64,
            },
        );
        pack.extend_from_slice(&compressed);
        manifest.files.push(ContentFile {
            path: if i == 0 {
                "RevLoader.exe".into()
            } else {
                format!("data/{i}.bin")
            },
            size: raw.len() as u64,
            sha256: raw_sha.clone(),
            excluded_from_hash_check: false,
            temporary: false,
            additional_check: false,
            chunks: vec![raw_sha],
        });
        if i == 1 {
            boundary = pack.len() as u64;
        }
    }
    let sha = digest(&pack);
    let replicas = (0..3)
        .map(|i| format!("pipeline_replica_{i:03}"))
        .collect::<Vec<_>>();
    for chunk in manifest.chunks.values_mut() {
        chunk.pack_sha256 = sha.clone();
    }
    manifest.packs.insert(
        sha,
        DrivePack {
            size: pack.len() as u64,
            replica_file_ids: replicas.clone(),
        },
    );
    manifest.download_size = pack.len() as u64;
    manifest.unpacked_size = 24 * MIB;
    let body = Arc::new(pack);
    let bodies = replicas.into_iter().map(|id| (id, body.clone())).collect();
    let manifest = crate::drive_pack_tests::finish_manifest(manifest);
    manifest.validate().unwrap();
    (Arc::new(manifest), Arc::new(bodies), boundary)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drive_pack_early_files_cross_range_and_wait_hash_barrier_or_rollback() {
    use crate::services::{api_client::ApiClient, content_pack_install_service::run_packed_probe};
    let (manifest, bodies, boundary) = pipeline_fixture();
    assert!(boundary > 16 * MIB); // second compressed chunk crosses the HTTP boundary
    for cancel in [false, true] {
        let peer = Peer::start(bodies.clone()).await;
        peer.pause_at.store(boundary, SeqCst);
        peer.gate.add_permits(100);
        let dir = scratch();
        let game = dir.path().join("game");
        let mut options = PackRunOptions::new("early-pipeline");
        options.local_transport = Some(peer.url.clone());
        let metrics = options.metrics.clone();
        let token = CancellationToken::new();
        let (m, g, c) = (manifest.clone(), game.clone(), token.clone());
        let task = tokio::spawn(async move {
            run_packed_probe(
                &ApiClient::new().unwrap(),
                m.clone(),
                g,
                m.files.clone(),
                options,
                c,
            )
            .await
        });
        let scenario = async {
            until(
                || metrics.snapshot().committed_bytes == 16 * MIB,
                "early file commits before pack EOF",
            )
            .await;
            assert!(!task.is_finished());
            assert!(metrics.snapshot().download_finished_sec.is_none());
            assert!(!game.join(".zhekarik/content/state.json").exists());
            assert_eq!(
                std::fs::metadata(game.join("RevLoader.exe")).unwrap().len(),
                8 * MIB
            );
            if cancel {
                token.cancel();
            } else {
                peer.body_gate.add_permits(100);
            }
        };
        let outcome =
            futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(scenario)).await;
        if outcome.is_err() {
            token.cancel();
        }
        let result = task.await.unwrap();
        if outcome.is_ok() {
            if cancel {
                assert!(result.is_err());
                assert!(!game.join("RevLoader.exe").exists());
                assert!(!game.join(".zhekarik/content/state.json").exists());
            } else {
                assert!(result.is_ok(), "{result:?}");
                assert!(game.join(".zhekarik/content/state.json").exists());
            }
        }
        assert_eq!(metrics.snapshot().active_jobs, 0);
        assert_eq!(metrics.snapshot().active_requests, 0);
        assert_eq!(metrics.snapshot().active_materializers, 0);
        peer.shutdown().await;
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drive_pack_resume_prefix_and_corruption_never_finalize_unverified_data() {
    use crate::services::{api_client::ApiClient, content_pack_install_service::run_packed_probe};
    let (manifest, bodies, _) = pipeline_fixture();
    // Persisted prefix is rehashed and usable, but contributes no new traffic.
    let peer = Peer::start(bodies.clone()).await;
    peer.gate.add_permits(100);
    let dir = scratch();
    let cache = PackCache::new(dir.path(), &manifest.content_sha256, "resume")
        .await
        .unwrap();
    let pack_sha = manifest.packs.keys().next().unwrap();
    let body = bodies.values().next().unwrap();
    std::fs::write(
        cache.full_partial_path(pack_sha).unwrap(),
        &body[..16 * MIB as usize],
    )
    .unwrap();
    let required = manifest
        .files
        .iter()
        .flat_map(|f| f.chunks.clone())
        .collect::<Vec<_>>();
    let plans = plan_pack_fetches(&manifest, &required).unwrap();
    let mut options = PackRunOptions::new("resume");
    options.local_transport = Some(peer.url.clone());
    let metrics = options.metrics.clone();
    let (tx, mut rx) = mpsc::channel(64);
    let consume = tokio::spawn(async move {
        let mut count = 0;
        while let Some(event) = rx.recv().await {
            if let crate::services::content_pack_download_service::PackDownloadEvent::ChunkReady(
                _,
            ) = event
            {
                count += 1;
            }
        }
        count
    });
    download_pack_fetches(
        reqwest::Client::new(),
        manifest.clone(),
        plans,
        cache,
        "resume",
        CancellationToken::new(),
        tx,
        Arc::new(AtomicU64::new(0)),
        options,
    )
    .await
    .unwrap();
    assert_eq!(consume.await.unwrap(), 3);
    assert_eq!(
        metrics.snapshot().received_bytes,
        body.len() as u64 - 16 * MIB
    );
    assert_eq!(
        metrics.snapshot().unique_bytes,
        metrics.snapshot().received_bytes
    );
    peer.shutdown().await;
    // Valid compressed/raw chunks are still insufficient: the full pack and
    // final file hashes independently veto state/inventory finalization.
    for corrupt in ["pack", "chunk", "file"] {
        let mut bad = (*manifest).clone();
        if corrupt == "pack" {
            let old = bad.packs.keys().next().unwrap().clone();
            let pack = bad.packs.remove(&old).unwrap();
            let wrong = "f".repeat(64);
            bad.packs.insert(wrong.clone(), pack);
            for chunk in bad.chunks.values_mut() {
                chunk.pack_sha256 = wrong.clone();
            }
        } else if corrupt == "chunk" {
            bad.chunks
                .get_mut(&bad.files[0].chunks[0])
                .unwrap()
                .compressed_sha256 = "f".repeat(64);
        } else {
            bad.files[0].sha256 = "f".repeat(64);
        }
        let bad = Arc::new(crate::drive_pack_tests::finish_manifest(bad));
        bad.validate().unwrap();
        let peer = Peer::start(bodies.clone()).await;
        peer.gate.add_permits(100);
        let dir = scratch();
        let game = dir.path().join("game");
        let mut options = PackRunOptions::new(corrupt);
        options.local_transport = Some(peer.url.clone());
        let metrics = options.metrics.clone();
        let result = run_packed_probe(
            &ApiClient::new().unwrap(),
            bad.clone(),
            game.clone(),
            bad.files.clone(),
            options,
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err(), "{corrupt} corruption was accepted");
        assert!(!game.join(".zhekarik/content/state.json").exists());
        assert!(!game.join("RevLoader.exe").exists());
        assert_eq!(metrics.snapshot().active_jobs, 0);
        assert_eq!(metrics.snapshot().active_materializers, 0);
        peer.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drive_pack_prefetch_overlaps_current_caps_buffer_and_drains_cancel() {
    use crate::services::{api_client::ApiClient, content_pack_install_service::run_packed_probe};
    let (manifest, bodies, _) = pipeline_fixture();
    for cancel in [false, true] {
        let peer = Peer::start(bodies.clone()).await;
        peer.pause_at.store(15 * MIB, SeqCst);
        peer.pause_first_only.store(true, SeqCst);
        peer.gate.add_permits(100);
        let dir = scratch();
        let game = dir.path().join("game");
        let mut options = PackRunOptions::new("prefetch-test");
        options.local_transport = Some(peer.url.clone());
        options.metrics.materializer(2, 2, 0., 4096 * MIB);
        let metrics = options.metrics.clone();
        let token = CancellationToken::new();
        let (m, g, c) = (manifest.clone(), game.clone(), token.clone());
        let task = tokio::spawn(async move {
            run_packed_probe(
                &ApiClient::new().unwrap(),
                m.clone(),
                g,
                m.files.clone(),
                options,
                c,
            )
            .await
        });
        let scenario = async {
            until(
                || metrics.snapshot().prefetch_bytes >= MIB,
                "bounded prefetch while current is paused",
            )
            .await;
            let snapshot = metrics.snapshot();
            // Socket reads can be shorter than64KiB: the bound is a maximum,
            // not a promise that the32 slots always contain exactly2MiB.
            assert!(snapshot.prefetch_bytes >= MIB && snapshot.prefetch_bytes <= 2 * MIB);
            assert!(snapshot.prefetch_peak_buffer_bytes <= 2 * MIB);
            assert_eq!(snapshot.peak_requests, 2);
            assert!(snapshot.peak_requests <= 6);
            assert!(snapshot.http_protocols.contains_key("HTTP/1.1"));
            assert!(!game.join(".zhekarik/content/state.json").exists());
            assert!(!task.is_finished());
        };
        let outcome =
            futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(scenario)).await;
        if cancel || outcome.is_err() {
            token.cancel();
        } else {
            peer.body_gate.add_permits(100);
        }
        let result = task.await.unwrap();
        if outcome.is_ok() {
            assert_eq!(result.is_err(), cancel, "{result:?}");
            assert_eq!(game.join(".zhekarik/content/state.json").exists(), !cancel);
        }
        assert_eq!(metrics.snapshot().active_requests, 0);
        assert_eq!(metrics.snapshot().active_jobs, 0);
        peer.shutdown().await;
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drive_pack_real_scheduler_trials_backlog_pressure_and_cancel() {
    let (manifest, bodies) = fixture();
    for (reject, optimized) in [(false, false), (true, false), (false, true), (true, true)] {
        let peer = Peer::start(bodies.clone()).await;
        let game = scratch();
        let cache = PackCache::new(game.path(), &manifest.content_sha256, "scheduler-probe")
            .await
            .unwrap();
        let required = manifest
            .files
            .iter()
            .flat_map(|f| f.chunks.iter().cloned())
            .collect::<Vec<_>>();
        let plans = plan_pack_fetches(&manifest, &required).unwrap();
        let (events, mut receiver) = mpsc::channel(64);
        let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
        let cancellation = CancellationToken::new();
        let clock = Arc::new(AtomicU64::new(0));
        let backlog = Arc::new(AtomicU64::new(if reject { 0 } else { 256 * MIB }));
        let mut options = PackRunOptions::new("controlled-scheduler");
        options.profile = if optimized {
            crate::services::content_pack_metrics::PackProfile::Optimized
        } else {
            crate::services::content_pack_metrics::PackProfile::Baseline
        };
        options.metrics.materializer(2, 2, 0., 4096 * MIB);
        options.local_transport = Some(peer.url.clone());
        options.tick_interval = Duration::from_millis(10);
        options.clock_ms = Some(clock.clone());
        let metrics = options.metrics.clone();
        let (task_manifest, task_backlog, task_cancel) =
            (manifest.clone(), backlog.clone(), cancellation.clone());
        let download = tokio::spawn(async move {
            download_pack_fetches(
                reqwest::Client::new(),
                task_manifest,
                plans,
                cache,
                "scheduler-probe",
                task_cancel,
                events,
                task_backlog,
                options,
            )
            .await
        });
        let scenario = async {
            until(
                || metrics.snapshot().active_requests == 2 && !metrics.history().is_empty(),
                "initial two requests",
            )
            .await;
            let mut at = if optimized && reject { 10000 } else { 8000 };
            if !reject {
                clock.store(at, SeqCst);
                peer.gate.add_permits(8);
                until(
                    || {
                        reason(&metrics, "ready_backlog")
                            && metrics.snapshot().received_bytes >= 64 * MIB
                    },
                    "backlog hold",
                )
                .await;
                assert_eq!(metrics.snapshot().target_jobs, 2);
                assert_eq!(metrics.snapshot().peak_jobs, 2);
                backlog.store(0, SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                at += if optimized { 10000 } else { 8000 };
            }
            clock.store(at, SeqCst);
            peer.gate.add_permits(8);
            until(
                || {
                    reason(&metrics, "trial_increase")
                        && metrics.snapshot().active_requests >= 3
                        && peer.active.load(SeqCst) >= 3
                },
                "real third task/request",
            )
            .await;
            assert_eq!(metrics.snapshot().target_jobs, 3);
            if optimized {
                tokio::time::sleep(Duration::from_millis(50)).await; // Observe the actual third task.
                clock.store(at + 2000, SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await; // Start post-warmup window.
            }
            let verdict_at = at
                + if optimized {
                    if reject {
                        16000
                    } else {
                        12000
                    }
                } else if reject {
                    8000
                } else {
                    4000
                };
            if optimized {
                // Deliver the controlled window before advancing virtual time.
                // Otherwise the first 64 MiB at t=10s legitimately opens an
                // extension before the ninth pack arrives at that SAME instant.
                let count = if reject { 8 } else { 9 };
                let expected = metrics.snapshot().received_bytes + count as u64 * 8 * MIB;
                peer.gate.add_permits(count);
                until(
                    || metrics.snapshot().received_bytes >= expected,
                    "controlled window bytes",
                )
                .await;
                clock.store(verdict_at, SeqCst);
            } else {
                clock.store(verdict_at, SeqCst);
                peer.gate.add_permits(8);
            }
            until(
                || {
                    reason(
                        &metrics,
                        if reject {
                            "trial_rejected"
                        } else {
                            "trial_accepted"
                        },
                    )
                },
                "trial verdict",
            )
            .await;
            assert_eq!(metrics.snapshot().target_jobs, if reject { 2 } else { 3 });
            if !reject {
                peer.throttled.store(true, SeqCst);
                peer.gate.add_permits(1);
                until(|| reason(&metrics, "pressure"), "HTTP 429 pressure").await;
                assert_eq!(metrics.snapshot().target_jobs, 1);
            }
            assert!(peer.peak.load(SeqCst) >= 3);
            assert!(metrics.snapshot().peak_jobs >= 3);
        };
        let result =
            futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(scenario)).await;
        cancellation.cancel();
        assert!(download.await.unwrap().is_err());
        drain.await.unwrap();
        assert_eq!(metrics.snapshot().active_jobs, 0);
        assert_eq!(metrics.snapshot().active_requests, 0);
        peer.shutdown().await;
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}
