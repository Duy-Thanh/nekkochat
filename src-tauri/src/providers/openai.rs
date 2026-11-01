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
        let client = crate::net::api_client(None);

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

        let body = json!({
            "model": model,
            "messages": messages,
            "stream": true
        });

        let mut req = self.client.post(&url).json(&body);
        if !api_key.is_empty() {
            req = req.bearer_auth(api_key);
        }

        let res = req
            .send()
            .await
            .map_err(|e| format!("Network/Connect error to {}: {}", url, e))?;

        let status = res.status();
        if status.is_redirection() {
            return Err(format!("The endpoint tried to redirect (HTTP {}); redirects are refused", status));
        }
        if !status.is_success() {
            let err_text = crate::net::truncate(
                &crate::net::read_text_lossy(res, crate::net::MAX_SMALL_BODY).await,
                500,
            );
            return Err(format!("API Error [HTTP {}]: {}", status, err_text));
        }

        let event_stream = crate::net::cap_stream(res.bytes_stream(), crate::net::MAX_STREAM_BYTES).eventsource();

        let stream = event_stream.filter_map(|event| async move {
            match event {
                Ok(ev) => {
                    if ev.data == "[DONE]" {
                        return None;
                    }
                    let parsed = serde_json::from_str::<serde_json::Value>(&ev.data).ok()?;
                    let delta = &parsed["choices"][0]["delta"];

                    if let Some(content) = delta["content"].as_str() {
                        return Some(Ok(content.to_string()));
                    }
                    // Wrap reasoning content in <think> tags so the frontend can parse it
                    delta["reasoning_content"]
                        .as_str()
                        .or_else(|| delta["reasoning"].as_str())
                        .map(|reasoning| Ok(format!("<think>{}</think>", reasoning)))
                }
                Err(e) => Some(Err(format!("Stream error: {}", super::error_chain(&e)))),
            }
        });

        Ok(Box::pin(stream))
    }
}