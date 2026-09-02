use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::DateTime;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::models::{
    validate_game_path, ContentChunk, ContentChunking, ContentCompression, ContentFile,
    GameManifest, CONTENT_CHUNK_SIZE,
};

pub const DRIVE_PACK_MAX_SIZE: u64 = 64 * 1024 * 1024;
pub const DRIVE_PACK_REPLICA_COUNT: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrivePackProfile {
    pub name: String,
    pub max_pack_size: u64,
    pub replica_count: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrivePack {
    pub size: u64,
    pub replica_file_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackedContentChunk {
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub compressed_sha256: String,
    pub pack_sha256: String,
    pub offset: u64,
}

impl From<&PackedContentChunk> for ContentChunk {
    fn from(value: &PackedContentChunk) -> Self {
        Self {
            uncompressed_size: value.uncompressed_size,
            compressed_size: value.compressed_size,
            compressed_sha256: value.compressed_sha256.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrivePackManifest {
    pub schema_version: u8,
    pub manifest_sha256: String,
    pub content_sha256: String,
    pub release_id: String,
    pub game_version: String,
    pub generated_at: String,
    pub source_archive_sha256: String,
    pub download_size: u64,
    pub unpacked_size: u64,
    pub chunking: ContentChunking,
    pub compression: ContentCompression,
    pub pack_profile: DrivePackProfile,
    pub packs: BTreeMap<String, DrivePack>,
    pub chunks: BTreeMap<String, PackedContentChunk>,
    pub files: Vec<ContentFile>,
}

impl DrivePackManifest {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != 3 {
            return invalid("unsupported content pack manifest schema");
        }
        validate_sha256(&self.manifest_sha256, "pack manifest")?;
        validate_sha256(&self.content_sha256, "content")?;
        validate_sha256(&self.source_archive_sha256, "source archive")?;
        validate_release(&self.game_version, &self.release_id)?;
        DateTime::parse_from_rfc3339(&self.generated_at)
            .map_err(|_| AppError::InvalidData("invalid content generated_at".into()))?;

        if self.chunking.profile != "fixed-v1"
            || self.chunking.chunk_size != CONTENT_CHUNK_SIZE
            || self.compression.profile != "zstd-v1"
            || self.compression.level != 6
            || !self.compression.frame_checksum
        {
            return invalid("unsupported packed content encoding profile");
        }
        if self.pack_profile.name != "drive-pack-v1"
            || self.pack_profile.max_pack_size != DRIVE_PACK_MAX_SIZE
            || self.pack_profile.replica_count as usize != DRIVE_PACK_REPLICA_COUNT
        {
            return invalid("unsupported Google Drive pack profile");
        }
        if self.packs.is_empty() || self.chunks.is_empty() || self.files.is_empty() {
            return invalid("content pack manifest is empty");
        }

        let mut spans_by_pack = self
            .packs
            .keys()
            .map(|pack_sha| (pack_sha.clone(), Vec::new()))
            .collect::<BTreeMap<_, Vec<(u64, u64)>>>();
        let mut packed_download_size = 0_u64;
        for (pack_sha, pack) in &self.packs {
            validate_sha256(pack_sha, "content pack")?;
            if pack.size == 0 || pack.size > DRIVE_PACK_MAX_SIZE {
                return invalid("invalid content pack size");
            }
            packed_download_size = packed_download_size
                .checked_add(pack.size)
                .ok_or_else(|| AppError::InvalidData("content pack size overflow".into()))?;
            let replicas = pack.replica_file_ids.iter().collect::<HashSet<_>>();
            if pack.replica_file_ids.len() != DRIVE_PACK_REPLICA_COUNT
                || replicas.len() != DRIVE_PACK_REPLICA_COUNT
                || pack
                    .replica_file_ids
                    .iter()
                    .any(|file_id| !valid_drive_id(file_id))
            {
                return invalid("invalid Google Drive pack replicas");
            }
        }

        let mut compressed_download_size = 0_u64;
        for (raw_sha, chunk) in &self.chunks {
            validate_sha256(raw_sha, "raw content chunk")?;
            validate_sha256(&chunk.compressed_sha256, "compressed content chunk")?;
            validate_sha256(&chunk.pack_sha256, "content pack")?;
            if chunk.uncompressed_size == 0
                || chunk.uncompressed_size > CONTENT_CHUNK_SIZE
                || chunk.compressed_size == 0
            {
                return invalid("invalid packed content chunk size");
            }
            let pack = self.packs.get(&chunk.pack_sha256).ok_or_else(|| {
                AppError::InvalidData("content chunk references a missing pack".into())
            })?;
            let end = chunk
                .offset
                .checked_add(chunk.compressed_size)
                .ok_or_else(|| AppError::InvalidData("content pack span overflow".into()))?;
            if end > pack.size {
                return invalid("content chunk exceeds its pack");
            }
            spans_by_pack
                .get_mut(&chunk.pack_sha256)
                .expect("validated pack key must exist")
                .push((chunk.offset, chunk.compressed_size));
            compressed_download_size = compressed_download_size
                .checked_add(chunk.compressed_size)
                .ok_or_else(|| AppError::InvalidData("content download size overflow".into()))?;
        }

        for (pack_sha, spans) in &mut spans_by_pack {
            spans.sort_unstable();
            let mut expected_offset = 0_u64;
            for (offset, size) in spans.iter() {
                if *offset != expected_offset {
                    return invalid("content pack spans contain a gap or overlap");
                }
                expected_offset = expected_offset
                    .checked_add(*size)
                    .ok_or_else(|| AppError::InvalidData("content pack span overflow".into()))?;
            }
            if expected_offset != self.packs[pack_sha].size {
                return invalid("content pack spans do not cover the pack");
            }
        }

        let mut paths = HashSet::new();
        let mut used_chunks = BTreeSet::new();
        let mut unpacked_size = 0_u64;
        let mut has_loader = false;
        for file in &self.files {
            if !file.path.is_ascii() {
                return invalid("content manifest paths must be ASCII");
            }
            validate_game_path(&file.path)?;
            if !paths.insert(file.path.to_ascii_lowercase()) {
                return invalid("duplicate content file path");
            }
            validate_sha256(&file.sha256, &file.path)?;
            let mut file_size = 0_u64;
            for (index, raw_sha) in file.chunks.iter().enumerate() {
                validate_sha256(raw_sha, &file.path)?;
                let chunk = self
                    .chunks
                    .get(raw_sha)
                    .ok_or_else(|| AppError::InvalidData("missing packed content chunk".into()))?;
                if index + 1 < file.chunks.len() && chunk.uncompressed_size != CONTENT_CHUNK_SIZE {
                    return invalid("non-final content chunk has an invalid size");
                }
                file_size = file_size
                    .checked_add(chunk.uncompressed_size)
                    .ok_or_else(|| AppError::InvalidData("content file size overflow".into()))?;
                used_chunks.insert(raw_sha.clone());
            }
            if file_size != file.size {
                return invalid("content file chunk closure is invalid");
            }
            unpacked_size = unpacked_size
                .checked_add(file.size)
                .ok_or_else(|| AppError::InvalidData("content unpacked size overflow".into()))?;
            has_loader |= file.path == "RevLoader.exe";
        }
        if !has_loader
            || unpacked_size != self.unpacked_size
            || used_chunks.len() != self.chunks.len()
            || used_chunks
                .iter()
                .any(|raw_sha| !self.chunks.contains_key(raw_sha))
            || compressed_download_size != self.download_size
            || packed_download_size != self.download_size
        {
            return invalid("packed content manifest closure is invalid");
        }

        let legacy = self.legacy_content_projection()?;
        if legacy_json_identity(&legacy)? != self.content_sha256 {
            return invalid("legacy content identity does not match v3");
        }
        let mut manifest_value = serde_json::to_value(self)?;
        manifest_value
            .as_object_mut()
            .expect("manifest serializes as an object")
            .remove("manifest_sha256");
        let canonical = serde_json_canonicalizer::to_vec(&manifest_value).map_err(|error| {
            AppError::InvalidData(format!(
                "content pack manifest cannot be canonicalized: {error}"
            ))
        })?;
        if sha256_hex(&canonical) != self.manifest_sha256 {
            return invalid("canonical content pack identity is invalid");
        }
        Ok(())
    }

    pub fn validate_against_v1(&self, v1: &GameManifest) -> Result<(), AppError> {
        v1.validate_complete()?;
        if self.game_version != v1.game_version
            || self.generated_at != v1.generated_at
            || self.source_archive_sha256 != v1.archive.sha256
            || self.unpacked_size != v1.archive.unpacked_size
            || self.files.len() != v1.files.len()
        {
            return invalid("content pack manifest does not match canonical v1 metadata");
        }
        let expected = v1
            .files
            .iter()
            .map(|file| (file.path.to_ascii_lowercase(), file))
            .collect::<HashMap<_, _>>();
        for file in &self.files {
            let Some(original) = expected.get(&file.path.to_ascii_lowercase()) else {
                return invalid("content pack file is absent from canonical v1 metadata");
            };
            if file.path != original.path
                || file.size != original.size
                || file.sha256 != original.sha256
                || file.excluded_from_hash_check != original.excluded_from_hash_check
                || file.temporary != original.temporary
            {
                return invalid("content pack file differs from canonical v1 metadata");
            }
        }
        Ok(())
    }

    pub fn legacy_content_projection(&self) -> Result<Value, AppError> {
        let chunks = self
            .chunks
            .iter()
            .map(|(raw_sha, chunk)| {
                (
                    raw_sha.clone(),
                    json!({
                        "uncompressed_size": chunk.uncompressed_size,
                        "compressed_size": chunk.compressed_size,
                        "compressed_sha256": chunk.compressed_sha256,
                    }),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        Ok(json!({
            "schema_version": 2,
            "release_id": self.release_id,
            "game_version": self.game_version,
            "generated_at": self.generated_at,
            "source_archive_sha256": self.source_archive_sha256,
            "chunking": self.chunking,
            "compression": self.compression,
            "download_size": self.download_size,
            "unpacked_size": self.unpacked_size,
            "chunks": chunks,
            "files": self.files,
        }))
    }

    pub fn drive_url(file_id: &str) -> Result<Url, AppError> {
        if !valid_drive_id(file_id) {
            return invalid("invalid Google Drive pack file identifier");
        }
        Url::parse(&format!(
            "https://drive.usercontent.google.com/download?id={file_id}&export=download&confirm=t"
        ))
        .map_err(|error| AppError::InvalidData(format!("invalid Google Drive pack URL: {error}")))
    }
}

pub fn legacy_json_identity(value: &Value) -> Result<String, AppError> {
    let sorted = recursively_sorted(value);
    let mut bytes = serde_json::to_vec(&sorted)?;
    bytes.push(b'\n');
    Ok(sha256_hex(&bytes))
}

fn recursively_sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut sorted = serde_json::Map::new();
            for key in keys {
                sorted.insert(key.clone(), recursively_sorted(&map[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(recursively_sorted).collect()),
        other => other.clone(),
    }
}

fn validate_release(game_version: &str, release_id: &str) -> Result<(), AppError> {
    let parts = game_version.split('.').collect::<Vec<_>>();
    let version_valid = matches!(parts.len(), 3 | 4)
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    let revision_valid = release_id
        .strip_prefix(&format!("{game_version}-r"))
        .is_some_and(|revision| {
            !revision.is_empty()
                && !revision.starts_with('0')
                && revision.bytes().all(|byte| byte.is_ascii_digit())
        });
    if !version_valid || !revision_valid {
        return invalid("invalid packed content release identifier");
    }
    Ok(())
}

pub fn validate_sha256(value: &str, label: &str) -> Result<(), AppError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid(&format!("invalid SHA-256 for {label}"));
    }
    Ok(())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn valid_drive_id(value: &str) -> bool {
    (10..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn invalid<T>(message: &str) -> Result<T, AppError> {
    Err(AppError::InvalidData(message.to_string()))
}
