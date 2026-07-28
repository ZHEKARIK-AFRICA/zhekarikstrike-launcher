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
    materializer_worker_limits, required_content_install_bytes, wait_until_chunk_ready,
    AdaptiveDownloadController, ChunkReadiness,
};
use crate::services::content_journal_service::{
    recover_interrupted_commit, ContentJournal, ContentJournalEntry, ContentJournalPhase,
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
async fn content_journal_rolls_back_an_interrupted_commit() {
    let directory = tempdir().unwrap();
    let game = directory.path();
    let backup = game.join(".zhekarik/content/backup/tx/csgo/game.bin");
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
        schema_version: 1,
        transaction_id: "tx".to_string(),
        release_id: "1.0.3.4-r1".to_string(),
        content_sha256: "a".repeat(64),
        phase: ContentJournalPhase::Commit,
        files: vec![
            ContentJournalEntry {
                path: "csgo/game.bin".to_string(),
                had_original: true,
            },
            ContentJournalEntry {
                path: "added.bin".to_string(),
                had_original: false,
            },
        ],
    };

    recover_interrupted_commit(game, &journal).await.unwrap();
    assert_eq!(tokio::fs::read(target).await.unwrap(), b"old");
    assert!(!tokio::fs::try_exists(added).await.unwrap());
}

#[test]
fn content_disk_space_counts_only_unique_missing_chunks_and_checks_overflow() {
    let raw = b"loader";
    let compressed = encoded(raw);
    let manifest: ContentManifest =
        serde_json::from_value(manifest_json(raw, &compressed)).unwrap();
    let required = required_content_install_bytes(&manifest, &HashSet::new(), 100, 50, 25).unwrap();
    assert_eq!(required, compressed.len() as u64 + 175);

    let cached = HashSet::from([sha256(raw)]);
    assert_eq!(
        required_content_install_bytes(&manifest, &cached, 100, 50, 25).unwrap(),
        175
    );
    assert!(required_content_install_bytes(&manifest, &HashSet::new(), u64::MAX, 1, 0,).is_err());
}
