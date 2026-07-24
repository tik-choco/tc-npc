//! Long-term memory vector store: a flat JSON file of chunk records with
//! brute-force cosine-similarity search. Ports the behavior of the Go
//! `rag/pkg/store` JSON store and `rag/pkg/content` helpers (`SplitText`,
//! `CosineSimilarity`, `CalculateHash`) used by `agent-memory`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One stored chunk: text + its embedding + bookkeeping fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub doc_id: String,
    /// sha256 hex digest of `text`, used to dedup identical chunks.
    pub hash: String,
    pub text: String,
    pub embedding: Vec<f32>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Unix timestamp (seconds).
    pub created_at: i64,
}

/// A search hit: the stored text and its cosine-similarity score against
/// the query embedding.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub text: String,
    pub score: f32,
}

/// sha256 hex digest of `text`, used both for dedup and as the `hash`
/// field of a [`Record`].
pub fn calculate_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Split `text` into a sliding window of chunks of (at most) `chunk_size`
/// Unicode scalar values ("runes" in the Go original), overlapping by
/// `overlap` characters between consecutive chunks. Ports
/// `rag/pkg/content.SplitText` char-for-char (Go operates on `[]rune`,
/// which corresponds to Rust `char`).
pub fn split_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chunk_size == 0 || chars.len() <= chunk_size {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut i = 0usize;
    loop {
        let end = (i + chunk_size).min(chars.len());
        chunks.push(chars[i..end].iter().collect());

        if end == chars.len() {
            break;
        }

        let step = if chunk_size > overlap { chunk_size - overlap } else { 1 };
        i += step;
    }
    chunks
}

/// Cosine similarity between two equal-length vectors. Returns `0.0` for
/// mismatched lengths, empty vectors, or zero-norm vectors — matching the
/// Go original's degenerate-case handling.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0f64;
    let mut norm_a = 0f64;
    let mut norm_b = 0f64;
    for i in 0..a.len() {
        let x = a[i] as f64;
        let y = b[i] as f64;
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a.sqrt() * norm_b.sqrt())) as f32
}

/// JSON-file-backed vector store, `{data_dir}/memory-store.json`. Loaded
/// once at startup, saved (atomically, via a temp file + rename) after
/// every mutation.
pub struct VectorStore {
    path: PathBuf,
    records: Vec<Record>,
}

