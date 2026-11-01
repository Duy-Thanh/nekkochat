use crate::data_crypto::{self, DataKey};
use crate::net;
use crate::types::{cosine_similarity, DocumentChunk, DocumentInfo, OpenAiEmbeddingResponse};
use reqwest::Client;
use serde_json::json;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

const MAX_CHUNK_CHARS: usize = 1200;
const MIN_SCORE: f32 = 0.25;

/// Indexing reads the whole file into memory and costs one embedding request per chunk, so both
/// are bounded: a huge file must not exhaust memory or run up the user's API bill.
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_CHUNKS_PER_FILE: usize = 1500;
/// Longest text sent to the embedding endpoint as a search query
const MAX_QUERY_CHARS: usize = 8000;
/// Longest embedding vector accepted back (real models are 256-4096)
const MAX_EMBEDDING_DIMS: usize = 16384;

/// Only plain-text and source files can be indexed (matches the file picker's filter).
pub const ALLOWED_EXTENSIONS: &[&str] = &[
    "txt", "md", "rs", "py", "c", "cpp", "h", "js", "ts", "json", "svelte", "html", "css", "toml",
    "yaml", "yml", "csv", "log", "go", "java", "kt", "sh",
];

/// An embedding endpoint with its key already resolved and checked by the caller.
pub struct ResolvedEmbedding {
    pub base_url: String,
    pub model: String,
    pub api_key: Zeroizing<String>,
}

/// The store only holds vectors and the on-disk file. Embedding network calls live outside
/// this struct so the mutex is not held while waiting on the API (chat keeps working during indexing).
pub struct RagEngine {
    chunks: Vec<DocumentChunk>,
    storage_path: Option<PathBuf>,
    /// Present only while the store is unlocked. The vectors and the text of the user's
    /// documents are encrypted on disk, so without this key the store is empty and read-only.
    key: Option<DataKey>,
}

const KIND: &str = "rag";

