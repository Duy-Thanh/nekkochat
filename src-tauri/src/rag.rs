use crate::types::{
    cosine_similarity, DocumentChunk, DocumentInfo, EmbeddingConfig, OpenAiEmbeddingResponse,
};
use reqwest::Client;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_CHUNK_CHARS: usize = 1200;
const MIN_SCORE: f32 = 0.25;

/// Store chỉ giữ vector + file trên đĩa. Việc gọi mạng embedding nằm ngoài
/// struct này để không giữ mutex trong lúc chờ API (chat vẫn chạy khi đang index).
pub struct RagEngine {
    chunks: Vec<DocumentChunk>,
    storage_path: Option<PathBuf>,
}

impl RagEngine {
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            storage_path: None,
        }
    }

    // Set đường dẫn lưu rag_store.json và tự động load vector cũ lên RAM
    pub fn init_storage(&mut self, app_dir: PathBuf) {
        let store_file = app_dir.join("rag_store.json");
        self.storage_path = Some(store_file.clone());

        if let Ok(content) = fs::read_to_string(&store_file) {
            match serde_json::from_str::<Vec<DocumentChunk>>(&content) {
                Ok(saved) => self.chunks = saved,
                Err(e) => {
                    // Không ghi đè file hỏng: giữ lại bản backup cho user
                    eprintln!("[RAG] rag_store.json is corrupted ({}); backed up to .bak", e);
                    let _ = fs::rename(&store_file, store_file.with_extension("json.bak"));
                }
            }
        }
    }

    fn save_to_disk(&self) -> Result<(), String> {
        let Some(path) = &self.storage_path else {
            return Ok(());
        };
        let content = serde_json::to_string(&self.chunks)
            .map_err(|e| format!("Failed to serialize RAG store: {}", e))?;
        crate::write_atomic(path, &content)
    }

    pub fn list_documents(&self) -> Vec<DocumentInfo> {
        let mut docs: Vec<DocumentInfo> = Vec::new();
        for c in &self.chunks {
            match docs.iter_mut().find(|d| d.file_name == c.file_name) {
                Some(d) => d.chunks += 1,
                None => docs.push(DocumentInfo {
                    file_name: c.file_name.clone(),
                    chunks: 1,
                }),
            }
        }
        docs
    }

    pub fn delete_document(&mut self, file_name: &str) -> Result<(), String> {
        self.chunks.retain(|c| c.file_name != file_name);
        self.save_to_disk()
    }

    /// Thay toàn bộ chunk cũ của file bằng chunk mới (index lại không bị trùng).
    pub fn replace_document(
        &mut self,
        file_name: &str,
        new_chunks: Vec<DocumentChunk>,
    ) -> Result<usize, String> {
        let count = new_chunks.len();
        self.chunks.retain(|c| c.file_name != file_name);
        self.chunks.extend(new_chunks);
        self.save_to_disk()?;
        Ok(count)
    }

    /// Chấm điểm các chunk theo query embedding đã tính sẵn.
    pub fn search(&self, query_embedding: &[f32], top_k: usize) -> String {
        let mut scored: Vec<(f32, &DocumentChunk)> = self
            .chunks
            .iter()
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

/// `is_query`: các model bất đối xứng của NVIDIA cần phân biệt query / passage.
pub async fn embed(
    client: &Client,
    cfg: &EmbeddingConfig,
    text: &str,
    is_query: bool,
) -> Result<Vec<f32>, String> {
    if cfg.base_url.is_empty() {
        return Err("Embedding Base URL is not configured".to_string());
    }
    let url = embeddings_url(&cfg.base_url);
    let is_nvidia = cfg.base_url.contains("nvidia.com");

    let model = if !cfg.model.is_empty() {
        cfg.model.as_str()
    } else if is_nvidia {
        "nvidia/nemotron-3-embed-1b"
    } else {
        "text-embedding-3-small"
    };

    let mut body = json!({ "input": text, "model": model });
    if is_nvidia {
        // OpenAI chuẩn sẽ từ chối field lạ, nên chỉ gửi cho NVIDIA NIM
        body["input_type"] = json!(if is_query { "query" } else { "passage" });
    }

    let mut req = client.post(&url).json(&body);
    if !cfg.api_key.is_empty() {
        req = req.bearer_auth(&cfg.api_key);
    }

    let res = req
        .send()
        .await
        .map_err(|e| format!("Network error sending embedding request: {}", e))?;

    let status = res.status();
    if !status.is_success() {
        let err_text = res.text().await.unwrap_or_default();
        return Err(format!("Embedding API Error [HTTP {}] at {}: {}", status, url, err_text));
    }

    let parsed: OpenAiEmbeddingResponse = res
        .json()
        .await
        .map_err(|e| format!("Parse embedding error: {}", e))?;

    parsed
        .data
        .into_iter()
        .next()
        .map(|d| d.embedding)
        .ok_or_else(|| "Embedding API returned empty data array".to_string())
}

/// Đọc file và cắt thành các chunk <= MAX_CHUNK_CHARS (gộp đoạn ngắn, tách đoạn dài).
pub fn read_and_chunk(path: &Path) -> Result<(String, Vec<String>), String> {
    let file_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let content = fs::read_to_string(path)
        .map_err(|e| format!("Could not read file (UTF-8 text required): {}", e))?;

    let chunks = chunk_text(&content);
    if chunks.is_empty() {
        return Err("File is empty".to_string());
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
        // Đoạn quá dài: tách cứng theo số ký tự, đúng ranh giới char
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
        // Tiếng Việt nhiều byte/ký tự: không được panic khi cắt
        let text = "ệ".repeat(MAX_CHUNK_CHARS * 2 + 5);
        let chunks = chunk_text(&text);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= MAX_CHUNK_CHARS));
    }

    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(chunk_text("  \n\n  ").is_empty());
    }

    #[test]
    fn embeddings_url_variants() {
        assert_eq!(embeddings_url("https://a.com/v1/"), "https://a.com/v1/embeddings");
        assert_eq!(embeddings_url("http://localhost:11434"), "http://localhost:11434/v1/embeddings");
        assert_eq!(embeddings_url("https://a.com/v1/embeddings"), "https://a.com/v1/embeddings");
    }
}
