use super::{ChunkStream, LlmProvider};
use crate::types::ChatMessage;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::Client;
use serde_json::json;
pub struct AnthropicProvider {
    client: Client,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        let client = crate::net::api_client(None);

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

        // Anthropic takes the system prompt outside the messages array
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
            .header("x-api-key", api_key) // never forwarded: redirects are disabled
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

        if res.status().is_redirection() {
            return Err(format!("The endpoint tried to redirect (HTTP {}); redirects are refused", res.status()));
        }
        if !res.status().is_success() {
            let err_text = crate::net::truncate(
                &crate::net::read_text_lossy(res, crate::net::MAX_SMALL_BODY).await,
                500,
            );
            return Err(format!("Anthropic API Error: {}", err_text));
        }

        let event_stream = crate::net::cap_stream(res.bytes_stream(), crate::net::MAX_STREAM_BYTES).eventsource();

        let stream = event_stream.filter_map(|event| async move {
            match event {
                Ok(ev) => {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&ev.data) {
                        if parsed["type"] == "error" {
                            return Some(Err(parsed["error"]["message"]
                                .as_str()
                                .unwrap_or("unknown stream error")
                                .to_string()));
                        }
                        if parsed["type"] == "content_block_delta"
                            && let Some(text) = parsed["delta"]["text"].as_str()
                        {
                            return Some(Ok(text.to_string()));
                        }
                    }
                    None
                }
                Err(e) => Some(Err(format!("Stream error: {}", super::error_chain(&e)))),
            }
        });

        Ok(Box::pin(stream))
    }
}