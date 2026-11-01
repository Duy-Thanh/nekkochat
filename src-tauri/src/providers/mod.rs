pub mod anthropic;
pub mod openai;

use crate::types::{ChatMessage};
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

pub type ChunkStream = Pin<Box<dyn Stream<Item = Result<String, String>> + Send>>;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn stream_chat(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        messages: Vec<ChatMessage>,
    ) -> Result<ChunkStream, String>;
}

pub fn get_provider(provider_type: &str) -> Result<Box<dyn LlmProvider>, String> {
    match provider_type {
        "openai" | "ollama" => Ok(Box::new(openai::OpenAiProvider::new())),
        "anthropic" => Ok(Box::new(anthropic::AnthropicProvider::new())),
        _ => Err(format!("Unsupported provider: {}", provider_type)),
    }
}
/// Joins the whole `source()` chain so the root cause (reset, timeout, h2...) is visible
/// instead of just the outermost layer such as "error decoding response body".
pub fn error_chain(e: &dyn std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        msg.push_str(&format!(" <- {}", s));
        cur = s.source();
    }
    msg
}
