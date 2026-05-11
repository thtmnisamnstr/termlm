use crate::chunk::Chunk;
use crate::lexical::LexicalIndex;
use crate::scan::BinaryEntry;
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use half::f16;
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexManifest {
    pub index_version: u32,
    pub embedding_model_hash: String,
    #[serde(default = "default_embedding_mode")]
    pub embedding_mode: String,
    pub embed_dim: usize,
    pub vector_storage: String,
    pub chunk_count: usize,
    pub generated_at: DateTime<Utc>,
    pub query_prefix: String,
    pub doc_prefix: String,
}

fn default_embedding_mode() -> String {
    "disabled".to_string()
}

#[derive(Debug, Clone)]
pub struct IndexStore {
    pub root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct LayoutWriteArtifacts<'a> {
    pub entries: &'a [BinaryEntry],
    pub chunks: &'a [Chunk],
    pub tombstoned_chunks: &'a [Chunk],
    pub lexical_index_enabled: bool,
    pub embed_dim: usize,
    pub vector_storage: &'a str,
    pub doc_prefix: &'a str,
    pub embeddings_f32: Option<&'a [Vec<f32>]>,
}

const ENTRIES_MAGIC_V1: &[u8; 8] = b"TLME0001";
const CHUNKS_MAGIC_V1: &[u8; 8] = b"TLMC0001";
const LEXICON_MAGIC_V1: &[u8; 8] = b"TLML0001";
const POSTINGS_MAGIC_V1: &[u8; 8] = b"TLMP0001";
const ENTRY_RECORD_BYTES: usize = 60;
const CHUNK_RECORD_BYTES: usize = 72;

impl IndexStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn ensure_layout(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create {}", self.root.display()))?;
        Ok(())
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.root.join("manifest.json")
    }

    pub fn load_manifest(&self) -> Result<Option<IndexManifest>> {
        let path = self.manifest_path();
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let parsed = serde_json::from_str::<IndexManifest>(&raw)
            .with_context(|| format!("parse {}", path.display()))?;
        Ok(Some(parsed))
    }

    pub fn write_manifest_atomic(&self, manifest: &IndexManifest) -> Result<()> {
        self.ensure_layout()?;
        let dst = self.manifest_path();
        atomic_write_json(&dst, manifest)
    }

    pub fn load_chunks(&self) -> Result<Vec<Chunk>> {
        let chunks_bin = self.root.join("chunks.bin");
        let docs_bin = self.root.join("docs.bin");
        if !(chunks_bin.exists() && docs_bin.exists()) {
            return Ok(Vec::new());
        }
        self.load_chunks_from_binary_layout()
    }

    pub fn load_entries(&self) -> Result<Vec<BinaryEntry>> {
        let path = self.root.join("entries.bin");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        if !raw.starts_with(ENTRIES_MAGIC_V1) {
            bail!("entries.bin has unsupported format");
        }
        let paths_blob = fs::read(self.root.join("paths.bin"))
            .with_context(|| format!("read {}", self.root.join("paths.bin").display()))?;
        read_entries_v1(&raw, &paths_blob)
    }

    pub fn load_lexical_index(&self) -> Result<Option<LexicalIndex>> {
        let lexicon_path = self.root.join("lexicon.bin");
        let postings_path = self.root.join("postings.bin");
        if !(lexicon_path.exists() && postings_path.exists()) {
            return Ok(None);
        }
        let lexicon_raw =
            fs::read(&lexicon_path).with_context(|| format!("read {}", lexicon_path.display()))?;
        let postings_raw = fs::read(&postings_path)
            .with_context(|| format!("read {}", postings_path.display()))?;
        let postings = read_lexical_postings_v1(&lexicon_raw, &postings_raw)?;
        Ok(Some(LexicalIndex::from_postings(postings)))
    }

    pub fn write_layout_artifacts(&self, artifacts: LayoutWriteArtifacts<'_>) -> Result<()> {
        self.ensure_layout()?;
        write_paths_bin(&self.root.join("paths.bin"), artifacts.entries)?;
        write_entries_bin(
            &self.root.join("entries.bin"),
            artifacts.entries,
            artifacts.chunks,
        )?;
        write_docs_bin(&self.root.join("docs.bin"), artifacts.chunks)?;
        write_chunks_bin(
            &self.root.join("chunks.bin"),
            artifacts.entries,
            artifacts.chunks,
            artifacts.tombstoned_chunks,
        )?;
        let lexicon_path = self.root.join("lexicon.bin");
        let postings_path = self.root.join("postings.bin");
        if artifacts.lexical_index_enabled {
            write_lexical_bins(&lexicon_path, &postings_path, artifacts.chunks)?;
        } else {
            remove_file_if_exists(&lexicon_path)?;
            remove_file_if_exists(&postings_path)?;
        }
        write_vectors_file(
            &self.root,
            artifacts.chunks,
            artifacts.embed_dim,
            artifacts.vector_storage,
            artifacts.doc_prefix,
            artifacts.embeddings_f32,
        )?;
        Ok(())
    }

    pub fn hash_file(path: &Path) -> Result<String> {
        let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let hash = Sha256::digest(&data);
        Ok(format!("{hash:x}"))
    }

    pub fn mmap_file(&self, filename: &str) -> Result<Option<Mmap>> {
        let path = self.root.join(filename);
        if !path.exists() {
            return Ok(None);
        }
        let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: the file descriptor remains alive for the lifetime of mmap in this scope.
        let mmap =
            unsafe { Mmap::map(&file) }.with_context(|| format!("mmap {}", path.display()))?;
        Ok(Some(mmap))
    }

    fn load_chunks_from_binary_layout(&self) -> Result<Vec<Chunk>> {
        let entries = self.load_entries()?;
        let docs = fs::read(self.root.join("docs.bin"))
            .with_context(|| format!("read {}", self.root.join("docs.bin").display()))?;
        let raw = fs::read(self.root.join("chunks.bin"))
            .with_context(|| format!("read {}", self.root.join("chunks.bin").display()))?;
        read_chunks_v1(&raw, &docs, &entries)
    }
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("remove {}", path.display())),
    }
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    Ok(())
}

