use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::models::{
    expected_game_file_url, legacy_json_identity, ContentChunking, ContentCompression, ContentFile,
    ContentInventory, DrivePack, DrivePackManifest, DrivePackProfile, GameArchiveManifest,
    GameFileManifestEntry, GameManifest, PackedContentChunk,
};
use crate::services::api_client::parse_content_pack_manifest_response;
use crate::services::content_commit_service::{
    queue_success_cleanup, run_streaming_commit, CommitContext, StagingBudget, VerifiedArtifact,
};
use crate::services::content_inventory_service::{
    load_content_inventory, migrate_persisted_v2_manifest, save_content_inventory,
};
use crate::services::content_journal_service::{
    atomic_json, content_root, journal_path, staging_path, write_journal, ContentJournal,
    ContentJournalAction, ContentJournalEntry, ContentJournalPhase,
};
use crate::services::content_pack_controller::{
    AdaptivePackController, ControllerSample, PressureWindow,
};
use crate::services::content_pack_download_service::{
    full_pack_request_range, VerifiedPackedChunk,
};
use crate::services::content_pack_install_service::read_packed_chunk;
use crate::services::content_pack_plan_service::{plan_pack_fetches, ByteRange, PackTransferMode};

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn finish_manifest(mut manifest: DrivePackManifest) -> DrivePackManifest {
    manifest.content_sha256 = legacy_json_identity(&manifest.legacy_content_projection().unwrap())
        .expect("legacy identity");
    let mut value = serde_json::to_value(&manifest).unwrap();
    value.as_object_mut().unwrap().remove("manifest_sha256");
    manifest.manifest_sha256 = sha(&serde_json_canonicalizer::to_vec(&value).unwrap());
    manifest
}

pub(crate) fn two_chunk_manifest() -> DrivePackManifest {
    let raw_a = vec![b'a'; 10];
    let raw_b = vec![b'b'; 90];
    let raw_a_sha = sha(&raw_a);
    let raw_b_sha = sha(&raw_b);
    let compressed_a = vec![1_u8; 10];
    let compressed_b = vec![2_u8; 90];
    let mut pack_bytes = compressed_a.clone();
    pack_bytes.extend_from_slice(&compressed_b);
    let pack_sha = sha(&pack_bytes);
    let chunks = BTreeMap::from([
        (
            raw_a_sha.clone(),
            PackedContentChunk {
                uncompressed_size: 10,
                compressed_size: 10,
                compressed_sha256: sha(&compressed_a),
                pack_sha256: pack_sha.clone(),
                offset: 0,
            },
        ),
        (
            raw_b_sha.clone(),
            PackedContentChunk {
                uncompressed_size: 90,
                compressed_size: 90,
                compressed_sha256: sha(&compressed_b),
                pack_sha256: pack_sha.clone(),
                offset: 10,
            },
        ),
    ]);
    finish_manifest(DrivePackManifest {
        schema_version: 3,
        manifest_sha256: "0".repeat(64),
        content_sha256: "0".repeat(64),
        release_id: "1.0.3.6-r1".into(),
        game_version: "1.0.3.6".into(),
        generated_at: "2026-09-02T00:00:00Z".into(),
        source_archive_sha256: "3".repeat(64),
        download_size: 100,
        unpacked_size: 100,
        chunking: ContentChunking {
            profile: "fixed-v1".into(),
            chunk_size: 8 * 1024 * 1024,
        },
        compression: ContentCompression {
            profile: "zstd-v1".into(),
            level: 6,
            frame_checksum: true,
        },
        pack_profile: DrivePackProfile {
            name: "drive-pack-v1".into(),
            max_pack_size: 64 * 1024 * 1024,
            replica_count: 3,
        },
        packs: BTreeMap::from([(
            pack_sha,
            DrivePack {
                size: 100,
                replica_file_ids: vec![
                    "drive_file_id_0001".into(),
                    "drive_file_id_0002".into(),
                    "drive_file_id_0003".into(),
                ],
            },
        )]),
        chunks,
        files: vec![
            ContentFile {
                path: "RevLoader.exe".into(),
                size: 10,
                sha256: sha(&raw_a),
                excluded_from_hash_check: false,
                temporary: false,
                additional_check: true,
                chunks: vec![raw_a_sha],
            },
            ContentFile {
                path: "bin/data.bin".into(),
                size: 90,
                sha256: sha(&raw_b),
                excluded_from_hash_check: false,
                temporary: false,
                additional_check: false,
                chunks: vec![raw_b_sha],
            },
            ContentFile {
                path: "empty.dat".into(),
                size: 0,
                sha256: sha(b""),
                excluded_from_hash_check: false,
                temporary: false,
                additional_check: false,
                chunks: Vec::new(),
            },
        ],
    })
}

