use std::collections::HashSet;
use std::io::Cursor;

use reqwest::StatusCode;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::models::{ContentChunk, ContentManifest, ContentMirrorIndex};
use crate::services::api_client::{parse_content_manifest_response, parse_content_mirror_response};
use crate::services::content_download_service::{
    decode_verified_chunk, download_content_chunk, read_verified_local_chunk, DriveCircuitBreaker,
};
use crate::services::content_install_service::{
    cleanup_obsolete_directories, commit_staged_files, load_obsolete_content_entries,
    materializer_worker_limits, required_content_install_bytes, wait_until_chunk_ready,
    AdaptiveDownloadController, ChunkReadiness,
};
use crate::services::content_journal_service::{
    atomic_json, backup_path, content_root, journal_path, recover_pending_content, staging_path,
    state_path, write_journal, ContentCompletionState, ContentJournal, ContentJournalAction,
    ContentJournalEntry, ContentJournalPhase,
};

fn sha256(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn encoded(raw: &[u8]) -> Vec<u8> {
    zstd::stream::encode_all(Cursor::new(raw), 6).expect("fixture should compress")
}

fn manifest_json(raw: &[u8], compressed: &[u8]) -> serde_json::Value {
    let raw_sha = sha256(raw);
    let compressed_sha = sha256(compressed);
    json!({
        "schema_version": 2,
        "content_sha256": "a".repeat(64),
        "release_id": "1.0.3.4-r1",
        "game_version": "1.0.3.4",
        "generated_at": "2026-07-27T06:00:00Z",
        "source_archive_sha256": "b".repeat(64),
        "delivery": {
            "chunk_base_url": "https://api.zhekarik.africa/launcher/game/v2/chunks",
            "recommended_concurrency": 1
        },
        "chunking": {"profile": "fixed-v1", "chunk_size": 8388608},
        "compression": {"profile": "zstd-v1", "level": 6, "frame_checksum": true},
        "download_size": compressed.len(),
        "unpacked_size": raw.len(),
        "chunks": {
            raw_sha.clone(): {
                "uncompressed_size": raw.len(),
                "compressed_size": compressed.len(),
                "compressed_sha256": compressed_sha
            }
        },
        "files": [{
            "path": "RevLoader.exe",
            "size": raw.len(),
            "sha256": raw_sha.clone(),
            "excluded_from_hash_check": false,
            "temporary": false,
            "additional_check": true,
            "chunks": [raw_sha]
        }]
    })
}

fn content_deletion_manifest(
    content_marker: char,
    release_id: &str,
    game_version: &str,
    extra_files: &[(&str, &[u8])],
) -> ContentManifest {
    let loader = format!("loader-{content_marker}").into_bytes();
    let loader_compressed = encoded(&loader);
    let mut document = manifest_json(&loader, &loader_compressed);
    document["content_sha256"] = json!(content_marker.to_string().repeat(64));
    document["release_id"] = json!(release_id);
    document["game_version"] = json!(game_version);
    document["source_archive_sha256"] = json!("f".repeat(64));
    let mut unpacked_size = loader.len() as u64;
    let mut download_size = loader_compressed.len() as u64;
    for (path, raw) in extra_files {
        let compressed = encoded(raw);
        let raw_sha = sha256(raw);
        document["chunks"][&raw_sha] = json!({
            "uncompressed_size": raw.len(),
            "compressed_size": compressed.len(),
            "compressed_sha256": sha256(&compressed)
        });
        document["files"].as_array_mut().unwrap().push(json!({
            "path": path,
            "size": raw.len(),
            "sha256": raw_sha.clone(),
            "excluded_from_hash_check": false,
            "temporary": false,
            "additional_check": false,
            "chunks": [raw_sha]
        }));
        unpacked_size += raw.len() as u64;
        download_size += compressed.len() as u64;
    }
    document["unpacked_size"] = json!(unpacked_size);
    document["download_size"] = json!(download_size);
    let manifest: ContentManifest = serde_json::from_value(document).unwrap();
    manifest.validate().unwrap();
    manifest
}

async fn persist_active_content_manifest(game: &std::path::Path, manifest: &ContentManifest) {
    atomic_json(
        &content_root(game)
            .join("manifests")
            .join(format!("{}.json", manifest.content_sha256)),
        manifest,
    )
    .await
    .unwrap();
    atomic_json(
        &state_path(game),
        &ContentCompletionState {
            schema_version: 1,
            transaction_id: None,
            content_sha256: manifest.content_sha256.clone(),
            release_id: manifest.release_id.clone(),
            game_version: manifest.game_version.clone(),
        },
    )
    .await
    .unwrap();
}

#[test]
fn content_manifest_and_http_fallback_are_strict() {
    let raw = b"loader";
    let compressed = encoded(raw);
    let document = manifest_json(raw, &compressed);
    let manifest: ContentManifest = serde_json::from_value(document.clone()).unwrap();
    manifest
        .validate()
        .expect("valid content closure should pass");

    assert!(parse_content_manifest_response(StatusCode::NOT_FOUND, b"")
        .expect("404 should be the fallback signal")
        .is_none());
    assert!(parse_content_manifest_response(StatusCode::INTERNAL_SERVER_ERROR, b"{}").is_err());
    let body = serde_json::to_vec(&document).unwrap();
    assert!(parse_content_manifest_response(StatusCode::OK, &body)
        .unwrap()
        .is_some());

    let mut unsafe_document = document;
    unsafe_document["files"][0]["path"] = json!("../outside.exe");
    let unsafe_manifest: ContentManifest = serde_json::from_value(unsafe_document).unwrap();
    assert!(unsafe_manifest.validate().is_err());
}

#[test]
fn content_drive_mirror_requires_exact_chunk_closure_and_builds_only_the_fixed_host() {
    let raw = b"loader";
    let compressed = encoded(raw);
    let manifest: ContentManifest =
        serde_json::from_value(manifest_json(raw, &compressed)).unwrap();
    let compressed_sha = sha256(&compressed);
    let mirror: ContentMirrorIndex = serde_json::from_value(json!({
        "schema_version": 1,
        "content_sha256": manifest.content_sha256,
        "source": "google_drive",
        "initial_concurrency": 2,
        "max_concurrency": 8,
        "chunks": { compressed_sha.clone(): "1O6eniBjd9dd1ES-j1OKuVRXmKL6ke4vE" }
    }))
    .unwrap();

    mirror.validate(&manifest).unwrap();
    assert_eq!(
        mirror.chunk_url(&compressed_sha).unwrap(),
        "https://drive.usercontent.google.com/download?id=1O6eniBjd9dd1ES-j1OKuVRXmKL6ke4vE&export=download"
    );

    let mut incomplete = mirror.clone();
    incomplete.chunks.clear();
    assert!(incomplete.validate(&manifest).is_err());
    let mut invalid_id = mirror;
    invalid_id
        .chunks
        .insert(compressed_sha, "https://attacker.invalid/chunk".into());
    assert!(invalid_id.validate(&manifest).is_err());

    assert!(
        parse_content_mirror_response(StatusCode::NOT_FOUND, b"", &manifest)
            .unwrap()
            .is_none()
    );
    assert!(
        parse_content_mirror_response(StatusCode::SERVICE_UNAVAILABLE, b"{}", &manifest).is_err()
    );
}

#[test]
fn content_adaptive_worker_bounds_are_deterministic() {
    assert_eq!(materializer_worker_limits(1, 256 * 1024 * 1024), (1, 1));
    assert_eq!(
        materializer_worker_limits(16, 8 * 1024 * 1024 * 1024),
        (2, 6)
    );

    let mut controller = AdaptiveDownloadController::new(2, 8);
    assert_eq!(controller.current(), 2);
    controller.observe_window(100.0, false, false, 0);
    assert_eq!(controller.current(), 3);
    controller.observe_window(80.0, false, false, 0);
    assert_eq!(controller.current(), 2);
    controller.observe_window(80.0, true, true, 0);
    assert_eq!(controller.current(), 1);

    let circuit = DriveCircuitBreaker::default();
    assert!(circuit.is_enabled());
    assert!(!circuit.register_failed_chunk());
    assert!(!circuit.register_failed_chunk());
    assert!(circuit.register_failed_chunk());
    assert!(!circuit.is_enabled());
}

#[tokio::test]
async fn content_materializer_waits_only_for_its_next_chunk() {
    let (sender, receiver) = tokio::sync::watch::channel(ChunkReadiness::Pending);
    let cancel = CancellationToken::new();
    let waiter = tokio::spawn(async move { wait_until_chunk_ready(receiver, &cancel).await });
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());
    sender.send(ChunkReadiness::Ready).unwrap();
    waiter.await.unwrap().unwrap();
}