fn atomic_write_json<T: Serialize + ?Sized>(dst: &Path, value: &T) -> Result<()> {
    ensure_parent_dir(dst)?;
    let tmp = dst.with_extension("tmp");
    let serialized = serde_json::to_vec_pretty(value)?;

    {
        let mut f = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(&serialized)
            .with_context(|| format!("write {}", tmp.display()))?;
        f.flush()
            .with_context(|| format!("flush {}", tmp.display()))?;
        f.sync_all()
            .with_context(|| format!("fsync {}", tmp.display()))?;
    }

    fs::rename(&tmp, dst)
        .with_context(|| format!("rename {} -> {}", tmp.display(), dst.display()))?;
    Ok(())
}

fn atomic_write_bytes(dst: &Path, bytes: &[u8]) -> Result<()> {
    ensure_parent_dir(dst)?;
    let tmp = dst.with_extension("tmp");
    {
        let mut f = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(bytes)
            .with_context(|| format!("write {}", tmp.display()))?;
        f.flush()
            .with_context(|| format!("flush {}", tmp.display()))?;
        f.sync_all()
            .with_context(|| format!("fsync {}", tmp.display()))?;
    }
    fs::rename(&tmp, dst)
        .with_context(|| format!("rename {} -> {}", tmp.display(), dst.display()))?;
    Ok(())
}

#[derive(Debug, Clone, Default)]
struct EntryStats {
    chunk_first: u32,
    chunk_count: u32,
    doc_off: u64,
    doc_len: u64,
}

#[derive(Debug, Clone)]
struct PathCatalog {
    blob: Vec<u8>,
    offsets: BTreeMap<String, (u64, u32)>,
}

fn build_path_catalog(entries: &[BinaryEntry]) -> Result<PathCatalog> {
    let mut blob = Vec::<u8>::new();
    let mut offsets = BTreeMap::<String, (u64, u32)>::new();
    for entry in entries {
        let path = entry.path.to_string_lossy().to_string();
        let off = u64::try_from(blob.len()).context("paths blob offset overflow")?;
        let bytes = path.as_bytes();
        let len = u32::try_from(bytes.len()).context("path length overflow")?;
        blob.extend_from_slice(bytes);
        offsets.insert(path, (off, len));
    }
    Ok(PathCatalog { blob, offsets })
}

fn build_entry_stats(chunks: &[Chunk]) -> BTreeMap<String, EntryStats> {
    let mut stats = BTreeMap::<String, EntryStats>::new();
    let mut offset = 0u64;
    for (idx, chunk) in chunks.iter().enumerate() {
        let byte_len = chunk.text.len() as u64;
        let entry = stats
            .entry(chunk.path.clone())
            .or_insert_with(|| EntryStats {
                chunk_first: idx as u32,
                ..EntryStats::default()
            });
        entry.chunk_count = entry.chunk_count.saturating_add(1);
        if entry.doc_len == 0 {
            entry.doc_off = offset;
        }
        entry.doc_len = entry.doc_len.saturating_add(byte_len);
        offset = offset.saturating_add(byte_len);
    }
    stats
}

fn write_paths_bin(path: &Path, entries: &[BinaryEntry]) -> Result<()> {
    let catalog = build_path_catalog(entries)?;
    atomic_write_bytes(path, &catalog.blob)
}