impl RagEngine {
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            storage_path: None,
            key: None,
        }
    }

    /// Sets where the store lives. Nothing is read until `unlock` provides the key.
    pub fn set_storage(&mut self, app_dir: PathBuf) {
        self.storage_path = Some(app_dir.join("rag_store.json"));
    }

    /// Whether the store can currently be read and written.
    pub fn is_unlocked(&self) -> bool {
        self.key.is_some()
    }

    /// Whether an encrypted store file exists (used to decide if a new data key may be created).
    pub fn file_is_encrypted(path: &Path) -> bool {
        fs::read(path).map(|b| data_crypto::is_encrypted(&b)).unwrap_or(false)
    }

    /// Loads the store with the data key. A store written by an older version (plain JSON) is
    /// read and immediately re-saved encrypted.
    pub fn unlock(&mut self, key: DataKey) -> Result<(), String> {
        self.key = Some(key);
        self.chunks.clear();

        let Some(path) = self.storage_path.clone() else {
            return Ok(());
        };
        let Ok(bytes) = fs::read(&path) else {
            return Ok(()); // no store yet
        };

        let migrate = !data_crypto::is_encrypted(&bytes);
        let parsed = if migrate {
            serde_json::from_slice::<Vec<DocumentChunk>>(&bytes).map_err(|e| e.to_string())
        } else {
            let plain = data_crypto::decrypt(self.key.as_ref().expect("just set"), KIND, &bytes);
            plain.and_then(|p| serde_json::from_slice::<Vec<DocumentChunk>>(&p).map_err(|e| e.to_string()))
        };

        match parsed {
            Ok(chunks) => {
                self.chunks = chunks;
                if migrate {
                    self.save_to_disk()?;
                }
                Ok(())
            }
            Err(e) => {
                self.key = None;
                if migrate {
                    // A damaged plain-text store: set it aside instead of overwriting it
                    eprintln!("[RAG] the document store is damaged ({}); kept as .bak", e);
                    let _ = fs::rename(&path, path.with_extension("json.bak"));
                }
                // An encrypted file that fails to decrypt is left exactly as it is, so the right
                // key can still open it later
                Err(format!("The document store could not be read: {}", e))
            }
        }
    }

    /// Forgets the key and every decrypted document from memory.
    pub fn lock(&mut self) {
        self.key = None;
        self.chunks.clear();
    }

    fn save_to_disk(&self) -> Result<(), String> {
        let Some(path) = &self.storage_path else {
            return Ok(());
        };
        let key = self
            .key
            .as_ref()
            .ok_or("The document store is locked. Unlock it in Settings > Security first.")?;
        let json = zeroize::Zeroizing::new(
            serde_json::to_vec(&self.chunks).map_err(|e| format!("Failed to serialize RAG store: {}", e))?,
        );
        crate::write_atomic_bytes(path, &data_crypto::encrypt(key, KIND, &json)?)
    }

    pub fn list_documents(&self) -> Vec<DocumentInfo> {
        let mut docs: Vec<DocumentInfo> = Vec::new();
        for c in &self.chunks {
            match docs.iter_mut().find(|d| d.file_name == c.file_name) {
                Some(d) => d.chunks += 1,
                None => docs.push(DocumentInfo {
                    file_name: c.file_name.clone(),
                    chunks: 1,
                    model: c.model.clone(),
                    can_reindex: !c.source_path.is_empty(),
                }),
            }
        }
        docs
    }

    /// The path a document was originally picked from. Only ever read from our own store, so
    /// the frontend cannot make a re-index read some other file.
    pub fn source_path_of(&self, file_name: &str) -> Option<String> {
        self.chunks
            .iter()
            .find(|c| c.file_name == file_name && !c.source_path.is_empty())
            .map(|c| c.source_path.clone())
    }

    pub fn delete_document(&mut self, file_name: &str) -> Result<(), String> {
        if !self.is_unlocked() {
            return Err("The document store is locked".to_string());
        }
        let before = self.chunks.clone();
        self.chunks.retain(|c| c.file_name != file_name);
        self.save_to_disk().inspect_err(|_| self.chunks = before)
    }

    /// Replaces all existing chunks of a file with new ones (re-indexing creates no duplicates).
    pub fn replace_document(
        &mut self,
        file_name: &str,
        new_chunks: Vec<DocumentChunk>,
    ) -> Result<usize, String> {
        if !self.is_unlocked() {
            return Err("The document store is locked. Unlock it in Settings > Security first.".to_string());
        }
        let count = new_chunks.len();
        let before = self.chunks.clone();
        self.chunks.retain(|c| c.file_name != file_name);
        self.chunks.extend(new_chunks);
        // Memory must match the disk: if the write fails, undo the change
        self.save_to_disk().inspect_err(|_| self.chunks = before)?;
        Ok(count)
    }

    /// A chunk is only comparable with a query embedded by the same model.
    fn is_compatible(chunk: &DocumentChunk, model: &str) -> bool {
        !chunk.model.is_empty() && chunk.model == model
    }

    /// Returns (usable, stale) chunk counts for the given embedding model.
    pub fn compatibility(&self, model: &str) -> (usize, usize) {
        let usable = self.chunks.iter().filter(|c| Self::is_compatible(c, model)).count();
        (usable, self.chunks.len() - usable)
    }

    /// Scores chunks against an already-computed query embedding. Chunks created by a
    /// different embedding model are skipped: their vectors are not comparable.
    pub fn search(&self, query_embedding: &[f32], model: &str, top_k: usize) -> String {
        let mut scored: Vec<(f32, &DocumentChunk)> = self
            .chunks
            .iter()
            .filter(|c| Self::is_compatible(c, model))
            .map(|c| (cosine_similarity(query_embedding, &c.embedding), c))
            .filter(|(score, _)| *score > MIN_SCORE)
            .collect();

        if scored.is_empty() {
            return String::new();
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut ctx = String::from("EXCERPTS FROM THE USER'S PERSONAL DOCUMENTS (RAG CONTEXT):\n");
        for (score, chunk) in scored.into_iter().take(top_k) {
            ctx.push_str(&format!(
                "\n--- [Document: {} | Score: {:.2}] ---\n{}\n",
                chunk.file_name, score, chunk.content
            ));
        }
        ctx
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

fn embeddings_url(base_url: &str) -> String {
    let clean = base_url.trim_end_matches('/');
    if clean.ends_with("/embeddings") {
        clean.to_string()
    } else if clean.ends_with("/v1") {
        format!("{}/embeddings", clean)
    } else {
        format!("{}/v1/embeddings", clean)
    }
}

/// `is_query`: NVIDIA's asymmetric models need to tell queries and passages apart.
pub async fn embed(
    client: &Client,
    cfg: &ResolvedEmbedding,
    text: &str,
    is_query: bool,
) -> Result<Vec<f32>, String> {
    if cfg.base_url.trim().is_empty() {
        return Err("Embedding Base URL is not configured".to_string());
    }
    net::validate_endpoint(&cfg.base_url)?;
    let url = embeddings_url(&cfg.base_url);
    let is_nvidia = cfg.base_url.contains("nvidia.com");

    // No guessed default: the right model depends entirely on the endpoint
    let model = cfg.model.trim();
    if model.is_empty() {
        return Err("Embedding model is not set. Enter one in Settings > Embedding (RAG).".to_string());
    }

    let input: String = text.chars().take(MAX_QUERY_CHARS.max(MAX_CHUNK_CHARS)).collect();
    let mut body = json!({ "input": input, "model": model });
    if is_nvidia {
        // Standard OpenAI rejects unknown fields, so only send this to NVIDIA NIM
        body["input_type"] = json!(if is_query { "query" } else { "passage" });
    }

    let mut req = client.post(&url).json(&body);
    if !cfg.api_key.is_empty() {
        req = req.bearer_auth(cfg.api_key.as_str());
    }

    let res = req
        .send()
        .await
        .map_err(|e| format!("Network error sending embedding request: {}", e))?;

    let status = res.status();
    if status.is_redirection() {
        return Err(format!("The embedding endpoint tried to redirect (HTTP {}); redirects are refused", status));
    }
    if !status.is_success() {
        let err_text = net::read_text_lossy(res, net::MAX_SMALL_BODY).await;
        return Err(format!(
            "Embedding API Error [HTTP {}] at {}: {}",
            status,
            url,
            net::truncate(&err_text, 300)
        ));
    }

    let parsed: OpenAiEmbeddingResponse = net::read_json_limited(res, net::MAX_EMBEDDING_BODY).await?;
    let vector = parsed
        .data
        .into_iter()
        .next()
        .map(|d| d.embedding)
        .ok_or_else(|| "Embedding API returned empty data array".to_string())?;
    if vector.is_empty() || vector.len() > MAX_EMBEDDING_DIMS {
        return Err(format!("Embedding API returned a vector of {} dimensions", vector.len()));
    }
    Ok(vector)
}

/// Reads a file and splits it into chunks of at most MAX_CHUNK_CHARS (merges short paragraphs, splits long ones).
/// Only regular text/source files under MAX_FILE_BYTES are accepted.
pub fn read_and_chunk(path: &Path) -> Result<(String, Vec<String>), String> {
    let file_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !ALLOWED_EXTENSIONS.contains(&extension.as_str()) {
        return Err(format!("\"{}\" is not a supported text or source file type", file_name));
    }

    let meta = fs::metadata(path).map_err(|e| format!("Could not open file: {}", e))?;
    if !meta.is_file() {
        return Err("Only regular files can be indexed".to_string());
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "File is too large ({} MiB). The limit is {} MiB.",
            meta.len() / 1024 / 1024,
            MAX_FILE_BYTES / 1024 / 1024
        ));
    }

    // Read at most one byte past the limit, in case the file grew after the check above
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("Could not read file: {}", e))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("File is too large".to_string());
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| "Could not read file (UTF-8 text required)".to_string())?;

    let chunks = chunk_text(&content);
    if chunks.is_empty() {
        return Err("File is empty".to_string());
    }
    if chunks.len() > MAX_CHUNKS_PER_FILE {
        return Err(format!(
            "This file would need {} embedding requests; the limit is {}. Split it into smaller files.",
            chunks.len(),
            MAX_CHUNKS_PER_FILE
        ));
    }
    Ok((file_name, chunks))
}

