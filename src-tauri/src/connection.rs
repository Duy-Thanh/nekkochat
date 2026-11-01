use crate::net;
use crate::rag::ResolvedEmbedding;
use crate::types::ConnectionInfo;
use reqwest::Client;
use serde_json::Value;
use std::time::{Duration, Instant};

const TEST_TIMEOUT: Duration = Duration::from_secs(15);

fn models_url(provider: &str, base_url: &str) -> String {
    let mut base = base_url.trim().trim_end_matches('/');
    if let Some(stripped) = base.strip_suffix("/chat/completions") {
        base = stripped;
    }
    if provider == "anthropic" {
        format!("{}/models?limit=1000", base)
    } else if base.ends_with("/v1") {
        format!("{}/models", base)
    } else {
        format!("{}/v1/models", base)
    }
}

/// Extract model ids from the common response shapes:
/// OpenAI/Anthropic/Ollama-compat `{ "data": [{ "id": .. }] }` or `{ "models": [{ "name"|"id": .. }] }`.
fn parse_models(body: &Value) -> Vec<String> {
    let list = body["data"].as_array().or_else(|| body["models"].as_array());
    let mut ids: Vec<String> = list
        .map(|items| {
            items
                .iter()
                .filter_map(|m| m["id"].as_str().or_else(|| m["name"].as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    ids
}

/// Non-destructive connectivity check: lists models. Does not send any chat request.
/// A 404/405 means the server is reachable but has no model listing, which is reported
/// as `models_listed = false` rather than as a failure.
pub async fn test_connection(
    client: &Client,
    provider: &str,
    base_url: &str,
    api_key: &str,
) -> Result<ConnectionInfo, String> {
    if base_url.trim().is_empty() {
        return Err("Base URL is empty".to_string());
    }
    net::validate_endpoint(base_url)?;
    let url = models_url(provider, base_url);

    let mut req = client.get(&url).timeout(TEST_TIMEOUT);
    if provider == "anthropic" {
        req = req
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01");
    } else if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }

    let started = Instant::now();
    let res = req.send().await.map_err(|e| {
        let kind = if e.is_timeout() {
            "timed out"
        } else if e.is_connect() {
            "could not connect"
        } else {
            "request failed"
        };
        format!("{} ({}): {}", kind, url, crate::providers::error_chain(&e))
    })?;
    let latency_ms = started.elapsed().as_millis() as u64;

    let status = res.status();
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::METHOD_NOT_ALLOWED {
        return Ok(ConnectionInfo { models: Vec::new(), models_listed: false, latency_ms });
    }
    if status.is_redirection() {
        return Err(format!("The endpoint tried to redirect (HTTP {}); redirects are refused", status));
    }
    if !status.is_success() {
        let body = net::read_text_lossy(res, net::MAX_SMALL_BODY).await;
        let hint = match status.as_u16() {
            401 | 403 => " (check the API key)",
            _ => "",
        };
        return Err(format!("HTTP {}{}: {}", status.as_u16(), hint, net::truncate(&body, 300)));
    }

    let body: Value = net::read_json_limited(res, net::MAX_SMALL_BODY).await?;
    let models = parse_models(&body);
    Ok(ConnectionInfo { models_listed: !models.is_empty(), models, latency_ms })
}

/// Embeds a short string and reports the vector size.
pub async fn test_embedding(client: &Client, cfg: &ResolvedEmbedding) -> Result<String, String> {
    let started = Instant::now();
    let v = crate::rag::embed(client, cfg, "connection test", true).await?;
    Ok(format!(
        "OK: {}-dimension vector in {} ms",
        v.len(),
        started.elapsed().as_millis()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn models_url_variants() {
        assert_eq!(models_url("openai", "https://a.com/v1/"), "https://a.com/v1/models");
        assert_eq!(models_url("ollama", "http://localhost:11434"), "http://localhost:11434/v1/models");
        assert_eq!(
            models_url("openai", "https://a.com/v1/chat/completions"),
            "https://a.com/v1/models"
        );
        assert_eq!(
            models_url("anthropic", "https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1/models?limit=1000"
        );
    }

    #[test]
    fn parses_openai_and_ollama_shapes() {
        let openai = json!({ "data": [{ "id": "b" }, { "id": "a" }, { "id": "a" }] });
        assert_eq!(parse_models(&openai), vec!["a", "b"]);
        let ollama = json!({ "models": [{ "name": "llama3" }] });
        assert_eq!(parse_models(&ollama), vec!["llama3"]);
        assert!(parse_models(&json!({})).is_empty());
    }
}