fn canonical_v1(manifest: &DrivePackManifest) -> GameManifest {
    GameManifest {
        game_version: manifest.game_version.clone(),
        generated_at: manifest.generated_at.clone(),
        files: manifest
            .files
            .iter()
            .map(|file| GameFileManifestEntry {
                path: file.path.clone(),
                size: file.size,
                sha256: file.sha256.clone(),
                url: expected_game_file_url(&file.path),
                excluded_from_hash_check: file.excluded_from_hash_check,
                temporary: file.temporary,
            })
            .collect(),
        archive: GameArchiveManifest {
            url: "https://api.zhekarik.africa/launcher/game/archive".into(),
            size: 1,
            sha256: manifest.source_archive_sha256.clone(),
            unpacked_size: manifest.unpacked_size,
        },
    }
}

#[test]
fn drive_pack_manifest_identity_v1_closure_url_and_discovery_matrix() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../tests/fixtures/content-v3-canonicalization.json"
    ))
    .unwrap();
    assert_eq!(
        legacy_json_identity(&fixture["legacy_content"]["document"]).unwrap(),
        fixture["legacy_content"]["sha256"].as_str().unwrap()
    );

    let manifest = two_chunk_manifest();
    manifest.validate().unwrap();
    manifest
        .validate_against_v1(&canonical_v1(&manifest))
        .unwrap();
    let url = DrivePackManifest::drive_url("drive_file_id_0001").unwrap();
    assert_eq!(url.host_str(), Some("drive.usercontent.google.com"));
    assert!(url
        .as_str()
        .starts_with("https://drive.usercontent.google.com/"));
    assert!(
        parse_content_pack_manifest_response(StatusCode::NOT_FOUND, b"")
            .unwrap()
            .is_none()
    );
    assert!(parse_content_pack_manifest_response(StatusCode::SERVICE_UNAVAILABLE, b"").is_err());
}

#[tokio::test]
async fn drive_pack_planning_cache_spans_and_integrity_matrix() {
    let manifest = two_chunk_manifest();
    let raw = manifest
        .files
        .iter()
        .flat_map(|file| file.chunks.iter().cloned())
        .collect::<Vec<_>>();
    let sparse = plan_pack_fetches(&manifest, &raw[..1]).unwrap();
    assert!(matches!(sparse[0].mode, PackTransferMode::Ranges(_)));
    let dense = plan_pack_fetches(&manifest, &raw).unwrap();
    assert!(matches!(dense[0].mode, PackTransferMode::Full));

    let compressed = zstd::stream::encode_all(&b"packed data"[..], 6).unwrap();
    let raw_sha = sha(b"packed data");
    let chunk = crate::models::ContentChunk {
        uncompressed_size: 11,
        compressed_size: compressed.len() as u64,
        compressed_sha256: sha(&compressed),
    };
    let directory = tempfile::tempdir().unwrap();
    let pack = directory.path().join("pack.bin");
    tokio::fs::write(&pack, &compressed).await.unwrap();
    let decoded = read_packed_chunk(
        VerifiedPackedChunk {
            raw_sha256: raw_sha.clone(),
            compressed_sha256: chunk.compressed_sha256.clone(),
            path: pack,
            offset: 0,
            compressed_size: chunk.compressed_size,
            uncompressed_size: chunk.uncompressed_size,
        },
        raw_sha,
        chunk,
    )
    .await
    .unwrap();
    assert_eq!(decoded, b"packed data");
}

#[test]
fn drive_pack_adaptive_controller_trial_pressure_and_cooldown_matrix() {
    let mut controller = AdaptivePackController::new(6);
    let started = Instant::now();
    for tick in 1..=3 {
        controller.observe(
            started + Duration::from_secs(tick * 2),
            ControllerSample {
                useful_bytes: tick * 32 * 1024 * 1024,
                ready_backlog_bytes: 128 * 1024 * 1024,
                pressure: PressureWindow::default(),
                active_attempts: Vec::new(),
            },
        );
    }
    assert_eq!(controller.target(), 3);
    let decision = controller.observe(
        started + Duration::from_secs(8),
        ControllerSample {
            useful_bytes: 128 * 1024 * 1024,
            ready_backlog_bytes: 128 * 1024 * 1024,
            pressure: PressureWindow {
                throttled: true,
                timeout_or_server_errors: 0,
            },
            active_attempts: Vec::new(),
        },
    );
    assert!(decision.changed);
    assert_eq!(decision.target, 1);
}

#[test]
fn drive_pack_adaptive_controller_waits_for_materialization_then_trials_and_rejects_no_gain() {
    let mut controller = AdaptivePackController::new(6);
    let started = Instant::now();
    let mib = 1024 * 1024;

    // Downloads may be far from complete: only verified bytes waiting for the
    // materializer should inhibit a higher-concurrency trial.
    for tick in 1..=3 {
        controller.observe(
            started + Duration::from_secs(tick * 2),
            ControllerSample {
                useful_bytes: tick * 32 * mib,
                ready_backlog_bytes: 256 * mib,
                pressure: PressureWindow::default(),
                active_attempts: Vec::new(),
            },
        );
    }
    assert_eq!(
        controller.target(),
        2,
        "do not outrun a backed-up materializer"
    );

    for tick in 4..=6 {
        controller.observe(
            started + Duration::from_secs(tick * 2),
            ControllerSample {
                useful_bytes: tick * 32 * mib,
                ready_backlog_bytes: 32 * mib,
                pressure: PressureWindow::default(),
                active_attempts: Vec::new(),
            },
        );
    }
    assert_eq!(
        controller.target(),
        3,
        "trial once the materializer catches up"
    );

    for tick in 7..=9 {
        controller.observe(
            started + Duration::from_secs(tick * 2),
            ControllerSample {
                useful_bytes: tick * 32 * mib,
                ready_backlog_bytes: 0,
                pressure: PressureWindow::default(),
                active_attempts: Vec::new(),
            },
        );
    }
    assert_eq!(
        controller.target(),
        2,
        "extra workers without a speed gain are rolled back"
    );
}