fn write_entries_bin(path: &Path, entries: &[BinaryEntry], chunks: &[Chunk]) -> Result<()> {
    let catalog = build_path_catalog(entries)?;
    let stats = build_entry_stats(chunks);

    let mut payload = Vec::with_capacity(8 + 4 + entries.len() * ENTRY_RECORD_BYTES);
    payload.extend_from_slice(ENTRIES_MAGIC_V1);
    payload.extend_from_slice(&(entries.len() as u32).to_le_bytes());

    for e in entries {
        let entry_path = e.path.to_string_lossy();
        let (path_off, path_len) = catalog
            .offsets
            .get(entry_path.as_ref())
            .copied()
            .unwrap_or((0, 0));
        let s = stats.get(entry_path.as_ref()).cloned().unwrap_or_default();

        payload.extend_from_slice(&path_off.to_le_bytes());
        payload.extend_from_slice(&path_len.to_le_bytes());
        payload.extend_from_slice(&e.mtime_secs.to_le_bytes());
        payload.extend_from_slice(&e.size.to_le_bytes());
        payload.extend_from_slice(&e.inode.to_le_bytes());
        payload.extend_from_slice(&s.doc_off.to_le_bytes());
        payload.extend_from_slice(&s.doc_len.to_le_bytes());
        payload.extend_from_slice(&s.chunk_first.to_le_bytes());
        payload.extend_from_slice(&s.chunk_count.to_le_bytes());
    }

    atomic_write_bytes(path, &payload)
}

fn write_docs_bin(path: &Path, chunks: &[Chunk]) -> Result<()> {
    let mut out = Vec::<u8>::new();
    for c in chunks {
        out.extend_from_slice(c.text.as_bytes());
    }
    atomic_write_bytes(path, &out)
}

fn write_chunks_bin(
    path: &Path,
    entries: &[BinaryEntry],
    chunks: &[Chunk],
    tombstoned_chunks: &[Chunk],
) -> Result<()> {
    let mut section_ids = BTreeMap::<String, u32>::new();
    for c in chunks.iter().chain(tombstoned_chunks.iter()) {
        if !section_ids.contains_key(&c.section_name) {
            let next = u32::try_from(section_ids.len()).context("too many section names")?;
            section_ids.insert(c.section_name.clone(), next);
        }
    }
    let mut sections = vec![String::new(); section_ids.len()];
    for (name, id) in &section_ids {
        if let Some(slot) = sections.get_mut(*id as usize) {
            *slot = name.clone();
        }
    }

    let entry_idx = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.path.to_string_lossy().to_string(), i as u32))
        .collect::<BTreeMap<_, _>>();

    let mut payload = Vec::new();
    payload.extend_from_slice(CHUNKS_MAGIC_V1);
    payload.extend_from_slice(&(sections.len() as u32).to_le_bytes());
    for section in &sections {
        let bytes = section.as_bytes();
        let len = u32::try_from(bytes.len()).context("section name too long")?;
        payload.extend_from_slice(&len.to_le_bytes());
        payload.extend_from_slice(bytes);
    }

    let record_count = chunks
        .len()
        .checked_add(tombstoned_chunks.len())
        .context("chunk row count overflow")?;
    payload.extend_from_slice(&(record_count as u32).to_le_bytes());

    let mut doc_offset = 0u64;
    for c in chunks {
        let section_id = section_ids.get(&c.section_name).copied().unwrap_or(0);
        let byte_len = u32::try_from(c.text.len()).context("chunk text too long")?;
        let chunk_index = u32::try_from(c.chunk_index).context("chunk_index overflow")?;
        let total_chunks = u32::try_from(c.total_chunks).context("total_chunks overflow")?;
        let hash_bytes = decode_doc_hash_hex(&c.doc_hash);
        let extracted_at = c.extracted_at.timestamp();
        let idx = entry_idx.get(&c.path).copied().unwrap_or(u32::MAX);

        payload.extend_from_slice(&idx.to_le_bytes());
        payload.extend_from_slice(&section_id.to_le_bytes());
        payload.extend_from_slice(&doc_offset.to_le_bytes());
        payload.extend_from_slice(&byte_len.to_le_bytes());
        payload.extend_from_slice(&chunk_index.to_le_bytes());
        payload.extend_from_slice(&total_chunks.to_le_bytes());
        payload.push(0u8);
        payload.extend_from_slice(&[encode_extraction_method(&c.extraction_method), 0u8, 0u8]);
        payload.extend_from_slice(&extracted_at.to_le_bytes());
        payload.extend_from_slice(&hash_bytes);

        doc_offset = doc_offset.saturating_add(u64::from(byte_len));
    }
    for c in tombstoned_chunks {
        let section_id = section_ids.get(&c.section_name).copied().unwrap_or(0);
        let chunk_index = u32::try_from(c.chunk_index).context("chunk_index overflow")?;
        let total_chunks = u32::try_from(c.total_chunks).context("total_chunks overflow")?;
        let hash_bytes = decode_doc_hash_hex(&c.doc_hash);
        let extracted_at = c.extracted_at.timestamp();
        let idx = entry_idx.get(&c.path).copied().unwrap_or(u32::MAX);

        payload.extend_from_slice(&idx.to_le_bytes());
        payload.extend_from_slice(&section_id.to_le_bytes());
        payload.extend_from_slice(&0u64.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&chunk_index.to_le_bytes());
        payload.extend_from_slice(&total_chunks.to_le_bytes());
        payload.push(1u8);
        payload.extend_from_slice(&[encode_extraction_method(&c.extraction_method), 0u8, 0u8]);
        payload.extend_from_slice(&extracted_at.to_le_bytes());
        payload.extend_from_slice(&hash_bytes);
    }

    atomic_write_bytes(path, &payload)
}

