use sha2::{Digest, Sha256};
use std::{collections::HashMap, io::Read, sync::Mutex};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("invalid inclusive pack range")]
    Range,
    #[error("content size arithmetic overflow")]
    Overflow,
    #[error("{0}")]
    Integrity(&'static str),
    #[error("invalid zstd chunk: {0}")]
    Compression(String),
}

/// Concrete methods keep the hashing loop in this optimized crate, not in the
/// caller's unoptimized Tauri monomorphizations.
#[derive(Default)]
pub struct StreamSha(Sha256);
impl StreamSha {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
    pub fn finish(self) -> String {
        hex::encode(self.0.finalize())
    }
    pub fn digest(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }
}

pub fn decode_verified(
    compressed: &[u8],
    compressed_size: u64,
    compressed_sha: &str,
    raw_size: u64,
    raw_sha: &str,
) -> Result<Vec<u8>, CoreError> {
    if compressed.len() as u64 != compressed_size || StreamSha::digest(compressed) != compressed_sha
    {
        return Err(CoreError::Integrity(
            "compressed content chunk failed verification",
        ));
    }
    let limit = raw_size.checked_add(1).ok_or(CoreError::Overflow)?;
    let capacity = usize::try_from(raw_size).map_err(|_| CoreError::Overflow)?;
    let decoder = zstd::stream::read::Decoder::new(compressed)
        .map_err(|e| CoreError::Compression(e.to_string()))?;
    let mut raw = Vec::with_capacity(capacity);
    decoder
        .take(limit)
        .read_to_end(&mut raw)
        .map_err(|e| CoreError::Compression(e.to_string()))?;
    if raw.len() as u64 != raw_size || StreamSha::digest(&raw) != raw_sha {
        return Err(CoreError::Integrity(
            "raw content chunk failed verification",
        ));
    }
    Ok(raw)
}

#[derive(Clone)]
pub struct ChunkSpan {
    pub offset: u64,
    pub size: u64,
    pub sha256: String,
}
pub struct ChunkVerifier {
    spans: Vec<ChunkSpan>,
    next: usize,
    hash: StreamSha,
    cursor: u64,
}
impl ChunkVerifier {
    pub fn new(spans: Vec<ChunkSpan>, base: u64) -> Self {
        Self {
            spans,
            next: 0,
            hash: StreamSha::new(),
            cursor: base,
        }
    }
    pub fn feed(&mut self, start: u64, bytes: &[u8]) -> Result<Vec<usize>, CoreError> {
        if start != self.cursor {
            return Err(CoreError::Integrity("nonsequential pack stream"));
        }
        let end = start
            .checked_add(bytes.len() as u64)
            .ok_or(CoreError::Overflow)?;
        let mut ready = Vec::new();
        while let Some(chunk) = self.spans.get(self.next) {
            let chunk_end = chunk
                .offset
                .checked_add(chunk.size)
                .ok_or(CoreError::Overflow)?;
            if end <= chunk.offset {
                break;
            }
            let from = start.max(chunk.offset);
            let to = end.min(chunk_end);
            if from < to {
                self.hash
                    .update(&bytes[(from - start) as usize..(to - start) as usize]);
            }
            if end < chunk_end {
                break;
            }
            if std::mem::take(&mut self.hash).finish() != chunk.sha256 {
                return Err(CoreError::Integrity(
                    "streamed compressed chunk failed SHA-256",
                ));
            }
            ready.push(self.next);
            self.next += 1;
        }
        self.cursor = end;
        Ok(ready)
    }
}

#[derive(Default)]
pub struct UniqueTraffic(Mutex<HashMap<String, Vec<(u64, u64)>>>);
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drive_pack_core_hash_boundaries_decode_limits_and_equivalence() {
        let data = vec![73_u8; 8192];
        let sha = StreamSha::digest(&data);
        let mut hasher = StreamSha::new();
        for bytes in data.chunks(73) {
            hasher.update(bytes);
        }
        assert_eq!(hasher.finish(), sha);
        let mut stream = ChunkVerifier::new(
            vec![ChunkSpan {
                offset: 2,
                size: data.len() as u64,
                sha256: sha.clone(),
            }],
            0,
        );
        let mut bytes = vec![0, 1];
        bytes.extend_from_slice(&data);
        assert!(stream.feed(0, &bytes[..1024]).unwrap().is_empty());
        assert_eq!(stream.feed(1024, &bytes[1024..]).unwrap(), vec![0]);
        assert!(stream.feed(0, b"wrong order").is_err());
        let compressed = zstd::stream::encode_all(data.as_slice(), 6).unwrap();
        let compressed_sha = StreamSha::digest(&compressed);
        assert_eq!(
            decode_verified(
                &compressed,
                compressed.len() as u64,
                &compressed_sha,
                data.len() as u64,
                &sha
            )
            .unwrap(),
            data
        );
        assert!(decode_verified(
            &compressed,
            compressed.len() as u64,
            &compressed_sha,
            8,
            &sha
        )
        .is_err());
        assert!(decode_verified(&compressed, compressed.len() as u64, "bad", 8192, &sha).is_err());
    }
}
