use fastcdc::v2020::FastCDC;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

pub const MIN_DIFF_PAYLOAD_SIZE: usize = 32 * 1024; // 32 KB threshold
pub const RECENT_CACHE_CAPACITY: usize = 16;
pub const BLOCK_SIZE: usize = 32;

/// Content-Defined Chunk generated via FastCDC rolling hash algorithm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentChunk {
    pub offset: usize,
    pub length: usize,
    pub hash: u64,
}

/// Chunk clipboard payload using FastCDC content-defined boundaries.
pub fn chunk_with_fastcdc(data: &[u8]) -> Vec<ContentChunk> {
    if data.is_empty() {
        return Vec::new();
    }
    let min_size = 64;
    let avg_size = 256;
    let max_size = 1024;
    let chunker = FastCDC::new(data, min_size, avg_size, max_size);
    chunker
        .map(|c| ContentChunk {
            offset: c.offset,
            length: c.length,
            hash: c.hash,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeltaOp {
    Copy { offset: u32, length: u32 },
    Insert { data: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaPatch {
    pub base_hash: [u8; 32],
    pub ops: Vec<DeltaOp>,
    pub target_size: usize,
    pub target_hash: [u8; 32],
}

/// Ring buffer cache of recent clip contents keyed by BLAKE3 hash
#[derive(Clone)]
pub struct RecentClipCache {
    entries: VecDeque<([u8; 32], Vec<u8>)>,
    capacity: usize,
}

impl Default for RecentClipCache {
    fn default() -> Self {
        Self::new(RECENT_CACHE_CAPACITY)
    }
}

impl RecentClipCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn insert(&mut self, content: Vec<u8>) -> [u8; 32] {
        let hash = blake3::hash(&content);
        let hash_bytes = *hash.as_bytes();

        if let Some(pos) = self.entries.iter().position(|(h, _)| *h == hash_bytes) {
            let item = self.entries.remove(pos).unwrap();
            self.entries.push_front(item);
            return hash_bytes;
        }

        if self.entries.len() >= self.capacity {
            self.entries.pop_back();
        }
        self.entries.push_front((hash_bytes, content));
        hash_bytes
    }

    pub fn get(&self, hash: &[u8; 32]) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(h, _)| h == hash)
            .map(|(_, data)| data.as_slice())
    }

    /// Try to produce a byte-level delta patch against recent cache entries.
    pub fn create_delta_if_beneficial(&self, target: &[u8]) -> Option<DeltaPatch> {
        if target.len() < MIN_DIFF_PAYLOAD_SIZE || self.entries.is_empty() {
            return None;
        }

        let target_hash = *blake3::hash(target).as_bytes();

        for (base_hash, base_content) in &self.entries {
            let patch = create_byte_delta(base_content, target, *base_hash, target_hash);
            let serialized_size = bincode::serialized_size(&patch).unwrap_or(u64::MAX) as usize;

            // Achieves >= 50% bandwidth reduction
            if serialized_size < target.len() / 2 {
                return Some(patch);
            }
        }

        None
    }

    /// Reconstruct target buffer using base from cache and delta patch.
    pub fn apply_delta(&self, delta: &DeltaPatch) -> Result<Vec<u8>, String> {
        let base = self
            .get(&delta.base_hash)
            .ok_or_else(|| "Base clip not found in cache for delta reconstruction".to_string())?;

        let mut reconstructed = Vec::with_capacity(delta.target_size);

        for op in &delta.ops {
            match op {
                DeltaOp::Copy { offset, length } => {
                    let start = *offset as usize;
                    let end = start + *length as usize;
                    if end > base.len() {
                        return Err("Delta copy bounds exceed base buffer".to_string());
                    }
                    reconstructed.extend_from_slice(&base[start..end]);
                }
                DeltaOp::Insert { data } => {
                    reconstructed.extend_from_slice(data);
                }
            }
        }

        if reconstructed.len() != delta.target_size {
            return Err("Reconstructed size mismatch".to_string());
        }

        let actual_hash = *blake3::hash(&reconstructed).as_bytes();
        if actual_hash != delta.target_hash {
            return Err("Reconstructed hash mismatch".to_string());
        }

        Ok(reconstructed)
    }
}

/// Computes block-level delta operations between base and target
pub fn create_byte_delta(
    base: &[u8],
    target: &[u8],
    base_hash: [u8; 32],
    target_hash: [u8; 32],
) -> DeltaPatch {
    let mut block_map: HashMap<&[u8], usize> = HashMap::new();
    let mut i = 0;
    while i + BLOCK_SIZE <= base.len() {
        block_map.entry(&base[i..i + BLOCK_SIZE]).or_insert(i);
        i += BLOCK_SIZE;
    }

    let mut ops = Vec::new();
    let mut pos = 0;
    let mut pending_insert = Vec::new();

    while pos < target.len() {
        if pos + BLOCK_SIZE <= target.len() {
            let chunk = &target[pos..pos + BLOCK_SIZE];
            if let Some(&base_offset) = block_map.get(chunk) {
                // Flush pending inserts
                if !pending_insert.is_empty() {
                    ops.push(DeltaOp::Insert {
                        data: std::mem::take(&mut pending_insert),
                    });
                }

                // Greedily extend match
                let mut match_len = BLOCK_SIZE;
                while pos + match_len < target.len()
                    && base_offset + match_len < base.len()
                    && target[pos + match_len] == base[base_offset + match_len]
                {
                    match_len += 1;
                }

                ops.push(DeltaOp::Copy {
                    offset: base_offset as u32,
                    length: match_len as u32,
                });

                pos += match_len;
                continue;
            }
        }

        pending_insert.push(target[pos]);
        pos += 1;
    }

    if !pending_insert.is_empty() {
        ops.push(DeltaOp::Insert {
            data: pending_insert,
        });
    }

    DeltaPatch {
        base_hash,
        ops,
        target_size: target.len(),
        target_hash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delta_roundtrip_large() {
        let mut cache = RecentClipCache::new(5);

        let base = "fn master_clipboard_routine() { let x = 42; println!(\"syncing...\"); }\n"
            .repeat(800)
            .into_bytes();
        assert!(base.len() > MIN_DIFF_PAYLOAD_SIZE);

        let base_hash = cache.insert(base.clone());

        let mut modified = base.clone();
        modified.extend_from_slice(b"\n// Added patch test line 2026\n");

        let delta = cache
            .create_delta_if_beneficial(&modified)
            .expect("Delta should be beneficial");

        assert_eq!(delta.base_hash, base_hash);

        let restored = cache.apply_delta(&delta).expect("Apply delta must succeed");
        assert_eq!(restored, modified);
    }

    #[test]
    fn test_fastcdc_chunking() {
        let data = b"Content-Defined Chunking using FastCDC for universal clipboard deduplication."
            .repeat(50);
        let chunks = chunk_with_fastcdc(&data);
        assert!(!chunks.is_empty());
        let total_len: usize = chunks.iter().map(|c| c.length).sum();
        assert_eq!(total_len, data.len());
    }
}