#[test]
fn drive_pack_full_download_uses_bounded_ranges() {
    let slice = 16 * 1024 * 1024;
    let pack_size = 64 * 1024 * 1024 - 17;

    assert_eq!(
        full_pack_request_range(0, pack_size).unwrap(),
        ByteRange {
            start: 0,
            end_inclusive: slice - 1,
        }
    );
    assert_eq!(
        full_pack_request_range(slice, pack_size).unwrap(),
        ByteRange {
            start: slice,
            end_inclusive: (2 * slice) - 1,
        }
    );
    assert_eq!(
        full_pack_request_range(3 * slice, pack_size).unwrap(),
        ByteRange {
            start: 3 * slice,
            end_inclusive: pack_size - 1,
        }
    );
}

#[tokio::test]
async fn drive_pack_materialization_inventory_and_migration_matrix() {
    let manifest = two_chunk_manifest();
    let inventory = ContentInventory::from_v3(&manifest).unwrap();
    let directory = tempfile::tempdir().unwrap();
    save_content_inventory(directory.path(), &inventory)
        .await
        .unwrap();
    assert_eq!(
        load_content_inventory(directory.path(), &inventory.content_sha256)
            .await
            .unwrap()
            .unwrap()
            .release_id,
        inventory.release_id
    );

    let migration_root = tempfile::tempdir().unwrap();
    let v2 = inventory.as_v2_manifest();
    let path = content_root(migration_root.path())
        .join("manifests")
        .join(format!("{}.json", v2.content_sha256));
    atomic_json(&path, &v2).await.unwrap();
    let migrated =
        migrate_persisted_v2_manifest(migration_root.path(), &v2.content_sha256, &v2.release_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(migrated.content_sha256, v2.content_sha256);
    assert!(tokio::fs::try_exists(path).await.unwrap());
}

#[tokio::test]
async fn drive_pack_streaming_commit_recovery_unknown_files_and_cleanup_matrix() {
    let manifest = two_chunk_manifest();
    let inventory = ContentInventory::from_v3(&manifest).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let game = directory.path();
    tokio::fs::write(game.join("user.cfg"), b"keep")
        .await
        .unwrap();
    let transaction_id = uuid::Uuid::new_v4().to_string();
    let staging = staging_path(game, &transaction_id);
    tokio::fs::create_dir_all(&staging).await.unwrap();
    let new_bytes = vec![b'a'; 10];
    let staged = staging.join("RevLoader.exe");
    tokio::fs::write(&staged, &new_bytes).await.unwrap();
    let journal = ContentJournal {
        schema_version: 2,
        transaction_id: transaction_id.clone(),
        release_id: inventory.release_id.clone(),
        content_sha256: inventory.content_sha256.clone(),
        phase: ContentJournalPhase::StreamingCommit,
        files: vec![ContentJournalEntry {
            path: "RevLoader.exe".into(),
            action: ContentJournalAction::Replace,
            had_original: false,
            target_size: Some(10),
            target_sha256: Some(sha(&new_bytes)),
            original_size: None,
            original_sha256: None,
        }],
    };
    write_journal(game, &journal).await.unwrap();
    let (artifact_tx, artifact_rx) = mpsc::channel(1);
    artifact_tx
        .send(VerifiedArtifact {
            relative_path: PathBuf::from("RevLoader.exe"),
            temporary_path: staged,
            size: 10,
            sha256: sha(&new_bytes),
        })
        .await
        .unwrap();
    drop(artifact_tx);
    let (committed, mut committed_rx) = mpsc::channel(1);
    let state = run_streaming_commit(
        CommitContext {
            game_path: game.to_path_buf(),
            journal,
            inventory: inventory.clone(),
            staging_budget: StagingBudget::new(1024).unwrap(),
            committed,
        },
        artifact_rx,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(committed_rx.recv().await, Some(10));
    assert_eq!(state.content_sha256, inventory.content_sha256);
    assert_eq!(
        tokio::fs::read(game.join("RevLoader.exe")).await.unwrap(),
        new_bytes
    );
    assert_eq!(
        tokio::fs::read(game.join("user.cfg")).await.unwrap(),
        b"keep"
    );
    queue_success_cleanup(game, &transaction_id, &inventory.content_sha256)
        .await
        .unwrap();
    assert!(!tokio::fs::try_exists(journal_path(game)).await.unwrap());
}