fn chunk_text(content: &str) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    let flush = |current: &mut String, chunks: &mut Vec<String>| {
        if !current.trim().is_empty() {
            chunks.push(current.trim().to_string());
        }
        current.clear();
    };

    for para in content.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        // Paragraph too long: hard split by character count, on char boundaries
        if para.chars().count() > MAX_CHUNK_CHARS {
            flush(&mut current, &mut chunks);
            let chars: Vec<char> = para.chars().collect();
            for piece in chars.chunks(MAX_CHUNK_CHARS) {
                chunks.push(piece.iter().collect::<String>().trim().to_string());
            }
            continue;
        }

        if current.chars().count() + para.chars().count() + 2 > MAX_CHUNK_CHARS {
            flush(&mut current, &mut chunks);
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(para);
    }
    flush(&mut current, &mut chunks);
    chunks.retain(|c| !c.is_empty());
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_short_paragraphs_into_one_chunk() {
        let chunks = chunk_text("a\n\nb\n\nc");
        assert_eq!(chunks, vec!["a\n\nb\n\nc"]);
    }

    #[test]
    fn splits_when_over_limit() {
        let para = "x".repeat(MAX_CHUNK_CHARS - 10);
        let text = format!("{para}\n\n{para}");
        assert_eq!(chunk_text(&text).len(), 2);
    }

    #[test]
    fn hard_splits_long_paragraph_on_char_boundary() {
        // Multi-byte characters (e.g. Vietnamese): splitting must not panic
        let text = "ệ".repeat(MAX_CHUNK_CHARS * 2 + 5);
        let chunks = chunk_text(&text);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= MAX_CHUNK_CHARS));
    }

    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(chunk_text("  \n\n  ").is_empty());
    }

    fn chunk(model: &str, embedding: Vec<f32>) -> DocumentChunk {
        DocumentChunk {
            file_name: "a.md".into(),
            content: "hello".into(),
            embedding,
            model: model.into(),
            source_path: String::new(),
        }
    }

    #[test]
    fn search_skips_chunks_from_other_models() {
        let mut rag = RagEngine::new();
        rag.chunks = vec![chunk("model-a", vec![1.0, 0.0]), chunk("model-b", vec![1.0, 0.0])];
        assert_eq!(rag.compatibility("model-a"), (1, 1));
        let ctx = rag.search(&[1.0, 0.0], "model-a", 5);
        assert_eq!(ctx.matches("hello").count(), 1);
        assert!(rag.search(&[1.0, 0.0], "model-c", 5).is_empty());
    }

    #[test]
    fn legacy_chunks_without_model_are_stale() {
        let mut rag = RagEngine::new();
        rag.chunks = vec![chunk("", vec![1.0, 0.0])];
        assert_eq!(rag.compatibility(""), (0, 1));
        assert_eq!(rag.compatibility("model-a"), (0, 1));
        let legacy: DocumentChunk =
            serde_json::from_str(r#"{"fileName":"a","content":"c","embedding":[1.0]}"#).unwrap();
        assert!(legacy.model.is_empty() && legacy.source_path.is_empty());
    }

    fn temp_file(name: &str, content: &[u8]) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "nekkochat-rag-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    fn store_dir() -> PathBuf {
        temp_file("anchor.txt", b"").parent().unwrap().to_path_buf()
    }

    fn engine_in(dir: &Path) -> RagEngine {
        let mut rag = RagEngine::new();
        rag.set_storage(dir.to_path_buf());
        rag
    }

    #[test]
    fn the_store_is_encrypted_on_disk_and_comes_back_with_the_key() {
        let dir = store_dir();
        let key = DataKey::random().unwrap();
        let mut rag = engine_in(&dir);
        rag.unlock(key.clone()).unwrap();
        let mut c = chunk("m", vec![0.5, 0.5]);
        c.content = "the launch code is swordfish".into();
        rag.replace_document("a.md", vec![c]).unwrap();

        let on_disk = fs::read(dir.join("rag_store.json")).unwrap();
        assert!(data_crypto::is_encrypted(&on_disk));
        assert!(!String::from_utf8_lossy(&on_disk).contains("swordfish"));

        let mut fresh = engine_in(&dir);
        assert!(fresh.list_documents().is_empty(), "nothing is readable before unlocking");
        fresh.unlock(key).unwrap();
        assert_eq!(fresh.list_documents()[0].file_name, "a.md");
    }

    #[test]
    fn a_plain_text_store_from_an_older_version_is_migrated() {
        let dir = store_dir();
        let legacy = serde_json::to_vec(&vec![chunk("m", vec![1.0])]).unwrap();
        fs::write(dir.join("rag_store.json"), &legacy).unwrap();

        let mut rag = engine_in(&dir);
        rag.unlock(DataKey::random().unwrap()).unwrap();
        assert_eq!(rag.list_documents().len(), 1, "the old content is still there");
        assert!(data_crypto::is_encrypted(&fs::read(dir.join("rag_store.json")).unwrap()), "and now encrypted");
    }

    #[test]
    fn the_wrong_key_leaves_the_encrypted_file_untouched() {
        let dir = store_dir();
        let mut rag = engine_in(&dir);
        rag.unlock(DataKey::random().unwrap()).unwrap();
        rag.replace_document("a.md", vec![chunk("m", vec![1.0])]).unwrap();
        let before = fs::read(dir.join("rag_store.json")).unwrap();

        let mut other = engine_in(&dir);
        assert!(other.unlock(DataKey::random().unwrap()).is_err());
        assert!(!other.is_unlocked());
        assert_eq!(fs::read(dir.join("rag_store.json")).unwrap(), before, "file unchanged");
        assert!(!dir.join("rag_store.json.bak").exists(), "and not moved aside");
        assert!(other.replace_document("b.md", vec![]).is_err(), "no writes while locked");
    }

    #[test]
    fn locking_wipes_memory_and_blocks_writes() {
        let dir = store_dir();
        let mut rag = engine_in(&dir);
        rag.unlock(DataKey::random().unwrap()).unwrap();
        rag.replace_document("a.md", vec![chunk("m", vec![1.0])]).unwrap();

        rag.lock();
        assert!(rag.list_documents().is_empty());
        assert!(rag.is_empty());
        assert!(rag.replace_document("b.md", vec![chunk("m", vec![1.0])]).is_err());
        assert!(rag.delete_document("a.md").is_err());
        assert!(rag.list_documents().is_empty(), "a refused write changes nothing");
    }

    #[test]
    fn indexing_refuses_unsupported_or_dangerous_files() {
        // Not on the allow-list (could be a key, a database, a binary...)
        for name in ["id_rsa", "key.pem", "wallet.dat", "tool.exe", "noextension"] {
            let p = temp_file(name, b"data");
            assert!(read_and_chunk(&p).is_err(), "{name} must be refused");
        }
        // A directory named like a text file
        let dir = temp_file("x.txt", b"").parent().unwrap().join("folder.txt");
        fs::create_dir_all(&dir).unwrap();
        assert!(read_and_chunk(&dir).unwrap_err().contains("regular"));
        // Binary content with a text extension
        let bin = temp_file("blob.txt", &[0xff, 0xfe, 0x00, 0x80]);
        assert!(read_and_chunk(&bin).unwrap_err().contains("UTF-8"));
    }

    #[test]
    fn indexing_enforces_size_and_chunk_limits() {
        let big = temp_file("big.txt", &vec![b'a'; (MAX_FILE_BYTES + 1) as usize]);
        assert!(read_and_chunk(&big).unwrap_err().contains("too large"));

        // Many paragraphs that each become their own chunk -> too many embedding requests
        let para = "word ".repeat(300);
        let text = vec![para.as_str(); MAX_CHUNKS_PER_FILE + 50].join("\n\n");
        let many = temp_file("many.md", text.as_bytes());
        assert!(read_and_chunk(&many).unwrap_err().contains("embedding requests"));

        let ok = temp_file("fine.MD", b"hello\n\nworld");
        assert_eq!(read_and_chunk(&ok).unwrap().1, vec!["hello\n\nworld"]);
    }

    #[test]
    fn source_path_lookup_only_uses_the_store() {
        let mut rag = RagEngine::new();
        let mut c = chunk("m", vec![1.0]);
        c.source_path = "C:/docs/a.md".into();
        rag.chunks = vec![c];
        assert_eq!(rag.source_path_of("a.md").as_deref(), Some("C:/docs/a.md"));
        assert_eq!(rag.source_path_of("../../etc/passwd"), None);
        assert!(rag.list_documents()[0].can_reindex);
    }

    #[test]
    fn embeddings_url_variants() {
        assert_eq!(embeddings_url("https://a.com/v1/"), "https://a.com/v1/embeddings");
        assert_eq!(embeddings_url("http://localhost:11434"), "http://localhost:11434/v1/embeddings");
        assert_eq!(embeddings_url("https://a.com/v1/embeddings"), "https://a.com/v1/embeddings");
    }

    /// Prints how long a search over 5000 chunks of 1536 dimensions takes.
    /// Run: `cargo test --release search_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn search_timing() {
        let dims = 1536;
        let mut rag = RagEngine::new();
        rag.chunks = (0..5000)
            .map(|i| {
                let v: Vec<f32> = (0..dims).map(|d| (((i * 31 + d * 17) % 997) as f32) / 997.0 - 0.5).collect();
                chunk("m", v)
            })
            .collect();
        let query: Vec<f32> = (0..dims).map(|d| ((d * 13 % 991) as f32) / 991.0 - 0.5).collect();

        let t = std::time::Instant::now();
        let runs = 20;
        for _ in 0..runs {
            std::hint::black_box(rag.search(&query, "m", 3));
        }
        println!("[timing] search over 5000x{}: {:.2} ms per query", dims, t.elapsed().as_secs_f64() * 1000.0 / runs as f64);
    }
}
