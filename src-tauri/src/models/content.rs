use std::collections::{HashMap, HashSet};

use chrono::DateTime;
use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::models::validate_game_path;

pub const CONTENT_CHUNK_SIZE: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentDelivery {
    pub chunk_base_url: String,
    pub recommended_concurrency: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentChunking {
    pub profile: String,
    pub chunk_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentCompression {
    pub profile: String,
    pub level: i32,
    pub frame_checksum: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentChunk {
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub compressed_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub excluded_from_hash_check: bool,
    pub temporary: bool,
    pub additional_check: bool,
    pub chunks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentManifest {
    pub schema_version: u8,
    pub content_sha256: String,
    pub release_id: String,
    pub game_version: String,
    pub generated_at: String,
    pub source_archive_sha256: String,
    pub delivery: ContentDelivery,
    pub chunking: ContentChunking,
    pub compression: ContentCompression,
    pub download_size: u64,
    pub unpacked_size: u64,
    pub chunks: HashMap<String, ContentChunk>,
    pub files: Vec<ContentFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentMirrorIndex {
    pub schema_version: u8,
    pub content_sha256: String,
    pub source: String,
    pub initial_concurrency: usize,
    pub max_concurrency: usize,
    pub chunks: HashMap<String, String>,
}

impl ContentMirrorIndex {
    pub fn validate(&self, manifest: &ContentManifest) -> Result<(), AppError> {
        if self.schema_version != 1
            || self.source != "google_drive"
            || self.content_sha256 != manifest.content_sha256
            || !(1..=8).contains(&self.initial_concurrency)
            || !(self.initial_concurrency..=8).contains(&self.max_concurrency)
        {
            return invalid("invalid Google Drive content mirror settings");
        }

        let expected = manifest
            .chunks
            .values()
            .map(|chunk| chunk.compressed_sha256.as_str())
            .collect::<HashSet<_>>();
        if self.chunks.len() != expected.len()
            || self
                .chunks
                .keys()
                .any(|compressed_sha| !expected.contains(compressed_sha.as_str()))
        {
            return invalid("Google Drive content mirror closure is invalid");
        }
        for (compressed_sha, file_id) in &self.chunks {
            validate_sha256(compressed_sha, "mirrored compressed chunk")?;
            if !(10..=128).contains(&file_id.len())
                || !file_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            {
                return invalid("invalid Google Drive content file identifier");
            }
        }
        Ok(())
    }

    pub fn chunk_url(&self, compressed_sha256: &str) -> Result<String, AppError> {
        validate_sha256(compressed_sha256, "mirrored compressed chunk")?;
        let file_id = self
            .chunks
            .get(compressed_sha256)
            .ok_or_else(|| AppError::InvalidData("missing Google Drive content chunk".into()))?;
        Ok(format!(
            "https://drive.usercontent.google.com/download?id={file_id}&export=download&confirm=t"
        ))
    }
}

impl ContentManifest {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != 2 {
            return invalid("unsupported content manifest schema");
        }
        validate_sha256(&self.content_sha256, "content")?;
        validate_sha256(&self.source_archive_sha256, "source archive")?;
        if !valid_game_version(&self.game_version)
            || self
                .release_id
                .strip_prefix(&format!("{}-r", self.game_version))
                .is_none_or(|revision| {
                    revision.is_empty()
                        || revision.starts_with('0')
                        || !revision.bytes().all(|byte| byte.is_ascii_digit())
                })
        {
            return invalid("invalid content release identifier");
        }
        DateTime::parse_from_rfc3339(&self.generated_at)
            .map_err(|_| AppError::InvalidData("invalid content generated_at".into()))?;

        let base = Url::parse(&self.delivery.chunk_base_url)
            .map_err(|_| AppError::InvalidData("invalid content chunk base URL".into()))?;
        let trusted_host = base.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("zhekarik.africa")
                || host.to_ascii_lowercase().ends_with(".zhekarik.africa")
        });
        if base.scheme() != "https"
            || !trusted_host
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || self.delivery.chunk_base_url.ends_with('/')
            || !(1..=8).contains(&self.delivery.recommended_concurrency)
        {
            return invalid("untrusted content delivery settings");
        }
        if self.chunking.profile != "fixed-v1" || self.chunking.chunk_size != CONTENT_CHUNK_SIZE {
            return invalid("unsupported content chunking profile");
        }
        if self.compression.profile != "zstd-v1"
            || self.compression.level != 6
            || !self.compression.frame_checksum
        {
            return invalid("unsupported content compression profile");
        }
        if self.files.is_empty() || self.chunks.is_empty() {
            return invalid("content manifest is empty");
        }

        let mut paths = HashSet::new();
        let mut used_chunks = HashSet::new();
        let mut unpacked_size = 0_u64;
        let mut has_loader = false;
        for (raw_sha, chunk) in &self.chunks {
            validate_sha256(raw_sha, "raw chunk")?;
            validate_sha256(&chunk.compressed_sha256, "compressed chunk")?;
            if chunk.uncompressed_size == 0
                || chunk.uncompressed_size > CONTENT_CHUNK_SIZE
                || chunk.compressed_size == 0
            {
                return invalid("invalid content chunk size");
            }
        }
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
                    .ok_or_else(|| AppError::InvalidData("missing content chunk".into()))?;
                if index + 1 < file.chunks.len()
                    && chunk.uncompressed_size != self.chunking.chunk_size
                {
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
        {
            return invalid("content manifest closure is invalid");
        }
        let download_size = self.chunks.values().try_fold(0_u64, |total, chunk| {
            total
                .checked_add(chunk.compressed_size)
                .ok_or_else(|| AppError::InvalidData("content download size overflow".into()))
        })?;
        if download_size != self.download_size {
            return invalid("content download size is invalid");
        }
        Ok(())
    }
}

fn valid_game_version(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    matches!(parts.len(), 3 | 4)
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn validate_sha256(value: &str, label: &str) -> Result<(), AppError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid(&format!("invalid SHA-256 for {label}"));
    }
    Ok(())
}

fn invalid<T>(message: &str) -> Result<T, AppError> {
    Err(AppError::InvalidData(message.to_string()))
}