fn write_lexical_bins(lexicon_path: &Path, postings_path: &Path, chunks: &[Chunk]) -> Result<()> {
    let mut lexicon = BTreeMap::<String, usize>::new();
    let mut postings = BTreeMap::<String, Vec<usize>>::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let text = format!(
            "{} {} {}",
            chunk.command_name, chunk.section_name, chunk.text
        );
        for tok in tokenize(&text) {
            *lexicon.entry(tok.clone()).or_insert(0) += 1;
            postings.entry(tok).or_default().push(i);
        }
    }

    let mut postings_payload = Vec::<u8>::new();
    postings_payload.extend_from_slice(POSTINGS_MAGIC_V1);
    postings_payload.extend_from_slice(&(postings.len() as u32).to_le_bytes());

    let mut lex_payload = Vec::<u8>::new();
    lex_payload.extend_from_slice(LEXICON_MAGIC_V1);
    lex_payload.extend_from_slice(&(lexicon.len() as u32).to_le_bytes());

    for (token, df) in &lexicon {
        let ids = postings.get(token).cloned().unwrap_or_default();
        let ids_u32 = ids
            .iter()
            .map(|id| u32::try_from(*id).context("posting id overflow"))
            .collect::<Result<Vec<_>>>()?;

        let post_off = u64::try_from(postings_payload.len()).context("postings offset overflow")?;
        postings_payload.extend_from_slice(&(ids_u32.len() as u32).to_le_bytes());
        for id in &ids_u32 {
            postings_payload.extend_from_slice(&id.to_le_bytes());
        }
        let post_len = u32::try_from(postings_payload.len() as u64 - post_off)
            .context("postings length overflow")?;

        let tok = token.as_bytes();
        let tok_len = u32::try_from(tok.len()).context("lex token too long")?;
        lex_payload.extend_from_slice(&tok_len.to_le_bytes());
        lex_payload.extend_from_slice(tok);
        lex_payload.extend_from_slice(&u32::try_from(*df).unwrap_or(u32::MAX).to_le_bytes());
        lex_payload.extend_from_slice(&post_off.to_le_bytes());
        lex_payload.extend_from_slice(&post_len.to_le_bytes());
    }

    atomic_write_bytes(lexicon_path, &lex_payload)?;
    atomic_write_bytes(postings_path, &postings_payload)
}