impl VectorStore {
    /// Load records from `path` if it exists; a missing file just means an
    /// empty store (matching the Go original, which treats "not exist" as
    /// success with zero records).
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let records = match std::fs::read_to_string(&path) {
            Ok(data) => serde_json::from_str::<Vec<Record>>(&data).unwrap_or_else(|err| {
                tracing::warn!(path = %path.display(), error = %err, "npc-memory: failed to parse memory store, starting empty");
                Vec::new()
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "npc-memory: failed to read memory store, starting empty");
                Vec::new()
            }
        };
        Self { path, records }
    }

    /// Add one chunk as a new record under `doc_id`. Skipped (dedup) if a
    /// record with the same content hash already exists anywhere in the
    /// store. Does not save; call [`VectorStore::save`] after a batch of
    /// mutations.
    pub fn add_chunk(
        &mut self,
        doc_id: &str,
        text: &str,
        embedding: Vec<f32>,
        metadata: HashMap<String, String>,
        created_at: i64,
    ) {
        let hash = calculate_hash(text);
        if self.records.iter().any(|r| r.hash == hash) {
            return;
        }
        self.records.push(Record {
            doc_id: doc_id.to_string(),
            hash,
            text: text.to_string(),
            embedding,
            metadata,
            created_at,
        });
    }

    /// Brute-force cosine-similarity search over every stored record,
    /// filtered by `threshold` and truncated to the top `top_k` by score
    /// (descending).
    pub fn search(&self, query_embedding: &[f32], top_k: usize, threshold: f32) -> Vec<SearchHit> {
        let mut hits: Vec<SearchHit> = self
            .records
            .iter()
            .map(|r| SearchHit {
                text: r.text.clone(),
                score: cosine_similarity(query_embedding, &r.embedding),
            })
            .filter(|hit| hit.score >= threshold)
            .collect();

        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(top_k);
        hits
    }

    /// Persist the current records to disk: write to a temp file in the
    /// same directory, then rename over the target path (atomic on both
    /// POSIX and Windows).
    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(&self.records)?;
        let tmp_path = tmp_path_for(&self.path);
        std::fs::write(&tmp_path, data)?;
        std::fs::rename(&tmp_path, &self.path)?;
        Ok(())
    }
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "memory-store.json".to_string());
    name.push_str(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_text_short_text_is_single_chunk() {
        let chunks = split_text("hello", 512, 64);
        assert_eq!(chunks, vec!["hello".to_string()]);
    }

    #[test]
    fn split_text_slides_with_overlap() {
        // 10 chars, chunk_size=4, overlap=2 -> step=2
        let text = "abcdefghij";
        let chunks = split_text(text, 4, 2);
        assert_eq!(chunks, vec!["abcd", "cdef", "efgh", "ghij"]);
    }

    #[test]
    fn split_text_handles_multibyte_runes() {
        // Japanese text; must split by char count, not byte count.
        let text = "あいうえおかきくけこ"; // 10 chars
        let chunks = split_text(text, 4, 0);
        assert_eq!(chunks, vec!["あいうえ", "おかきく", "けこ"]);
    }

    #[test]
    fn split_text_zero_chunk_size_returns_whole_text() {
        let chunks = split_text("hello world", 0, 0);
        assert_eq!(chunks, vec!["hello world".to_string()]);
    }

    #[test]
    fn cosine_similarity_identical_vectors_is_one() {
        let v = vec![1.0f32, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors_is_zero() {
        let a = vec![1.0f32, 0.0];
        let b = vec![0.0f32, 1.0];
        assert!((cosine_similarity(&a, &b)).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_mismatched_lengths_is_zero() {
        let a = vec![1.0f32, 2.0];
        let b = vec![1.0f32];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn cosine_similarity_zero_vector_is_zero() {
        let a = vec![0.0f32, 0.0];
        let b = vec![1.0f32, 1.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn calculate_hash_is_stable_and_sensitive() {
        let h1 = calculate_hash("hello");
        let h2 = calculate_hash("hello");
        let h3 = calculate_hash("world");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 64); // sha256 hex digest length
    }

    #[test]
    fn add_chunk_dedups_by_hash() {
        let dir = std::env::temp_dir().join(format!("npc-memory-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("memory-store.json");
        let mut store = VectorStore::load(&path);
        store.add_chunk("doc1", "same text", vec![1.0, 0.0], HashMap::new(), 0);
        store.add_chunk("doc2", "same text", vec![0.0, 1.0], HashMap::new(), 1);
        assert_eq!(store.records.len(), 1);
        assert_eq!(store.records[0].doc_id, "doc1");
    }

    #[test]
    fn search_filters_by_threshold_and_truncates_to_top_k() {
        let dir = std::env::temp_dir().join(format!("npc-memory-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("memory-store.json");
        let mut store = VectorStore::load(&path);
        store.add_chunk("d", "a", vec![1.0, 0.0], HashMap::new(), 0);
        store.add_chunk("d", "b", vec![0.9, 0.1], HashMap::new(), 0);
        store.add_chunk("d", "c", vec![0.0, 1.0], HashMap::new(), 0);

        let hits = store.search(&[1.0, 0.0], 5, 0.5);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].text, "a");
        assert_eq!(hits[1].text, "b");

        let hits_top1 = store.search(&[1.0, 0.0], 1, 0.5);
        assert_eq!(hits_top1.len(), 1);
        assert_eq!(hits_top1[0].text, "a");
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("npc-memory-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("memory-store.json");
        let mut store = VectorStore::load(&path);
        let mut meta = HashMap::new();
        meta.insert("type".to_string(), "long_term_memory".to_string());
        store.add_chunk("doc1", "hello world", vec![1.0, 2.0, 3.0], meta, 42);
        store.save().unwrap();

        let reloaded = VectorStore::load(&path);
        assert_eq!(reloaded.records.len(), 1);
        assert_eq!(reloaded.records[0].text, "hello world");
        assert_eq!(reloaded.records[0].created_at, 42);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
