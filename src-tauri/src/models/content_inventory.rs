use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::models::{
    legacy_json_identity, ContentChunk, ContentChunking, ContentCompression, ContentFile,
    ContentManifest, DrivePackManifest,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentInventory {
    pub schema_version: u8,
    pub content_sha256: String,
    pub release_id: String,
    pub game_version: String,
    pub generated_at: String,
    pub source_archive_sha256: String,
    pub download_size: u64,
    pub unpacked_size: u64,
    pub chunking: ContentChunking,
    pub compression: ContentCompression,
    pub chunks: BTreeMap<String, ContentChunk>,
    pub files: Vec<ContentFile>,
}

impl ContentInventory {
    pub fn from_v3(manifest: &DrivePackManifest) -> Result<Self, AppError> {
        manifest.validate()?;
        let inventory = Self {
            schema_version: 1,
            content_sha256: manifest.content_sha256.clone(),
            release_id: manifest.release_id.clone(),
            game_version: manifest.game_version.clone(),
            generated_at: manifest.generated_at.clone(),
            source_archive_sha256: manifest.source_archive_sha256.clone(),
            download_size: manifest.download_size,
            unpacked_size: manifest.unpacked_size,
            chunking: manifest.chunking.clone(),
            compression: manifest.compression.clone(),
            chunks: manifest
                .chunks
                .iter()
                .map(|(raw_sha, chunk)| (raw_sha.clone(), ContentChunk::from(chunk)))
                .collect(),
            files: manifest.files.clone(),
        };
        inventory.validate()?;
        Ok(inventory)
    }

    pub fn from_v2(manifest: &ContentManifest) -> Result<Self, AppError> {
        manifest.validate()?;
        let inventory = Self {
            schema_version: 1,
            content_sha256: manifest.content_sha256.clone(),
            release_id: manifest.release_id.clone(),
            game_version: manifest.game_version.clone(),
            generated_at: manifest.generated_at.clone(),
            source_archive_sha256: manifest.source_archive_sha256.clone(),
            download_size: manifest.download_size,
            unpacked_size: manifest.unpacked_size,
            chunking: manifest.chunking.clone(),
            compression: manifest.compression.clone(),
            chunks: manifest
                .chunks
                .iter()
                .map(|(raw_sha, chunk)| (raw_sha.clone(), chunk.clone()))
                .collect(),
            files: manifest.files.clone(),
        };
        inventory.validate()?;
        Ok(inventory)
    }

    pub fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != 1 {
            return Err(AppError::InvalidData(
                "unsupported content inventory schema".into(),
            ));
        }
        let legacy = self.legacy_projection();
        if legacy_json_identity(&legacy)? != self.content_sha256 {
            return Err(AppError::InvalidData(
                "content inventory identity is invalid".into(),
            ));
        }
        self.as_v2_manifest().validate()
    }

    pub fn as_v2_manifest(&self) -> ContentManifest {
        ContentManifest {
            schema_version: 2,
            content_sha256: self.content_sha256.clone(),
            release_id: self.release_id.clone(),
            game_version: self.game_version.clone(),
            generated_at: self.generated_at.clone(),
            source_archive_sha256: self.source_archive_sha256.clone(),
            delivery: crate::models::ContentDelivery {
                chunk_base_url: "https://api.zhekarik.africa/launcher/game/v2/chunks".into(),
                recommended_concurrency: 1,
            },
            chunking: self.chunking.clone(),
            compression: self.compression.clone(),
            download_size: self.download_size,
            unpacked_size: self.unpacked_size,
            chunks: self
                .chunks
                .iter()
                .map(|(raw_sha, chunk)| (raw_sha.clone(), chunk.clone()))
                .collect(),
            files: self.files.clone(),
        }
    }

    fn legacy_projection(&self) -> Value {
        json!({
            "schema_version": 2,
            "release_id": self.release_id,
            "game_version": self.game_version,
            "generated_at": self.generated_at,
            "source_archive_sha256": self.source_archive_sha256,
            "chunking": self.chunking,
            "compression": self.compression,
            "download_size": self.download_size,
            "unpacked_size": self.unpacked_size,
            "chunks": self.chunks,
            "files": self.files,
        })
    }
}
