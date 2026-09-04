//! Append-only generations and streaming compressed-chunk verification.
use super::{content_pack_cache_service::PackCache, content_pack_metrics::PackMetrics};
use crate::{
    error::AppError,
    models::{DrivePackManifest, PackedContentChunk},
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::io::AsyncReadExt;

#[derive(Debug)]
pub(crate) struct PackGeneration {
    file: Option<File>,
    location: Mutex<(PathBuf, bool)>,
}
impl PackGeneration {
    #[cfg(test)]
    pub fn baseline_path(&self) -> PathBuf {
        self.location
            .lock()
            .expect("pack generation path")
            .0
            .clone()
    }
    pub fn open(path: &Path) -> Result<Arc<Self>, AppError> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(AppError::InvalidData(
                "pack generation is not a regular file".into(),
            ));
        }
        Ok(Arc::new(Self {
            file: Some(File::open(path)?),
            location: Mutex::new((path.to_owned(), false)),
        }))
    }
    pub fn read_exact_at(&self, mut out: &mut [u8], mut offset: u64) -> Result<(), AppError> {
        let file = self.file.as_ref().expect("live pack generation");
        while !out.is_empty() {
            #[cfg(windows)]
            let n = {
                use std::os::windows::fs::FileExt;
                file.seek_read(out, offset)?
            };
            #[cfg(unix)]
            let n = {
                use std::os::unix::fs::FileExt;
                file.read_at(out, offset)?
            };
            if n == 0 {
                return Err(AppError::InvalidData(
                    "pack generation ended inside a verified range".into(),
                ));
            }
            offset += n as u64;
            out = &mut out[n..];
        }
        Ok(())
    }
    pub fn relocated(&self, path: &Path) {
        self.location.lock().expect("pack generation path").0 = path.to_owned();
    }
    pub async fn retire(&self) -> Result<(), AppError> {
        let path = self
            .location
            .lock()
            .expect("pack generation path")
            .0
            .clone();
        let retired = path.with_extension(format!("{}.retired", uuid::Uuid::new_v4()));
        tokio::fs::rename(&path, &retired).await?;
        *self.location.lock().expect("pack generation path") = (retired, true);
        Ok(())
    }
}
impl Drop for PackGeneration {
    fn drop(&mut self) {
        drop(self.file.take());
        if let Ok((path, retired)) = self.location.get_mut() {
            if *retired {
                if let Err(error) = std::fs::remove_file(path) {
                    crate::logger::warn(&format!("retired pack cleanup deferred: {error}"));
                }
            }
        }
    }
}

/// Pack-relative intervals; cached prefix is registered without traffic credit.
#[derive(Default)]
pub(crate) struct UniqueTraffic(Mutex<HashMap<String, Vec<(u64, u64)>>>);
impl UniqueTraffic {
    pub fn observe(&self, pack: &str, start: u64, end: u64) -> u64 {
        if end <= start {
            return 0;
        }
        let mut map = self.0.lock().expect("unique pack intervals");
        let intervals = map.entry(pack.to_owned()).or_default();
        let duplicate: u64 = intervals
            .iter()
            .map(|(a, b)| end.min(*b).saturating_sub(start.max(*a)))
            .sum();
        intervals.push((start, end));
        intervals.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(intervals.len());
        for &(a, b) in intervals.iter() {
            if let Some(last) = merged.last_mut().filter(|last| a <= last.1) {
                last.1 = last.1.max(b);
            } else {
                merged.push((a, b));
            }
        }
        *intervals = merged;
        end - start - duplicate
    }
}

pub(crate) struct ChunkStream {
    chunks: Vec<(String, PackedContentChunk)>,
    next: usize,
    hash: Sha256,
    pub source: Arc<PackGeneration>,
    pub base: u64,
}
impl ChunkStream {
    pub fn new(
        manifest: &DrivePackManifest,
        required: &[String],
        source: Arc<PackGeneration>,
        base: u64,
        end: u64,
    ) -> Self {
        let mut chunks = required
            .iter()
            .filter_map(|raw| {
                let chunk = &manifest.chunks[raw];
                (chunk.offset >= base && chunk.offset + chunk.compressed_size <= end)
                    .then(|| (raw.clone(), chunk.clone()))
            })
            .collect::<Vec<_>>();
        chunks.sort_by_key(|(_, chunk)| chunk.offset);
        Self {
            chunks,
            next: 0,
            hash: Sha256::new(),
            source,
            base,
        }
    }
    /// Feed consecutive bytes. Publish returned chunks ONLY after flush.
    pub fn feed(
        &mut self,
        absolute_start: u64,
        bytes: &[u8],
    ) -> Result<Vec<(String, PackedContentChunk)>, AppError> {
        let end = absolute_start + bytes.len() as u64;
        let mut ready = Vec::new();
        while let Some((raw, chunk)) = self.chunks.get(self.next) {
            let chunk_end = chunk.offset + chunk.compressed_size;
            if end <= chunk.offset {
                break;
            }
            let from = absolute_start.max(chunk.offset);
            let to = end.min(chunk_end);
            if from < to {
                self.hash.update(
                    &bytes[(from - absolute_start) as usize..(to - absolute_start) as usize],
                );
            }
            if end < chunk_end {
                break;
            }
            let hash = std::mem::take(&mut self.hash);
            if hex::encode(hash.finalize()) != chunk.compressed_sha256 {
                return Err(AppError::InvalidData(
                    "streamed compressed chunk failed SHA-256".into(),
                ));
            }
            ready.push((raw.clone(), chunk.clone()));
            self.next += 1;
        }
        Ok(ready)
    }
    pub async fn resume(
        &mut self,
        path: &Path,
        metrics: &PackMetrics,
    ) -> Result<(u64, Sha256, Vec<(String, PackedContentChunk)>), AppError> {
        let mut file = tokio::fs::File::open(path).await?;
        let mut buffer = vec![0; 1024 * 1024];
        let mut hasher = Sha256::new();
        let mut offset = 0;
        let mut ready = Vec::new();
        loop {
            let n = file.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            match self.feed(self.base + offset, &buffer[..n]) {
                Ok(chunks) => ready.extend(chunks),
                Err(error) => {
                    metrics.integrity_failed();
                    return Err(error);
                }
            }
            offset += n as u64;
        }
        Ok((offset, hasher, ready))
    }
}

pub(crate) async fn create_generation(path: &Path) -> Result<Arc<PackGeneration>, AppError> {
    if PackCache::regular_file_size(path).await?.is_none() {
        tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await?;
    }
    PackGeneration::open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drive_pack_unique_intervals_exclude_cache_overlaps_and_replicas() {
        let traffic = UniqueTraffic::default();
        assert_eq!(traffic.observe("pack", 0, 10), 10);
        assert_eq!(traffic.observe("pack", 5, 15), 5);
        assert_eq!(traffic.observe("pack", 0, 15), 0);
        assert_eq!(traffic.observe("pack", 20, 25), 5);
        assert_eq!(traffic.observe("pack", 10, 30), 10);
        assert_eq!(traffic.observe("other", 0, 15), 15);
    }
    #[tokio::test]
    async fn drive_pack_retired_generation_remains_readable_until_last_reader() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("pack.part");
        std::fs::write(&path, b"verified old").unwrap();
        let generation = PackGeneration::open(&path).unwrap();
        let reader = generation.clone();
        generation.retire().await.unwrap();
        std::fs::write(&path, b"replacement!").unwrap();
        drop(generation);
        let mut bytes = [0; 12];
        reader.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"verified old");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
        drop(reader);
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
