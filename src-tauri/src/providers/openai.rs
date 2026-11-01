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
            // Không đặt timeout tổng: stream dài sẽ bị cắt. Timeout theo từng chunk nằm ở lib.rs
            .connect_timeout(std::time::Duration::from_secs(15))
            .tcp_keepalive(std::time::Duration::from_secs(30))
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
        if !status.is_success() {
            let err_text = res.text().await.unwrap_or_default();
            return Err(format!("API Error [HTTP {}]: {}", status, err_text));
        }

        let event_stream = res.bytes_stream().eventsource();

        let stream = event_stream.filter_map(|event| async move {
            match event {
                Ok(ev) => {
                    if ev.data == "[DONE]" {
                        None
                    } else if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&ev.data) {
                        let delta = &parsed["choices"][0]["delta"];
                        
                        if let Some(content) = delta["content"].as_str() {
                            Some(Ok(content.to_string()))
                        } else if let Some(reasoning) = delta["reasoning_content"]
                            .as_str()
                            .or_else(|| delta["reasoning"].as_str())
                        {
                            // Bọc reasoning content trong tag <think> để frontend dễ parse
                            Some(Ok(format!("<think>{}</think>", reasoning)))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                Err(e) => Some(Err(format!("Stream error: {}", super::error_chain(&e)))),
            }
        });

        Ok(Box::pin(stream))
    }
}