#[tokio::test]
async fn content_chunk_checks_wire_raw_and_bounded_output_and_reuses_local_bytes() {
    let raw = b"abcdefgh";
    let compressed = encoded(raw);
    let chunk = ContentChunk {
        uncompressed_size: raw.len() as u64,
        compressed_size: compressed.len() as u64,
        compressed_sha256: sha256(&compressed),
    };
    assert_eq!(
        decode_verified_chunk(&compressed, &sha256(raw), &chunk).unwrap(),
        raw
    );

    let mut tampered = compressed.clone();
    tampered[0] ^= 1;
    assert!(decode_verified_chunk(&tampered, &sha256(raw), &chunk).is_err());
    let too_small = ContentChunk {
        uncompressed_size: 4,
        ..chunk.clone()
    };
    assert!(decode_verified_chunk(&compressed, &sha256(raw), &too_small).is_err());

    let directory = tempdir().unwrap();
    let source = directory.path().join("old.bin");
    tokio::fs::write(&source, b"prefixabcdefghsuffix")
        .await
        .unwrap();
    assert_eq!(
        read_verified_local_chunk(&source, 6, &sha256(raw), &chunk)
            .await
            .unwrap(),
        Some(raw.to_vec())
    );
}

#[tokio::test]
async fn content_download_resumes_an_existing_part_with_range() {
    let raw = b"resumable chunk";
    let compressed = encoded(raw);
    let chunk = ContentChunk {
        uncompressed_size: raw.len() as u64,
        compressed_size: compressed.len() as u64,
        compressed_sha256: sha256(&compressed),
    };
    let directory = tempdir().unwrap();
    let target = directory
        .path()
        .join(format!("{}.zst", chunk.compressed_sha256));
    let part = target.with_extension("zst.part");
    let split = 5;
    tokio::fs::write(&part, &compressed[..split]).await.unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let tail = compressed[split..].to_vec();
    let total = compressed.len();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
        assert!(request.contains("range: bytes=5-"));
        let end = total - 1;
        let response = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 5-{end}/{total}\r\nConnection: close\r\n\r\n",
            tail.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.write_all(&tail).await.unwrap();
    });

    download_content_chunk(
        &reqwest::Client::new(),
        &format!("http://{address}/chunk.zst"),
        &target,
        &chunk,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    server.await.unwrap();
    assert_eq!(tokio::fs::read(target).await.unwrap(), compressed);
}

