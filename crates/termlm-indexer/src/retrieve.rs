use crate::chunk::Chunk;
use crate::embed::{f16_dot, normalize_to_f16};
use crate::lexical::LexicalIndex;
use half::f16;
use memmap2::Mmap;
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct RetrievalQuery {
    pub text: String,
    pub top_k: usize,
    pub min_score: f32,
    pub hybrid_enabled: bool,
    pub lexical_enabled: bool,
    pub lexical_top_k: usize,
    pub exact_command_boost: f32,
    pub exact_flag_boost: f32,
    pub section_boost_options: f32,
    pub command_aware: bool,
}

impl RetrievalQuery {
    pub fn new(text: impl Into<String>, top_k: usize, min_score: f32) -> Self {
        Self {
            text: text.into(),
            top_k,
            min_score,
            hybrid_enabled: true,
            lexical_enabled: true,
            lexical_top_k: 50,
            exact_command_boost: 2.0,
            exact_flag_boost: 1.0,
            section_boost_options: 0.25,
            command_aware: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RetrievalResult {
    pub chunk: Chunk,
    pub score: f32,
}

#[derive(Debug, Clone)]
pub struct HybridRetriever {
    pub chunks: Vec<Chunk>,
    lexical: LexicalIndex,
    vectors: VectorStore,
    dim: usize,
    query_prefix: String,
}

#[derive(Debug, Clone)]
enum VectorStore {
    None,
    Dense(Vec<Vec<f16>>),
    MmapF16 {
        mmap: Arc<Mmap>,
        rows: usize,
        dim: usize,
    },
    MmapF32 {
        mmap: Arc<Mmap>,
        rows: usize,
        dim: usize,
    },
}

impl Default for HybridRetriever {
    fn default() -> Self {
        Self {
            chunks: Vec::new(),
            lexical: LexicalIndex::default(),
            vectors: VectorStore::None,
            dim: 384,
            query_prefix: String::new(),
        }
    }
}

impl HybridRetriever {
    pub fn new(chunks: Vec<Chunk>) -> Self {
        Self::with_dim(chunks, 384)
    }

    pub fn with_dim(chunks: Vec<Chunk>, dim: usize) -> Self {
        Self::with_dim_and_prefixes(chunks, dim, "", "")
    }

    pub fn lexical_only(chunks: Vec<Chunk>) -> Self {
        let mut lexical = LexicalIndex::default();
        for (i, c) in chunks.iter().enumerate() {
            lexical.insert(i, &c.text);
            lexical.insert(i, &c.command_name);
            lexical.insert(i, &c.section_name);
        }
        Self {
            chunks,
            lexical,
            vectors: VectorStore::None,
            dim: 384,
            query_prefix: String::new(),
        }
    }

    pub fn set_lexical_index(&mut self, lexical: LexicalIndex) {
        self.lexical = lexical;
    }

    pub fn with_mmap_f16(
        chunks: Vec<Chunk>,
        dim: usize,
        query_prefix: impl Into<String>,
        mmap: Arc<Mmap>,
    ) -> Option<Self> {
        let rows = chunks.len();
        let expected = rows.checked_mul(dim)?.checked_mul(2)?;
        if mmap.len() < expected {
            return None;
        }
        let mut lexical = LexicalIndex::default();
        for (i, c) in chunks.iter().enumerate() {
            lexical.insert(i, &c.text);
            lexical.insert(i, &c.command_name);
            lexical.insert(i, &c.section_name);
        }
        Some(Self {
            chunks,
            lexical,
            vectors: VectorStore::MmapF16 { mmap, rows, dim },
            dim,
            query_prefix: query_prefix.into(),
        })
    }

    pub fn with_mmap_f32(
        chunks: Vec<Chunk>,
        dim: usize,
        query_prefix: impl Into<String>,
        mmap: Arc<Mmap>,
    ) -> Option<Self> {
        let rows = chunks.len();
        let expected = rows.checked_mul(dim)?.checked_mul(4)?;
        if mmap.len() < expected {
            return None;
        }
        let mut lexical = LexicalIndex::default();
        for (i, c) in chunks.iter().enumerate() {
            lexical.insert(i, &c.text);
            lexical.insert(i, &c.command_name);
            lexical.insert(i, &c.section_name);
        }
        Some(Self {
            chunks,
            lexical,
            vectors: VectorStore::MmapF32 { mmap, rows, dim },
            dim,
            query_prefix: query_prefix.into(),
        })
    }

    pub fn with_dim_and_prefixes(
        chunks: Vec<Chunk>,
        dim: usize,
        query_prefix: impl Into<String>,
        doc_prefix: impl Into<String>,
    ) -> Self {
        let query_prefix = query_prefix.into();
        let doc_prefix = doc_prefix.into();
        let mut lexical = LexicalIndex::default();
        let mut vectors = Vec::with_capacity(chunks.len());
        for (i, c) in chunks.iter().enumerate() {
            lexical.insert(i, &c.text);
            lexical.insert(i, &c.command_name);
            lexical.insert(i, &c.section_name);
            vectors.push(embed_text_f16(
                &format!(
                    "{}{}\n{}\n{}",
                    doc_prefix, c.command_name, c.section_name, c.text
                ),
                dim,
            ));
        }
        Self {
            chunks,
            lexical,
            vectors: VectorStore::Dense(vectors),
            dim,
            query_prefix,
        }
    }

    pub fn search(&self, query: &RetrievalQuery) -> Vec<RetrievalResult> {
        self.search_with_embedding(query, None)
    }

    pub fn search_with_embedding(
        &self,
        query: &RetrievalQuery,
        query_embedding: Option<&[f32]>,
    ) -> Vec<RetrievalResult> {
        if self.chunks.is_empty() {
            return Vec::new();
        }

        let qvec = if query.hybrid_enabled {
            if let Some(vec) = query_embedding {
                to_query_vector(vec, self.dim)
            } else {
                embed_text_f16(&format!("{}{}", self.query_prefix, query.text), self.dim)
            }
        } else {
            Vec::new()
        };
        let mut candidate_ids = BTreeSet::new();

        let mut lexical_hits = if query.lexical_enabled {
            self.lexical.search(&query.text)
        } else {
            Vec::new()
        };
        if query.lexical_top_k > 0 && lexical_hits.len() > query.lexical_top_k {
            lexical_hits.truncate(query.lexical_top_k);
        }
        for (doc_id, _) in &lexical_hits {
            candidate_ids.insert(*doc_id);
        }
        if candidate_ids.is_empty() {
            if query.hybrid_enabled || !query.lexical_enabled {
                // fall back to all rows so vector retrieval still works when lexical misses
                for i in 0..self.chunks.len() {
                    candidate_ids.insert(i);
                }
            } else {
                return Vec::new();
            }
        }

        let mut lexical_score_map = std::collections::BTreeMap::new();
        for (doc_id, lexical_score) in lexical_hits {
            lexical_score_map.insert(doc_id, lexical_score);
        }

        let q_lower = query.text.to_ascii_lowercase();
        let query_tokens = tokenize(&q_lower);
        let mut scored = Vec::with_capacity(candidate_ids.len());

        for doc_id in candidate_ids {
            if let Some(chunk) = self.chunks.get(doc_id) {
                let mut fused = lexical_score_map.get(&doc_id).copied().unwrap_or(0.0);
                if query.hybrid_enabled {
                    fused += self.vector_dot(doc_id, &qvec);
                }

                if query.command_aware {
                    let chunk_command = chunk.command_name.to_ascii_lowercase();
                    if query_tokens.iter().any(|tok| tok == &chunk_command) {
                        fused += query.exact_command_boost;
                    }
                    if chunk.section_name.eq_ignore_ascii_case("OPTIONS")
                        || chunk.section_name.eq_ignore_ascii_case("SYNOPSIS")
                        || chunk.section_name.eq_ignore_ascii_case("EXAMPLES")
                    {
                        fused += query.section_boost_options;
                    }
                    if query_tokens
                        .iter()
                        .any(|tok| tok.starts_with('-') && chunk.text.contains(tok))
                    {
                        fused += query.exact_flag_boost;
                    }
                }
                if fused >= query.min_score {
                    scored.push(RetrievalResult {
                        chunk: chunk.clone(),
                        score: fused,
                    });
                }
            }
        }

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.chunk.command_name.cmp(&b.chunk.command_name))
        });
        scored.truncate(query.top_k);
        scored
    }