fn write_vectors_file(
    root: &Path,
    chunks: &[Chunk],
    embed_dim: usize,
    vector_storage: &str,
    _doc_prefix: &str,
    embeddings_f32: Option<&[Vec<f32>]>,
) -> Result<()> {
    let f16_path = root.join("vectors.f16");
    let f32_path = root.join("vectors.f32");
    let Some(precomputed) = embeddings_f32 else {
        // Embeddings unavailable: disable vector side and rely on lexical retrieval.
        if f16_path.exists() {
            let _ = fs::remove_file(&f16_path);
        }
        if f32_path.exists() {
            let _ = fs::remove_file(&f32_path);
        }
        return Ok(());
    };

    if precomputed.len() != chunks.len() {
        anyhow::bail!(
            "embedding count mismatch: got {} vectors for {} chunks",
            precomputed.len(),
            chunks.len()
        );
    }
    let mut rows = Vec::<f16>::with_capacity(chunks.len() * embed_dim);
    for vec in precomputed {
        rows.extend(normalize_external_embedding(vec, embed_dim));
    }

    if vector_storage == "f32" {
        let mut f32_bytes = Vec::with_capacity(rows.len() * 4);
        for v in &rows {
            f32_bytes.extend_from_slice(&v.to_f32().to_le_bytes());
        }
        atomic_write_bytes(&f32_path, &f32_bytes)?;
        if f16_path.exists() {
            let _ = fs::remove_file(&f16_path);
        }
    } else {
        let mut f16_bytes = Vec::with_capacity(rows.len() * 2);
        for v in &rows {
            f16_bytes.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        atomic_write_bytes(&f16_path, &f16_bytes)?;
        if f32_path.exists() {
            let _ = fs::remove_file(&f32_path);
        }
    }
    Ok(())
}

fn normalize_external_embedding(input: &[f32], dim: usize) -> Vec<f16> {
    if dim == 0 {
        return vec![f16::from_f32(0.0)];
    }
    let mut out = vec![0.0f32; dim];
    let copy = input.len().min(dim);
    out[..copy].copy_from_slice(&input[..copy]);
    let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    out.into_iter().map(|v| f16::from_f32(v / norm)).collect()
}

fn tokenize(input: &str) -> Vec<String> {
    input
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

fn read_entries_v1(raw: &[u8], paths_blob: &[u8]) -> Result<Vec<BinaryEntry>> {
    if raw.len() < 12 {
        bail!("entries.bin too short");
    }
    if &raw[..8] != ENTRIES_MAGIC_V1 {
        bail!("entries.bin has unsupported magic");
    }
    let count = u32::from_le_bytes(raw[8..12].try_into().expect("slice sized")) as usize;
    let expected = 12usize
        .checked_add(
            count
                .checked_mul(ENTRY_RECORD_BYTES)
                .context("entries size overflow")?,
        )
        .context("entries file size overflow")?;
    if raw.len() < expected {
        bail!("entries.bin truncated");
    }

    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let base = 12 + i * ENTRY_RECORD_BYTES;
        let path_off = u64::from_le_bytes(raw[base..base + 8].try_into().expect("slice sized"));
        let path_len =
            u32::from_le_bytes(raw[base + 8..base + 12].try_into().expect("slice sized"));
        let mtime_secs =
            i64::from_le_bytes(raw[base + 12..base + 20].try_into().expect("slice sized"));
        let size = u64::from_le_bytes(raw[base + 20..base + 28].try_into().expect("slice sized"));
        let inode = u64::from_le_bytes(raw[base + 28..base + 36].try_into().expect("slice sized"));

        let path = read_blob_str(paths_blob, path_off, path_len)
            .unwrap_or_default()
            .to_string();
        let name = derive_command_name(&path);
        out.push(BinaryEntry {
            name,
            path: PathBuf::from(path),
            mtime_secs,
            size,
            inode,
        });
    }
    Ok(out)
}

fn read_chunks_v1(raw: &[u8], docs: &[u8], entries: &[BinaryEntry]) -> Result<Vec<Chunk>> {
    if raw.len() < 12 {
        bail!("chunks.bin too short");
    }
    if &raw[..8] != CHUNKS_MAGIC_V1 {
        bail!("chunks.bin has unsupported magic");
    }

    let mut cursor = 8usize;
    let section_count = read_u32(raw, &mut cursor)? as usize;
    let mut sections = Vec::with_capacity(section_count);
    for _ in 0..section_count {
        let len = read_u32(raw, &mut cursor)? as usize;
        if cursor + len > raw.len() {
            bail!("chunks.bin section table truncated");
        }
        sections.push(String::from_utf8_lossy(&raw[cursor..cursor + len]).to_string());
        cursor += len;
    }
    let row_count = read_u32(raw, &mut cursor)? as usize;
    let tail_bytes = row_count
        .checked_mul(CHUNK_RECORD_BYTES)
        .context("chunks row byte overflow")?;
    if cursor + tail_bytes > raw.len() {
        bail!("chunks.bin row table truncated");
    }

    let mut out = Vec::new();
    for _ in 0..row_count {
        let entry_idx = read_u32(raw, &mut cursor)?;
        let section_id = read_u32(raw, &mut cursor)?;
        let byte_offset = read_u64(raw, &mut cursor)?;
        let byte_len = read_u32(raw, &mut cursor)?;
        let chunk_index = read_u32(raw, &mut cursor)?;
        let total_chunks = read_u32(raw, &mut cursor)?;
        let tombstone = read_u8(raw, &mut cursor)? != 0;
        let extraction_method = decode_extraction_method(read_u8(raw, &mut cursor)?).to_string();
        cursor = cursor.saturating_add(2); // reserved
        if cursor > raw.len() {
            bail!("chunks.bin reserved bytes overflow");
        }
        let extracted_secs = read_i64(raw, &mut cursor)?;
        let hash_slice = read_fixed::<32>(raw, &mut cursor)?;

        if tombstone {
            continue;
        }
        let entry = entries
            .get(entry_idx as usize)
            .ok_or_else(|| anyhow!("chunks.bin references missing entry index {entry_idx}"))?;
        let section = sections
            .get(section_id as usize)
            .cloned()
            .unwrap_or_else(|| "UNKNOWN".to_string());
        let text = read_blob_bytes(docs, byte_offset, byte_len)
            .map(|s| String::from_utf8_lossy(s).to_string())
            .unwrap_or_default();
        let extracted_at =
            DateTime::<Utc>::from_timestamp(extracted_secs, 0).unwrap_or_else(Utc::now);
        out.push(Chunk {
            command_name: entry.name.clone(),
            path: entry.path.to_string_lossy().to_string(),
            extraction_method,
            section_name: section,
            chunk_index: chunk_index as usize,
            total_chunks: total_chunks as usize,
            doc_hash: encode_hash_hex(hash_slice),
            extracted_at,
            text,
        });
    }
    Ok(out)
}

fn read_lexical_postings_v1(
    lexicon_raw: &[u8],
    postings_raw: &[u8],
) -> Result<BTreeMap<String, BTreeSet<usize>>> {
    if lexicon_raw.len() < 12 {
        bail!("lexicon.bin too short");
    }
    if postings_raw.len() < 12 {
        bail!("postings.bin too short");
    }
    if !lexicon_raw.starts_with(LEXICON_MAGIC_V1) {
        bail!("lexicon.bin has unsupported magic");
    }
    if !postings_raw.starts_with(POSTINGS_MAGIC_V1) {
        bail!("postings.bin has unsupported magic");
    }

    let mut lex_cursor = 8usize;
    let lex_count = read_u32(lexicon_raw, &mut lex_cursor)? as usize;
    let postings_count = {
        let mut cursor = 8usize;
        read_u32(postings_raw, &mut cursor)? as usize
    };

    let mut out = BTreeMap::<String, BTreeSet<usize>>::new();
    for _ in 0..lex_count {
        let token_len = read_u32(lexicon_raw, &mut lex_cursor)? as usize;
        if lex_cursor + token_len > lexicon_raw.len() {
            bail!("lexicon.bin token table truncated");
        }
        let token = std::str::from_utf8(&lexicon_raw[lex_cursor..lex_cursor + token_len])
            .context("lexicon.bin token is not valid utf-8")?
            .to_string();
        lex_cursor += token_len;

        let _df = read_u32(lexicon_raw, &mut lex_cursor)?;
        let postings_offset = read_u64(lexicon_raw, &mut lex_cursor)?;
        let postings_len = read_u32(lexicon_raw, &mut lex_cursor)? as usize;

        let start = usize::try_from(postings_offset).context("postings offset overflow")?;
        let end = start
            .checked_add(postings_len)
            .context("postings length overflow")?;
        if end > postings_raw.len() {
            bail!("postings.bin row extends past file bounds");
        }

        let mut post_cursor = start;
        let id_count = read_u32(postings_raw, &mut post_cursor)? as usize;
        let expected_len = 4usize
            .checked_add(
                id_count
                    .checked_mul(4)
                    .context("postings row id bytes overflow")?,
            )
            .context("postings row length overflow")?;
        if expected_len > postings_len {
            bail!("postings.bin row truncated for token");
        }

        let mut ids = BTreeSet::<usize>::new();
        for _ in 0..id_count {
            let id = read_u32(postings_raw, &mut post_cursor)?;
            ids.insert(id as usize);
        }
        out.insert(token, ids);
    }

    if postings_count != out.len() {
        bail!(
            "lexicon/postings token count mismatch (lexicon={} postings={})",
            out.len(),
            postings_count
        );
    }

    Ok(out)
}

fn derive_command_name(path: &str) -> String {
    let p = Path::new(path);
    if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
        return name.to_string();
    }
    path.to_string()
}

