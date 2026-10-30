use crate::types::{
    cosine_similarity, DocumentChunk, OpenAiEmbeddingResponse,
};
use reqwest::Client;
use serde_json::json;
use std::fs;
use std::path::Path;

pub struct RagEngine {
    client: Client,
    chunks: Vec<DocumentChunk>,
}

impl RagEngine {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            chunks: Vec::new(),
        }
    }

    pub async fn get_embedding(
        &self,
        base_url: &str,
        api_key: &str,
        text: &str,
    ) -> Result<Vec<f32>, String> {
        let clean_base = base_url.trim_end_matches('/');
        
        let url = if clean_base.ends_with("/embeddings") {
            clean_base.to_string()
        } else if clean_base.ends_with("/v1") {
            format!("{}/embeddings", clean_base)
        } else {
            format!("{}/v1/embeddings", clean_base)
        };

        // nvidia/nv-embed-v1 là model embedding chuẩn được cấp permission mặc định cho tất cả nvapi keys
        let embedding_model = if base_url.contains("nvidia.com") {
            "nvidia/nemotron-3-embed-1b"
        } else {
            "text-embedding-3-small"
        };

        println!("[RAG EMBEDDING] Sending request to: {} | Model: {}", url, embedding_model);

        let body = json!({
            "input": text,
            "model": embedding_model,
            "input_type": "passage" // Nvidia NIM yêu cầu input_type với một số model
        });

        let mut req = self.client.post(&url).json(&body);
        if !api_key.is_empty() {
            req = req.bearer_auth(api_key);
        }

        let res = req
            .send()
            .await
            .map_err(|e| format!("Network error sending embedding request: {}", e))?;

        let status = res.status();
        if !status.is_success() {
            let err_text = res.text().await.unwrap_or_default();
            return Err(format!(
                "API Error [HTTP {}] at {}:\n{}\n\n[Gợi ý]: Hãy kiểm tra lại API Key hoặc cấp quyền cho Embedding Models trên Nvidia Dashboard.",
                status, url, err_text
            ));
        }

        let parsed: OpenAiEmbeddingResponse = res
            .json()
            .await
            .map_err(|e| format!("Parse OpenAI embedding JSON error: {}", e))?;

        if let Some(first) = parsed.data.into_iter().next() {
            Ok(first.embedding)
        } else {
            Err("Embedding API returned empty data array".to_string())
        }
    }

    pub async fn index_file(
        &mut self,
        base_url: &str,
        api_key: &str,
        file_path: &str,
    ) -> Result<usize, String> {
        let path = Path::new(file_path);
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let content =
            fs::read_to_string(path).map_err(|e| format!("Failed to read file: {}", e))?;

        if content.trim().is_empty() {
            return Err("File rỗng!".to_string());
        }

        let mut raw_chunks: Vec<&str> = content
            .split("\n\n")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        if raw_chunks.is_empty() {
            raw_chunks.push(content.trim());
        }

        let mut count = 0;
        for chunk_str in raw_chunks {
            match self.get_embedding(base_url, api_key, chunk_str).await {
                Ok(embedding) => {
                    self.chunks.push(DocumentChunk {
                        file_name: file_name.clone(),
                        content: chunk_str.to_string(),
                        embedding,
                    });
                    count += 1;
                }
                Err(e) => {
                    println!("[RAG Embedding Error]: {}", e);
                    return Err(e);
                }
            }
        }

        Ok(count)
    }

    pub async fn search_relevant_context(
        &self,
        base_url: &str,
        api_key: &str,
        query: &str,
        top_k: usize,
    ) -> Result<String, String> {
        if self.chunks.is_empty() {
            return Ok(String::new());
        }

        let query_embedding = self.get_embedding(base_url, api_key, query).await?;

        let mut scored_chunks: Vec<(f32, &DocumentChunk)> = self
            .chunks
            .iter()
            .map(|chunk| {
                let score = cosine_similarity(&query_embedding, &chunk.embedding);
                (score, chunk)
            })
            .collect();

        scored_chunks.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut context_str = String::from("TRÍ TRÍ TỪ TÀI LIỆU CÁ NHÂN (RAG CONTEXT):\n");
        let mut found = false;

        for (_i, (score, chunk)) in scored_chunks.iter().take(top_k).enumerate() {
            if *score > 0.25 {
                found = true;
                context_str.push_str(&format!(
                    "\n--- [Tài liệu: {} | Score: {:.2}] ---\n{}\n",
                    chunk.file_name, score, chunk.content
                ));
            }
        }

        if found {
            Ok(context_str)
        } else {
            Ok(String::new())
        }
    }
}