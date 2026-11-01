//! Network safety rules shared by every request that carries an API key or reads a server
//! response: endpoint validation, no redirects, and hard limits on how much data a server can
//! make the app buffer.

use futures::{Stream, StreamExt};
use reqwest::{Client, Url, redirect};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::Duration;

/// Error bodies, model lists and similar small JSON responses.
pub const MAX_SMALL_BODY: usize = 2 * 1024 * 1024;
/// An embedding response is a single vector, never megabytes.
pub const MAX_EMBEDDING_BODY: usize = 8 * 1024 * 1024;
/// Total raw bytes accepted from one streaming chat response (SSE framing included).
pub const MAX_STREAM_BYTES: usize = 32 * 1024 * 1024;

/// HTTP client for API calls. Redirects are disabled on purpose: reqwest only strips the
/// `Authorization` header on a cross-host redirect, so keys sent in other headers (such as
/// Anthropic's `x-api-key`) would be forwarded to whatever host a malicious or compromised
/// endpoint redirects to.
pub fn api_client(total_timeout: Option<Duration>) -> Client {
    let mut builder = Client::builder()
        .redirect(redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .tcp_keepalive(Duration::from_secs(30));
    if let Some(t) = total_timeout {
        builder = builder.timeout(t);
    }
    builder.build().unwrap_or_else(|_| Client::new())
}

fn is_private_v4(ip: Ipv4Addr) -> bool {
    ip.is_loopback() || ip.is_private()
}

fn is_private_v6(ip: Ipv6Addr) -> bool {
    // fc00::/7 is unique-local; loopback ::1
    ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00
}

/// Hosts where plain http is acceptable: this machine and private networks (a self-hosted
/// Ollama or vLLM). Link-local addresses are excluded because 169.254.169.254 is the cloud
/// metadata service.
fn is_local_host(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => is_private_v4(ip),
        Some(url::Host::Ipv6(ip)) => is_private_v6(ip),
        Some(url::Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            d == "localhost"
                || d.ends_with(".localhost")
                || d.ends_with(".local")
                // Single-label names such as `ollama-box` only resolve on a LAN
                || !d.contains('.')
        }
        None => false,
    }
}

/// Parses and validates an endpoint that an API key may be sent to.
pub fn validate_endpoint(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw.trim()).map_err(|_| format!("\"{}\" is not a valid URL", raw.trim()))?;

    match url.scheme() {
        "https" => {}
        "http" => {
            if !is_local_host(&url) {
                return Err(format!(
                    "Refusing to use plain http for {}: API keys and chats would travel unencrypted. Use https.",
                    url.host_str().unwrap_or("this host")
                ));
            }
        }
        other => return Err(format!("Unsupported URL scheme \"{}\" (use https)", other)),
    }
    if url.host_str().is_none() {
        return Err("The URL has no host".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLs with embedded credentials are not allowed".to_string());
    }
    Ok(url)
}

/// `scheme://host:port`, normalised. This is what a stored API key is bound to.
pub fn origin_of(raw: &str) -> Result<String, String> {
    Ok(validate_endpoint(raw)?.origin().ascii_serialization())
}

/// Reads at most `max` bytes of a response body; a bigger body is an error, not an OOM.
pub async fn read_limited(res: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    if res.content_length().is_some_and(|len| len as usize > max) {
        return Err(format!("Response is larger than the {} MiB limit", max / 1024 / 1024));
    }
    let mut body = Vec::new();
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Failed to read response: {}", e))?;
        if body.len() + chunk.len() > max {
            return Err(format!("Response is larger than the {} MiB limit", max / 1024 / 1024));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Like `read_limited`, lossy-decoded; for error messages. Never fails.
pub async fn read_text_lossy(res: reqwest::Response, max: usize) -> String {
    match read_limited(res, max).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => e,
    }
}

pub async fn read_json_limited<T: serde::de::DeserializeOwned>(
    res: reqwest::Response,
    max: usize,
) -> Result<T, String> {
    let bytes = read_limited(res, max).await?;
    serde_json::from_slice(&bytes).map_err(|e| format!("Server responded with invalid JSON: {}", e))
}

#[derive(Debug)]
pub struct NetError(pub String);

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NetError {}

/// Wraps a byte stream so that it fails once more than `max` bytes have gone through.
pub fn cap_stream<S, B, E>(stream: S, max: usize) -> impl Stream<Item = Result<B, NetError>>
where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut total = 0usize;
    stream.map(move |item| match item {
        Ok(bytes) => {
            total += bytes.as_ref().len();
            if total > max {
                Err(NetError(format!("Response exceeded the {} MiB limit", max / 1024 / 1024)))
            } else {
                Ok(bytes)
            }
        }
        Err(e) => Err(NetError(e.to_string())),
    })
}

/// Truncates a string for messages, on a char boundary.
pub fn truncate(s: &str, max_chars: usize) -> String {
    let mut out: String = s.chars().take(max_chars).collect();
    if s.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_is_accepted_anywhere() {
        assert!(validate_endpoint("https://api.openai.com/v1").is_ok());
        assert!(validate_endpoint("https://integrate.api.nvidia.com/v1").is_ok());
    }

    #[test]
    fn http_is_only_accepted_for_local_and_private_hosts() {
        for ok in [
            "http://localhost:11434",
            "http://127.0.0.1:8080/v1",
            "http://[::1]:8080",
            "http://192.168.1.50:8000/v1",
            "http://10.0.0.7",
            "http://172.20.0.3",
            "http://my-box:11434",
            "http://printer.local",
        ] {
            assert!(validate_endpoint(ok).is_ok(), "{ok} should be allowed");
        }
        for bad in [
            "http://api.openai.com/v1",
            "http://8.8.8.8",
            "http://203.0.113.5",
            "http://169.254.169.254/latest/meta-data",
            "http://172.32.0.1", // just outside 172.16/12
            "http://evil.example.com",
        ] {
            assert!(validate_endpoint(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn other_schemes_and_embedded_credentials_are_refused() {
        for bad in [
            "file:///etc/passwd",
            "ftp://example.com",
            "javascript:alert(1)",
            "https://user:pass@example.com",
            "https://user@example.com",
            "not a url",
            "",
        ] {
            assert!(validate_endpoint(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn origin_is_normalised() {
        assert_eq!(origin_of("https://API.OpenAI.com/v1/").unwrap(), "https://api.openai.com");
        assert_eq!(origin_of("https://a.com:443/x").unwrap(), "https://a.com");
        assert_eq!(origin_of("http://localhost:11434/v1").unwrap(), "http://localhost:11434");
        assert_ne!(origin_of("https://a.com").unwrap(), origin_of("https://a.com:8443").unwrap());
        // Same host, different scheme, is a different origin
        assert_ne!(origin_of("http://localhost").unwrap(), origin_of("https://localhost").unwrap());
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("ệệệệệ", 3), "ệệệ...");
        assert_eq!(truncate("abc", 3), "abc");
    }

    #[tokio::test]
    async fn cap_stream_fails_when_the_limit_is_exceeded() {
        let chunks: Vec<Result<Vec<u8>, String>> = vec![Ok(vec![0; 6]), Ok(vec![0; 6])];
        let capped: Vec<_> = cap_stream(futures::stream::iter(chunks), 10).collect().await;
        assert!(capped[0].is_ok());
        assert!(capped[1].as_ref().unwrap_err().0.contains("exceeded"));
    }
}