fn read_blob_str(blob: &[u8], off: u64, len: u32) -> Option<&str> {
    let bytes = read_blob_bytes(blob, off, len)?;
    std::str::from_utf8(bytes).ok()
}

fn read_blob_bytes(blob: &[u8], off: u64, len: u32) -> Option<&[u8]> {
    let start = usize::try_from(off).ok()?;
    let end = start.checked_add(len as usize)?;
    if end > blob.len() {
        return None;
    }
    Some(&blob[start..end])
}

fn read_u32(raw: &[u8], cursor: &mut usize) -> Result<u32> {
    if raw.len().saturating_sub(*cursor) < 4 {
        bail!("buffer underflow reading u32");
    }
    let out = u32::from_le_bytes(raw[*cursor..*cursor + 4].try_into().expect("slice sized"));
    *cursor += 4;
    Ok(out)
}

fn read_u64(raw: &[u8], cursor: &mut usize) -> Result<u64> {
    if raw.len().saturating_sub(*cursor) < 8 {
        bail!("buffer underflow reading u64");
    }
    let out = u64::from_le_bytes(raw[*cursor..*cursor + 8].try_into().expect("slice sized"));
    *cursor += 8;
    Ok(out)
}

fn read_i64(raw: &[u8], cursor: &mut usize) -> Result<i64> {
    if raw.len().saturating_sub(*cursor) < 8 {
        bail!("buffer underflow reading i64");
    }
    let out = i64::from_le_bytes(raw[*cursor..*cursor + 8].try_into().expect("slice sized"));
    *cursor += 8;
    Ok(out)
}

fn read_u8(raw: &[u8], cursor: &mut usize) -> Result<u8> {
    if raw.len().saturating_sub(*cursor) < 1 {
        bail!("buffer underflow reading u8");
    }
    let out = raw[*cursor];
    *cursor += 1;
    Ok(out)
}

fn read_fixed<const N: usize>(raw: &[u8], cursor: &mut usize) -> Result<[u8; N]> {
    if raw.len().saturating_sub(*cursor) < N {
        bail!("buffer underflow reading fixed slice");
    }
    let out: [u8; N] = raw[*cursor..*cursor + N].try_into().expect("slice sized");
    *cursor += N;
    Ok(out)
}

fn decode_doc_hash_hex(hash: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    let h = hash.trim();
    if h.len() < 64 {
        return out;
    }
    for (i, slot) in out.iter_mut().enumerate() {
        let idx = i * 2;
        let byte = u8::from_str_radix(&h[idx..idx + 2], 16).unwrap_or(0);
        *slot = byte;
    }
    out
}

