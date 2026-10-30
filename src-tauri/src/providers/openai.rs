use super::{ChunkStream, LlmProvider};
use crate::types::ChatMessage;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::Client;
use serde_json::json;

pub struct OpenAiProvider {
    client: Client,
}

impl OpenAiProvider {
    pub fn new() -> Self {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self { client }
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn stream_chat(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        messages: Vec<ChatMessage>,
    ) -> Result<ChunkStream, String> {
        let clean_base = base_url.trim_end_matches('/');
        let url = if clean_base.ends_with("/chat/completions") {
            clean_base.to_string()
        } else if clean_base.ends_with("/v1") {
            format!("{}/chat/completions", clean_base)
        } else {
            format!("{}/v1/chat/completions", clean_base)
        };

        println!("\n========== [NEKKOCHAT DEBUG REQUEST] ==========");
        println!("[1] Final Target URL: {}", url);
        println!("[2] Model: {}", model);
        println!("[3] API Key Length: {}", api_key.len());

        let body = json!({
            "model": model,
            "messages": messages,
            "stream": true
        });

        println!("[4] Payload JSON:\n{}", serde_json::to_string_pretty(&body).unwrap_or_default());

        let mut req = self.client.post(&url).json(&body);
        if !api_key.is_empty() {
            req = req.bearer_auth(api_key);
        }

        println!("[5] Sending HTTP Request...");
        let res = req
            .send()
            .await
            .map_err(|e| {
                let err_msg = format!("Network/Connect error to {}: {}", url, e);
                println!("[E] {}", err_msg);
                err_msg
            })?;

        let status = res.status();
        let headers = res.headers().clone();
        println!("[6] Response HTTP Status: {}", status);
        println!("[7] Response Content-Type: {:?}", headers.get("content-type"));

        if !status.is_success() {
            let err_text = res.text().await.unwrap_or_default();
            println!("[E] API Error Response Body:\n{}", err_text);
            println!("================================================\n");
            return Err(format!("API Error [HTTP {}]: {}", status, err_text));
        }

        println!("[8] Status 200 OK! Constructing SSE stream...");
        println!("================================================\n");

        let event_stream = res.bytes_stream().eventsource();

        let stream = event_stream.filter_map(|event| async move {
            match event {
                Ok(ev) => {
                    if ev.data == "[DONE]" {
                        None
                    } else if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&ev.data) {
                        let delta = &parsed["choices"][0]["delta"];
                        
                        // Lấy content chính, nếu không có thì lấy reasoning_content (suy luận)
                        if let Some(content) = delta["content"].as_str() {
                            Some(Ok(content.to_string()))
                        } else if let Some(reasoning) = delta["reasoning_content"].as_str() {
                            // Trả về phần suy nghĩ của AI (có thể bọc tag để UI hiển thị nếu thích)
                            Some(Ok(reasoning.to_string()))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                Err(e) => {
                    let err_msg = format!("Stream SSE Parse Error: {}", e);
                    println!("[E] {}", err_msg);
                    Some(Err(err_msg))
                }
            }
        });

        Ok(Box::pin(stream))
    }
}