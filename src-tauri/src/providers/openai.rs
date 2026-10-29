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
        Self {
            client: Client::new(),
        }
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
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

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
            .map_err(|e| format!("Network error: {}", e))?;

        if !res.status().is_success() {
            let err_text = res.text().await.unwrap_or_default();
            return Err(format!("API Error: {}", err_text));
        }

        let event_stream = res.bytes_stream().eventsource();

        let stream = event_stream.filter_map(|event| async move {
            match event {
                Ok(ev) => {
                    if ev.data == "[DONE]" {
                        None
                    } else if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&ev.data) {
                        if let Some(content) = parsed["choices"][0]["delta"]["content"].as_str() {
                            Some(Ok(content.to_string()))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                Err(e) => Some(Err(format!("Stream error: {}", e))),
            }
        });

        Ok(Box::pin(stream))
    }
}