    fn vector_dot(&self, doc_id: usize, qvec: &[f16]) -> f32 {
        match &self.vectors {
            VectorStore::Dense(rows) => rows
                .get(doc_id)
                .map(|row| f16_dot(qvec, row))
                .unwrap_or(0.0),
            VectorStore::None => 0.0,
            VectorStore::MmapF16 { mmap, rows, dim } => {
                if doc_id >= *rows || qvec.len() < *dim {
                    return 0.0;
                }
                let row_start = doc_id.saturating_mul(*dim).saturating_mul(2);
                let row_end = row_start.saturating_add(dim.saturating_mul(2));
                if row_end > mmap.len() {
                    return 0.0;
                }
                let bytes = &mmap[row_start..row_end];
                let mut dot = 0.0f32;
                for (idx, chunk) in bytes.chunks_exact(2).enumerate() {
                    let bits = u16::from_le_bytes([chunk[0], chunk[1]]);
                    let val = f16::from_bits(bits).to_f32();
                    dot += qvec[idx].to_f32() * val;
                }
                dot
            }
            VectorStore::MmapF32 { mmap, rows, dim } => {
                if doc_id >= *rows || qvec.len() < *dim {
                    return 0.0;
                }
                let row_start = doc_id.saturating_mul(*dim).saturating_mul(4);
                let row_end = row_start.saturating_add(dim.saturating_mul(4));
                if row_end > mmap.len() {
                    return 0.0;
                }
                let bytes = &mmap[row_start..row_end];
                let mut dot = 0.0f32;
                for (idx, chunk) in bytes.chunks_exact(4).enumerate() {
                    let val = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    dot += qvec[idx].to_f32() * val;
                }
                dot
            }
        }
    }
}

