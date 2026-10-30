use super::{ChunkStream, LlmProvider};
use crate::types::ChatMessage;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::Client;
use serde_json::json;
use std::time::Duration;
pub struct AnthropicProvider {
    client: Client,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        let client = Client::builder()
                    // setup timeout
                    .connect_timeout(Duration::from_secs(30))
                    .timeout(Duration::from_secs(60))
                    .build()
                    .unwrap_or_else(|_| Client::new());

        Self { client }
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn stream_chat(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        messages: Vec<ChatMessage>,
    ) -> Result<ChunkStream, String> {
        let url = format!("{}/messages", base_url.trim_end_matches('/'));

        // Anthropic tách system prompt ra ngoài array messages
        let mut system_prompt = String::new();
        let filtered_messages: Vec<serde_json::Value> = messages
            .into_iter()
            .filter_map(|m| {
                if m.role == "system" {
                    system_prompt = m.content;
                    None
                } else {
                    Some(json!({
                        "role": m.role,
                        "content": m.content
                    }))
                }
            })
            .collect();

        let mut body = json!({
            "model": model,
            "messages": filtered_messages,
            "max_tokens": 4096,
            "stream": true
        });

        if !system_prompt.is_empty() {
            body["system"] = json!(system_prompt);
        }

        let res = self
            .client
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

        if !res.status().is_success() {
            let err_text = res.text().await.unwrap_or_default();
            return Err(format!("Anthropic API Error: {}", err_text));
        }

        let event_stream = res.bytes_stream().eventsource();

        let stream = event_stream.filter_map(|event| async move {
            match event {
                Ok(ev) => {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&ev.data) {
                        if parsed["type"] == "content_block_delta" {
                            if let Some(text) = parsed["delta"]["text"].as_str() {
                                return Some(Ok(text.to_string()));
                            }
                        }
                    }
                    None
                }
                Err(e) => Some(Err(format!("Stream error: {}", e))),
            }
        });

        Ok(Box::pin(stream))
    }
}