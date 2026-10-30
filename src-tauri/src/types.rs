use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String, // system, user, assistant
    pub content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatPayload {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub messages: Vec<ChatMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
    pub messages: Vec<ChatMessage>,
}

// #[derive(Debug, Clone, Serialize)]
// pub enum StreamChunk {
//     Content(String),
//     Error(String),
//     Done,
// }