fn to_query_vector(input: &[f32], dim: usize) -> Vec<f16> {
    if dim == 0 {
        return vec![f16::from_f32(0.0)];
    }
    let mut out = vec![0.0f32; dim];
    let copy = input.len().min(dim);
    out[..copy].copy_from_slice(&input[..copy]);
    normalize_to_f16(&out)
}

fn embed_text_f16(text: &str, dim: usize) -> Vec<f16> {
    let mut vec = vec![0.0f32; dim.max(1)];
    for tok in tokenize(text) {
        let mut h = fnv1a64(tok.as_bytes()) as usize;
        let idx = h % vec.len();
        h = h.rotate_left(13);
        let sign = if h & 1 == 0 { 1.0 } else { -1.0 };
        vec[idx] += sign;
    }
    normalize_to_f16(&vec)
}

fn tokenize(input: &str) -> Vec<String> {
    input
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn hybrid_search_returns_ranked_chunks() {
        let chunks = vec![
            Chunk {
                command_name: "ls".to_string(),
                path: "/bin/ls".to_string(),
                extraction_method: "man".to_string(),
                section_name: "NAME".to_string(),
                chunk_index: 0,
                total_chunks: 1,
                doc_hash: "a".to_string(),
                extracted_at: Utc::now(),
                text: "ls - list directory contents".to_string(),
            },
            Chunk {
                command_name: "grep".to_string(),
                path: "/usr/bin/grep".to_string(),
                extraction_method: "man".to_string(),
                section_name: "NAME".to_string(),
                chunk_index: 0,
                total_chunks: 1,
                doc_hash: "b".to_string(),
                extracted_at: Utc::now(),
                text: "grep - search text patterns".to_string(),
            },
        ];
        let retriever = HybridRetriever::with_dim(chunks, 64);
        let query = RetrievalQuery::new("list files with ls", 1, 0.0);
        let hits = retriever.search(&query);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk.command_name, "ls");
    }

    #[test]
    fn external_query_embedding_is_used_when_provided() {
        let chunks = vec![
            Chunk {
                command_name: "ls".to_string(),
                path: "/bin/ls".to_string(),
                extraction_method: "man".to_string(),
                section_name: "NAME".to_string(),
                chunk_index: 0,
                total_chunks: 1,
                doc_hash: "a".to_string(),
                extracted_at: Utc::now(),
                text: "ls - list directory contents".to_string(),
            },
            Chunk {
                command_name: "grep".to_string(),
                path: "/usr/bin/grep".to_string(),
                extraction_method: "man".to_string(),
                section_name: "NAME".to_string(),
                chunk_index: 0,
                total_chunks: 1,
                doc_hash: "b".to_string(),
                extracted_at: Utc::now(),
                text: "grep - search text patterns".to_string(),
            },
        ];
        let retriever = HybridRetriever::with_dim(chunks, 64);
        let external = embed_text_f16("grep NAME grep - search text patterns", 64)
            .into_iter()
            .map(|v| v.to_f32())
            .collect::<Vec<_>>();

        let query = RetrievalQuery::new("completely unrelated text", 1, -10.0);
        let hits = retriever.search_with_embedding(&query, Some(&external));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk.command_name, "grep");
    }
}