#[tokio::test]
async fn release_1_6_12_recovery_preserves_chunks_and_parts_but_cleans_transaction_data() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let game = directory.path();
    let transaction_id = uuid::Uuid::new_v4().to_string();
    let journal = ContentJournal {
        schema_version: 2,
        transaction_id: transaction_id.clone(),
        release_id: "release-1".to_string(),
        content_sha256: "a".repeat(64),
        phase: ContentJournalPhase::Materialize,
        files: Vec::new(),
    };
    write_journal(game, &journal)
        .await
        .expect("journal should be written");
    let chunk = content_root(game).join("chunks/sha256/aa/chunk.zst");
    let part = content_root(game).join("chunks/sha256/bb/chunk.zst.part");
    let staging = staging_path(game, &transaction_id).join("pending.bin");
    for path in [&chunk, &part, &staging] {
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(path, b"fixture").await.unwrap();
    }

    let recovered = recover_pending_content(game)
        .await
        .expect("valid journal should recover");

    assert!(recovered);
    assert!(chunk.exists());
    assert!(part.exists());
    assert!(!staging_path(game, &transaction_id).exists());
    assert!(!journal_path(game).exists());
}

#[tokio::test]
async fn release_1_6_12_corrupt_journal_blocks_recovery_without_deleting_it() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = journal_path(directory.path());
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&path, b"not-json").await.unwrap();

    assert!(recover_pending_content(directory.path()).await.is_err());
    assert!(path.exists());
}

