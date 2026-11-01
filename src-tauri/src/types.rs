use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String, // system, user, assistant
    pub content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatPayload {
    pub session_id: String,
    /// The API key is looked up in Rust by profile id; the frontend never holds it
    pub profile_id: String,
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub embedding: EmbeddingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
    #[serde(default)]
    pub system_prompt: String,
    pub messages: Vec<ChatMessage>,
}

/// Embedding endpoint config for RAG (separate from the chat provider,
/// because Anthropic has no embeddings API).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingConfig {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,

    /// Only read from settings.json written by older versions, so the key can be migrated into
    /// secure storage. Never serialised, so it can reach neither the file nor the frontend.
    #[serde(default, skip_serializing)]
    pub api_key: String,
    /// Derived from secure storage; ignored when received from the frontend.
    #[serde(default, skip_deserializing, skip_serializing_if = "is_false")]
    pub has_api_key: bool,
    /// The origin the saved key is bound to (empty when there is no key).
    #[serde(default, skip_deserializing, skip_serializing_if = "String::is_empty")]
    pub key_origin: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A named connection config. Every field is entered by the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub base_url: String,
    pub model: String,

    /// Legacy, read-only: see `EmbeddingConfig::api_key`.
    #[serde(default, skip_serializing)]
    pub api_key: String,
    /// Derived from secure storage; ignored when received from the frontend.
    #[serde(default, skip_deserializing, skip_serializing_if = "is_false")]
    pub has_api_key: bool,
    /// The origin the saved key is bound to (empty when there is no key).
    #[serde(default, skip_deserializing, skip_serializing_if = "String::is_empty")]
    pub key_origin: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub active_profile_id: String,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub embedding: EmbeddingConfig,

    // Fields of the old single-provider settings.json. Read-only: the frontend
    // migrates them into a profile and they are never written back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Whether chat history can be saved right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Persistence {
    /// History is encrypted and saved normally
    Ok,
    /// The vault is locked: history is hidden and cannot be saved until it is unlocked
    Locked,
    /// There is no secure storage at all (no OS keychain and no vault yet), so history is not saved
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionsLoad {
    pub sessions: Vec<ChatSession>,
    pub persistence: Persistence,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsLoad {
    pub settings: Settings,
    pub warnings: Vec<String>,
    pub secrets: crate::secrets::SecretsStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    /// Model ids returned by the server (empty if the endpoint cannot list models)
    pub models: Vec<String>,
    pub models_listed: bool,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentChunk {
    pub file_name: String,
    pub content: String,
    pub embedding: Vec<f32>,
    /// Embedding model that produced `embedding`. Empty for chunks indexed by older versions,
    /// which are treated as incompatible because their model is unknown.
    #[serde(default)]
    pub model: String,
    /// Original file path, so a stale document can be re-indexed without picking it again.
    #[serde(default)]
    pub source_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentInfo {
    pub file_name: String,
    pub chunks: usize,
    pub model: String,
    /// Whether the original file path is known, so it can be re-indexed. The path itself is
    /// never sent to the frontend.
    pub can_reindex: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkEvent {
    pub session_id: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct OpenAiEmbeddingData {
    pub embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAiEmbeddingResponse {
    pub data: Vec<OpenAiEmbeddingData>,
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot_product / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_legacy_single_provider_settings() {
        let legacy = r#"{
            "provider": "openai", "baseUrl": "https://x/v1", "apiKey": "k", "model": "m",
            "embedding": { "baseUrl": "", "apiKey": "", "model": "" }
        }"#;
        let s: Settings = serde_json::from_str(legacy).unwrap();
        assert!(s.profiles.is_empty());
        assert_eq!(s.provider.as_deref(), Some("openai"));
        assert_eq!(s.api_key.as_deref(), Some("k"));
    }

    #[test]
    fn profile_settings_round_trip_without_legacy_fields() {
        let s = Settings {
            active_profile_id: "a".into(),
            profiles: vec![Profile {
                id: "a".into(),
                name: "NVIDIA".into(),
                provider: "openai".into(),
                base_url: "https://x/v1".into(),
                model: "m".into(),
                api_key: "legacy-never-written".into(),
                has_api_key: true,
                key_origin: "https://x".into(),
            }],
            embedding: EmbeddingConfig::default(),
            provider: None,
            base_url: None,
            api_key: None,
            model: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("\"provider\":null"));
        assert!(!json.contains("legacy-never-written"), "api_key must never be serialised");
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.profiles[0].name, "NVIDIA");
        assert!(back.provider.is_none());
    }
}