fn encode_hash_hex(hash: [u8; 32]) -> String {
    if hash.iter().all(|b| *b == 0) {
        return String::new();
    }
    let mut out = String::with_capacity(64);
    for b in hash {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

fn encode_extraction_method(method: &str) -> u8 {
    match method {
        "man" => 1,
        "--help" => 2,
        "-h" => 3,
        "stub" => 4,
        "heuristic" => 5,
        "builtin" => 6,
        "alias" => 7,
        "function" => 8,
        _ => 0,
    }
}

fn decode_extraction_method(code: u8) -> &'static str {
    match code {
        1 => "man",
        2 => "--help",
        3 => "-h",
        4 => "stub",
        5 => "heuristic",
        6 => "builtin",
        7 => "alias",
        8 => "function",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;

    #[test]
    fn manifest_round_trip() {
        let root = std::env::temp_dir().join(format!("termlm-index-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = IndexStore::new(&root);
        let m = IndexManifest {
            index_version: 1,
            embedding_model_hash: "abc".into(),
            embedding_mode: "hash_fallback".into(),
            embed_dim: 384,
            vector_storage: "f16".into(),
            chunk_count: 10,
            generated_at: Utc::now(),
            query_prefix: "q".into(),
            doc_prefix: "d".into(),
        };
        store.write_manifest_atomic(&m).expect("write");
        let loaded = store.load_manifest().expect("load").expect("some");
        assert_eq!(loaded.index_version, 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn entries_round_trip_from_layout_artifacts() {
        let root = std::env::temp_dir().join(format!(
            "termlm-index-entries-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&root);

        let store = IndexStore::new(&root);
        let entries = vec![BinaryEntry {
            name: "ls".to_string(),
            path: PathBuf::from("/bin/ls"),
            mtime_secs: 1,
            size: 2,
            inode: 3,
        }];
        let chunks = vec![Chunk {
            command_name: "ls".to_string(),
            path: "/bin/ls".to_string(),
            extraction_method: "man".to_string(),
            section_name: "NAME".to_string(),
            chunk_index: 0,
            total_chunks: 1,
            doc_hash: "abc".to_string(),
            extracted_at: Utc::now(),
            text: "ls - list".to_string(),
        }];

        store
            .write_layout_artifacts(LayoutWriteArtifacts {
                entries: &entries,
                chunks: &chunks,
                tombstoned_chunks: &[],
                lexical_index_enabled: true,
                embed_dim: 8,
                vector_storage: "f16",
                doc_prefix: "",
                embeddings_f32: None,
            })
            .expect("write artifacts");

        let loaded = store.load_entries().expect("load entries");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "ls");
        assert_eq!(loaded[0].path, PathBuf::from("/bin/ls"));
        assert_eq!(loaded[0].mtime_secs, 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn writing_without_embeddings_removes_vector_files() {
        let root = std::env::temp_dir().join(format!(
            "termlm-index-no-vectors-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&root);

        let store = IndexStore::new(&root);
        let entries = vec![BinaryEntry {
            name: "ls".to_string(),
            path: PathBuf::from("/bin/ls"),
            mtime_secs: 1,
            size: 2,
            inode: 3,
        }];
        let chunks = vec![Chunk {
            command_name: "ls".to_string(),
            path: "/bin/ls".to_string(),
            extraction_method: "man".to_string(),
            section_name: "NAME".to_string(),
            chunk_index: 0,
            total_chunks: 1,
            doc_hash: "abc".to_string(),
            extracted_at: Utc::now(),
            text: "ls - list".to_string(),
        }];

        let embeddings = vec![vec![1.0_f32, 0.0, 0.0, 0.0]];
        store
            .write_layout_artifacts(LayoutWriteArtifacts {
                entries: &entries,
                chunks: &chunks,
                tombstoned_chunks: &[],
                lexical_index_enabled: true,
                embed_dim: 4,
                vector_storage: "f16",
                doc_prefix: "",
                embeddings_f32: Some(&embeddings),
            })
            .expect("write vectors");
        assert!(root.join("vectors.f16").exists());

        store
            .write_layout_artifacts(LayoutWriteArtifacts {
                entries: &entries,
                chunks: &chunks,
                tombstoned_chunks: &[],
                lexical_index_enabled: true,
                embed_dim: 4,
                vector_storage: "f16",
                doc_prefix: "",
                embeddings_f32: None,
            })
            .expect("disable vectors");
        assert!(!root.join("vectors.f16").exists());
        assert!(!root.join("vectors.f32").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn chunks_round_trip_from_layout_artifacts() {
        let root = std::env::temp_dir().join(format!(
            "termlm-index-chunks-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&root);

        let store = IndexStore::new(&root);
        let entries = vec![BinaryEntry {
            name: "grep".to_string(),
            path: PathBuf::from("/usr/bin/grep"),
            mtime_secs: 11,
            size: 22,
            inode: 33,
        }];
        let now = Utc::now();
        let chunks = vec![
            Chunk {
                command_name: "grep".to_string(),
                path: "/usr/bin/grep".to_string(),
                extraction_method: "man".to_string(),
                section_name: "NAME".to_string(),
                chunk_index: 0,
                total_chunks: 2,
                doc_hash: "abcdef".to_string(),
                extracted_at: now,
                text: "grep searches text".to_string(),
            },
            Chunk {
                command_name: "grep".to_string(),
                path: "/usr/bin/grep".to_string(),
                extraction_method: "man".to_string(),
                section_name: "OPTIONS".to_string(),
                chunk_index: 1,
                total_chunks: 2,
                doc_hash: "abcdef".to_string(),
                extracted_at: now,
                text: "-i ignore case".to_string(),
            },
        ];

        store
            .write_layout_artifacts(LayoutWriteArtifacts {
                entries: &entries,
                chunks: &chunks,
                tombstoned_chunks: &[],
                lexical_index_enabled: true,
                embed_dim: 8,
                vector_storage: "f16",
                doc_prefix: "",
                embeddings_f32: None,
            })
            .expect("write artifacts");

        let loaded = store.load_chunks().expect("load chunks");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].command_name, "grep");
        assert_eq!(loaded[0].path, "/usr/bin/grep");
        assert_eq!(loaded[0].extraction_method, "man");
        assert_eq!(loaded[0].section_name, "NAME");
        assert_eq!(loaded[0].text, "grep searches text");
        assert_eq!(loaded[1].extraction_method, "man");
        assert_eq!(loaded[1].section_name, "OPTIONS");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_lexical_index_round_trip() {
        let root = std::env::temp_dir().join(format!(
            "termlm-index-lexical-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&root);

        let store = IndexStore::new(&root);
        let entries = vec![
            BinaryEntry {
                name: "grep".to_string(),
                path: PathBuf::from("/usr/bin/grep"),
                mtime_secs: 1,
                size: 2,
                inode: 3,
            },
            BinaryEntry {
                name: "ls".to_string(),
                path: PathBuf::from("/bin/ls"),
                mtime_secs: 4,
                size: 5,
                inode: 6,
            },
        ];
        let now = Utc::now();
        let chunks = vec![
            Chunk {
                command_name: "grep".to_string(),
                path: "/usr/bin/grep".to_string(),
                extraction_method: "man".to_string(),
                section_name: "NAME".to_string(),
                chunk_index: 0,
                total_chunks: 1,
                doc_hash: "a".to_string(),
                extracted_at: now,
                text: "grep search files".to_string(),
            },
            Chunk {
                command_name: "ls".to_string(),
                path: "/bin/ls".to_string(),
                extraction_method: "man".to_string(),
                section_name: "NAME".to_string(),
                chunk_index: 0,
                total_chunks: 1,
                doc_hash: "b".to_string(),
                extracted_at: now,
                text: "ls list files".to_string(),
            },
        ];

        store
            .write_layout_artifacts(LayoutWriteArtifacts {
                entries: &entries,
                chunks: &chunks,
                tombstoned_chunks: &[],
                lexical_index_enabled: true,
                embed_dim: 8,
                vector_storage: "f16",
                doc_prefix: "",
                embeddings_f32: None,
            })
            .expect("write artifacts");

        let lexical = store
            .load_lexical_index()
            .expect("load lexical")
            .expect("lexical present");
        let grep_hits = lexical.search("search");
        let ls_hits = lexical.search("list");
        assert!(grep_hits.iter().any(|(doc_id, _)| *doc_id == 0));
        assert!(ls_hits.iter().any(|(doc_id, _)| *doc_id == 1));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn disabling_lexical_index_removes_lexical_files() {
        let root = std::env::temp_dir().join(format!(
            "termlm-index-lexical-disable-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&root);

        let store = IndexStore::new(&root);
        let entries = vec![BinaryEntry {
            name: "ls".to_string(),
            path: PathBuf::from("/bin/ls"),
            mtime_secs: 1,
            size: 2,
            inode: 3,
        }];
        let chunks = vec![Chunk {
            command_name: "ls".to_string(),
            path: "/bin/ls".to_string(),
            extraction_method: "man".to_string(),
            section_name: "NAME".to_string(),
            chunk_index: 0,
            total_chunks: 1,
            doc_hash: "abc".to_string(),
            extracted_at: Utc::now(),
            text: "ls list files".to_string(),
        }];

        store
            .write_layout_artifacts(LayoutWriteArtifacts {
                entries: &entries,
                chunks: &chunks,
                tombstoned_chunks: &[],
                lexical_index_enabled: true,
                embed_dim: 8,
                vector_storage: "f16",
                doc_prefix: "",
                embeddings_f32: None,
            })
            .expect("write artifacts");
        assert!(root.join("lexicon.bin").exists());
        assert!(root.join("postings.bin").exists());

        store
            .write_layout_artifacts(LayoutWriteArtifacts {
                entries: &entries,
                chunks: &chunks,
                tombstoned_chunks: &[],
                lexical_index_enabled: false,
                embed_dim: 8,
                vector_storage: "f16",
                doc_prefix: "",
                embeddings_f32: None,
            })
            .expect("rewrite artifacts with lexical disabled");
        assert!(!root.join("lexicon.bin").exists());
        assert!(!root.join("postings.bin").exists());
        assert!(store.load_lexical_index().expect("load lexical").is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_lexical_index_missing_files_returns_none() {
        let root = std::env::temp_dir().join(format!(
            "termlm-index-lexical-none-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&root);

        let store = IndexStore::new(&root);
        let loaded = store.load_lexical_index().expect("load lexical");
        assert!(loaded.is_none());

        let _ = fs::remove_dir_all(&root);
    }
}