#[tokio::test]
async fn release_1_6_12_recovery_rolls_back_an_interrupted_commit() {
    let directory = tempdir().unwrap();
    let game = directory.path();
    let transaction_id = uuid::Uuid::new_v4().to_string();
    let backup = backup_path(game, &transaction_id).join("csgo/game.bin");
    let target = game.join("csgo/game.bin");
    tokio::fs::create_dir_all(backup.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::create_dir_all(target.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&backup, b"old").await.unwrap();
    tokio::fs::write(&target, b"new").await.unwrap();
    let added = game.join("added.bin");
    tokio::fs::write(&added, b"new").await.unwrap();
    let journal = ContentJournal {
        schema_version: 2,
        transaction_id,
        release_id: "1.0.3.4-r1".to_string(),
        content_sha256: "a".repeat(64),
        phase: ContentJournalPhase::Commit,
        files: vec![
            ContentJournalEntry {
                path: "csgo/game.bin".to_string(),
                action: ContentJournalAction::Replace,
                had_original: true,
            },
            ContentJournalEntry {
                path: "added.bin".to_string(),
                action: ContentJournalAction::Replace,
                had_original: false,
            },
        ],
    };

    write_journal(game, &journal).await.unwrap();
    assert!(recover_pending_content(game).await.unwrap());
    assert_eq!(tokio::fs::read(target).await.unwrap(), b"old");
    assert!(!tokio::fs::try_exists(added).await.unwrap());
}

#[tokio::test]
async fn release_1_6_12_completed_commit_keeps_files_and_only_cleans_transaction() {
    let directory = tempdir().unwrap();
    let game = directory.path();
    let transaction_id = uuid::Uuid::new_v4().to_string();
    let target = game.join("csgo/game.bin");
    let backup = backup_path(game, &transaction_id).join("csgo/game.bin");
    tokio::fs::create_dir_all(target.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::create_dir_all(backup.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&target, b"committed").await.unwrap();
    tokio::fs::write(&backup, b"old").await.unwrap();
    let journal = ContentJournal {
        schema_version: 2,
        transaction_id: transaction_id.clone(),
        release_id: "release-1".to_string(),
        content_sha256: "b".repeat(64),
        phase: ContentJournalPhase::Commit,
        files: vec![ContentJournalEntry {
            path: "csgo/game.bin".to_string(),
            action: ContentJournalAction::Replace,
            had_original: true,
        }],
    };
    write_journal(game, &journal).await.unwrap();
    atomic_json(
        &state_path(game),
        &ContentCompletionState {
            schema_version: 1,
            transaction_id: Some(transaction_id.clone()),
            content_sha256: journal.content_sha256.clone(),
            release_id: journal.release_id.clone(),
            game_version: "1.0.3.4".to_string(),
        },
    )
    .await
    .unwrap();

    assert!(recover_pending_content(game).await.unwrap());
    assert_eq!(tokio::fs::read(target).await.unwrap(), b"committed");
    assert!(!backup_path(game, &transaction_id).exists());
    assert!(!journal_path(game).exists());
}

#[tokio::test]
async fn release_1_6_12_old_matching_state_does_not_complete_a_new_transaction() {
    let directory = tempdir().unwrap();
    let game = directory.path();
    let previous_transaction = uuid::Uuid::new_v4().to_string();
    let transaction_id = uuid::Uuid::new_v4().to_string();
    let target = game.join("csgo/game.bin");
    let backup = backup_path(game, &transaction_id).join("csgo/game.bin");
    tokio::fs::create_dir_all(target.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::create_dir_all(backup.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&target, b"partially-committed")
        .await
        .unwrap();
    tokio::fs::write(&backup, b"old").await.unwrap();
    let journal = ContentJournal {
        schema_version: 2,
        transaction_id: transaction_id.clone(),
        release_id: "release-1".to_string(),
        content_sha256: "c".repeat(64),
        phase: ContentJournalPhase::Commit,
        files: vec![ContentJournalEntry {
            path: "csgo/game.bin".to_string(),
            action: ContentJournalAction::Replace,
            had_original: true,
        }],
    };
    write_journal(game, &journal).await.unwrap();
    atomic_json(
        &state_path(game),
        &ContentCompletionState {
            schema_version: 1,
            transaction_id: Some(previous_transaction),
            content_sha256: journal.content_sha256.clone(),
            release_id: journal.release_id.clone(),
            game_version: "1.0.3.4".to_string(),
        },
    )
    .await
    .unwrap();

    assert!(recover_pending_content(game).await.unwrap());
    assert_eq!(tokio::fs::read(target).await.unwrap(), b"old");
}

#[test]
fn content_disk_space_counts_only_unique_missing_chunks_and_checks_overflow() {
    let raw = b"loader";
    let compressed = encoded(raw);
    let manifest: ContentManifest =
        serde_json::from_value(manifest_json(raw, &compressed)).unwrap();
    let required =
        required_content_install_bytes(&manifest, &HashSet::new(), 100, 50, 0, 25).unwrap();
    assert_eq!(required, compressed.len() as u64 + 175);

    let cached = HashSet::from([sha256(raw)]);
    assert_eq!(
        required_content_install_bytes(&manifest, &cached, 100, 50, 0, 25).unwrap(),
        175
    );
    assert!(required_content_install_bytes(&manifest, &HashSet::new(), u64::MAX, 1, 0, 0).is_err());
}

#[tokio::test]
async fn content_deletion_exact_obsolete_file_is_backed_up_and_unknown_file_is_preserved() {
    let directory = tempdir().unwrap();
    let game = directory.path();
    let previous = content_deletion_manifest(
        'c',
        "1.0.3.4-r1",
        "1.0.3.4",
        &[("legacy/obsolete.bin", b"managed-old")],
    );
    let next = content_deletion_manifest('d', "1.0.3.5-r1", "1.0.3.5", &[]);
    persist_active_content_manifest(game, &previous).await;
    let obsolete = game.join("legacy/obsolete.bin");
    let unknown = game.join("legacy/user-notes.txt");
    tokio::fs::create_dir_all(obsolete.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&obsolete, b"managed-old").await.unwrap();
    tokio::fs::write(&unknown, b"keep-me").await.unwrap();

    let entries = load_obsolete_content_entries(game, &next).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "legacy/obsolete.bin");
    assert_eq!(entries[0].action, ContentJournalAction::Remove);
    let transaction_id = uuid::Uuid::new_v4().to_string();
    let journal = ContentJournal {
        schema_version: 2,
        transaction_id: transaction_id.clone(),
        release_id: next.release_id.clone(),
        content_sha256: next.content_sha256.clone(),
        phase: ContentJournalPhase::Commit,
        files: entries,
    };

    commit_staged_files(game, &staging_path(game, &transaction_id), &journal)
        .await
        .unwrap();
    cleanup_obsolete_directories(game, &journal).await.unwrap();

    assert!(!obsolete.exists());
    assert_eq!(tokio::fs::read(unknown).await.unwrap(), b"keep-me");
    assert_eq!(
        tokio::fs::read(backup_path(game, &transaction_id).join("legacy/obsolete.bin"))
            .await
            .unwrap(),
        b"managed-old"
    );
}

#[tokio::test]
async fn content_deletion_missing_invalid_or_mismatched_previous_manifest_deletes_nothing() {
    let directory = tempdir().unwrap();
    let game = directory.path();
    let next = content_deletion_manifest('e', "1.0.3.5-r1", "1.0.3.5", &[]);
    let candidate = game.join("legacy/old.bin");
    tokio::fs::create_dir_all(candidate.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&candidate, b"do-not-infer-ownership")
        .await
        .unwrap();

    assert!(load_obsolete_content_entries(game, &next)
        .await
        .unwrap()
        .is_empty());

    tokio::fs::create_dir_all(content_root(game)).await.unwrap();
    tokio::fs::write(state_path(game), b"invalid-json")
        .await
        .unwrap();
    assert!(load_obsolete_content_entries(game, &next)
        .await
        .unwrap()
        .is_empty());

    let previous_hash = "f".repeat(64);
    atomic_json(
        &state_path(game),
        &ContentCompletionState {
            schema_version: 1,
            transaction_id: None,
            content_sha256: previous_hash.clone(),
            release_id: "1.0.3.4-r1".into(),
            game_version: "1.0.3.4".into(),
        },
    )
    .await
    .unwrap();
    assert!(load_obsolete_content_entries(game, &next)
        .await
        .unwrap()
        .is_empty());
    tokio::fs::create_dir_all(content_root(game).join("manifests"))
        .await
        .unwrap();
    tokio::fs::write(
        content_root(game)
            .join("manifests")
            .join(format!("{previous_hash}.json")),
        b"invalid-manifest",
    )
    .await
    .unwrap();
    assert!(load_obsolete_content_entries(game, &next)
        .await
        .unwrap()
        .is_empty());

    let previous = content_deletion_manifest(
        'f',
        "1.0.3.4-r1",
        "1.0.3.4",
        &[("legacy/old.bin", b"managed-old")],
    );
    persist_active_content_manifest(game, &previous).await;
    let mut mismatched_state: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(state_path(game)).await.unwrap()).unwrap();
    mismatched_state["release_id"] = json!("1.0.3.4-r2");
    atomic_json(&state_path(game), &mismatched_state)
        .await
        .unwrap();
    assert!(load_obsolete_content_entries(game, &next)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        tokio::fs::read(candidate).await.unwrap(),
        b"do-not-infer-ownership"
    );
}

#[tokio::test]
async fn content_deletion_interrupted_recovery_restores_removal_and_preserves_chunks_and_parts() {
    let directory = tempdir().unwrap();
    let game = directory.path();
    let transaction_id = uuid::Uuid::new_v4().to_string();
    let target = game.join("legacy/obsolete.bin");
    let backup = backup_path(game, &transaction_id).join("legacy/obsolete.bin");
    tokio::fs::create_dir_all(target.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::create_dir_all(backup.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&target, b"managed-old").await.unwrap();
    tokio::fs::rename(&target, &backup).await.unwrap();
    let chunk = content_root(game).join("chunks/aa/chunk.zst");
    let part = content_root(game).join("chunks/bb/chunk.zst.part");
    for path in [&chunk, &part] {
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(path, b"cache").await.unwrap();
    }
    atomic_json(
        &journal_path(game),
        &json!({
            "schema_version": 2,
            "transaction_id": transaction_id,
            "release_id": "1.0.3.5-r1",
            "content_sha256": "a".repeat(64),
            "phase": "commit",
            "files": [{
                "path": "legacy/obsolete.bin",
                "action": "remove"
            }]
        }),
    )
    .await
    .unwrap();

    assert!(recover_pending_content(game).await.unwrap());
    assert_eq!(tokio::fs::read(target).await.unwrap(), b"managed-old");
    assert!(chunk.exists());
    assert!(part.exists());
}

#[tokio::test]
async fn content_deletion_committed_recovery_keeps_deletion_cleans_backup_and_only_safe_empty_dirs()
{
    let directory = tempdir().unwrap();
    let game = directory.path();
    let transaction_id = uuid::Uuid::new_v4().to_string();
    let backup = backup_path(game, &transaction_id).join("known/nested/obsolete.bin");
    tokio::fs::create_dir_all(backup.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&backup, b"managed-old").await.unwrap();
    tokio::fs::create_dir_all(game.join("known/nested"))
        .await
        .unwrap();
    tokio::fs::write(game.join("known/user.txt"), b"keep-me")
        .await
        .unwrap();
    tokio::fs::create_dir_all(game.join("unrelated-empty"))
        .await
        .unwrap();
    atomic_json(
        &journal_path(game),
        &json!({
            "schema_version": 2,
            "transaction_id": transaction_id,
            "release_id": "1.0.3.5-r1",
            "content_sha256": "b".repeat(64),
            "phase": "commit",
            "files": [{
                "path": "known/nested/obsolete.bin",
                "action": "remove"
            }]
        }),
    )
    .await
    .unwrap();
    atomic_json(
        &state_path(game),
        &ContentCompletionState {
            schema_version: 1,
            transaction_id: Some(transaction_id.clone()),
            content_sha256: "b".repeat(64),
            release_id: "1.0.3.5-r1".into(),
            game_version: "1.0.3.5".into(),
        },
    )
    .await
    .unwrap();

    assert!(recover_pending_content(game).await.unwrap());
    assert!(!game.join("known/nested/obsolete.bin").exists());
    assert!(!game.join("known/nested").exists());
    assert_eq!(
        tokio::fs::read(game.join("known/user.txt")).await.unwrap(),
        b"keep-me"
    );
    assert!(game.join("unrelated-empty").exists());
    assert!(!backup_path(game, &transaction_id).exists());
    assert!(!journal_path(game).exists());
}

#[tokio::test]
async fn content_deletion_v1_journal_recovery_remains_compatible() {
    let directory = tempdir().unwrap();
    let game = directory.path();
    let transaction_id = uuid::Uuid::new_v4().to_string();
    let target = game.join("csgo/game.bin");
    let backup = backup_path(game, &transaction_id).join("csgo/game.bin");
    tokio::fs::create_dir_all(target.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::create_dir_all(backup.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&target, b"new").await.unwrap();
    tokio::fs::write(&backup, b"old").await.unwrap();
    atomic_json(
        &journal_path(game),
        &json!({
            "schema_version": 1,
            "transaction_id": transaction_id,
            "release_id": "1.0.3.4-r1",
            "content_sha256": "c".repeat(64),
            "phase": "commit",
            "files": [{"path": "csgo/game.bin", "had_original": true}]
        }),
    )
    .await
    .unwrap();

    assert!(recover_pending_content(game).await.unwrap());
    assert_eq!(tokio::fs::read(target).await.unwrap(), b"old");
}

#[tokio::test]
async fn content_deletion_malformed_or_unsafe_journal_paths_are_rejected() {
    let directory = tempdir().unwrap();
    let game = directory.path().join("game");
    let outside = directory.path().join("outside.bin");
    tokio::fs::write(&outside, b"keep-me").await.unwrap();
    for unsafe_path in [
        "../outside.bin",
        r"C:\outside.bin",
        ".zhekarik/content/state.json",
    ] {
        atomic_json(
            &journal_path(&game),
            &json!({
                "schema_version": 2,
                "transaction_id": uuid::Uuid::new_v4().to_string(),
                "release_id": "1.0.3.5-r1",
                "content_sha256": "d".repeat(64),
                "phase": "commit",
                "files": [{"path": unsafe_path, "action": "remove"}]
            }),
        )
        .await
        .unwrap();
        assert!(recover_pending_content(&game).await.is_err());
    }
    atomic_json(
        &journal_path(&game),
        &json!({
            "schema_version": 2,
            "transaction_id": uuid::Uuid::new_v4().to_string(),
            "release_id": "1.0.3.5-r1",
            "content_sha256": "d".repeat(64),
            "phase": "commit",
            "files": [{"path": "legacy/obsolete.bin"}]
        }),
    )
    .await
    .unwrap();
    assert!(recover_pending_content(&game).await.is_err());
    assert_eq!(tokio::fs::read(outside).await.unwrap(), b"keep-me");
}

#[test]
fn content_deletion_disk_preflight_includes_obsolete_backup_and_checks_overflow() {
    let manifest = content_deletion_manifest('e', "1.0.3.5-r1", "1.0.3.5", &[]);
    let available = manifest.chunks.keys().cloned().collect::<HashSet<_>>();

    assert_eq!(
        required_content_install_bytes(&manifest, &available, 10, 20, 30, 40).unwrap(),
        100
    );
    assert!(required_content_install_bytes(&manifest, &available, 0, 1, u64::MAX, 0).is_err());
}
