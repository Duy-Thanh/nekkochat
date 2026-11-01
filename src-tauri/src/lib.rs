mod connection;
mod providers;
mod rag;
mod secrets;
mod types;
mod vault;

use futures::StreamExt;
use providers::get_provider;
use rag::RagEngine;
use reqwest::Client;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;
use tokio::task::AbortHandle;
use tokio::time::timeout;

use types::{
    ChatMessage, ChatPayload, ChatSession, ChunkEvent, DocumentChunk, DocumentInfo,
    EmbeddingConfig, Settings, SettingsLoad,
};

// Max wait for the next chunk (reasoning models can "think" for quite a while)
const CHUNK_TIMEOUT: Duration = Duration::from_secs(90);
const RAG_TOP_K: usize = 3;

pub struct AppState {
    pub rag: Arc<Mutex<RagEngine>>,
    /// Where API keys live: the OS keychain, or an encrypted vault when there is none
    pub secrets: secrets::Backend,
    /// Secret keys that failed to load; protects them from being deleted by a later save
    pub unreadable_keys: StdMutex<HashSet<String>>,
    pub http: Client,
    /// session_id -> streaming task, so the Stop button can abort it
    pub tasks: Arc<StdMutex<HashMap<String, AbortHandle>>>,
}

/// Writes to a temp file then renames, so a crash mid-write cannot corrupt the data.
pub(crate) fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content).map_err(|e| format!("Failed to write {:?}: {}", tmp, e))?;
    fs::rename(&tmp, path).map_err(|e| format!("Failed to replace {:?}: {}", path, e))
}

/// The folder holding all app data. `NEKKOCHAT_DATA_DIR` overrides the default location,
/// which allows a portable install or running a second, fully isolated instance.
fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = match std::env::var_os("NEKKOCHAT_DATA_DIR") {
        Some(custom) if !custom.is_empty() => PathBuf::from(custom),
        _ => app
            .path()
            .app_data_dir()
            .map_err(|e| format!("Failed to resolve app data dir: {}", e))?,
    };

    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create directory {:?}: {}", dir, e))?;
    }
    Ok(dir)
}

fn app_file(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join(name))
}

/// Embedding defaults to the chat endpoint (except Anthropic, which has no embeddings API).
fn resolve_embedding(
    provider: &str,
    base_url: &str,
    api_key: &str,
    cfg: &EmbeddingConfig,
) -> EmbeddingConfig {
    let mut out = cfg.clone();
    if out.base_url.trim().is_empty() {
        if provider != "anthropic" {
            out.base_url = base_url.to_string();
            if out.api_key.is_empty() {
                out.api_key = api_key.to_string();
            }
        }
    }
    out
}

#[tauri::command]
async fn index_document_command(
    app: AppHandle,
    state: State<'_, AppState>,
    provider: String,
    base_url: String,
    api_key: String,
    embedding: EmbeddingConfig,
    file_path: String,
) -> Result<String, String> {
    let cfg = resolve_embedding(&provider, &base_url, &api_key, &embedding);
    let (file_name, texts) = rag::read_and_chunk(Path::new(&file_path))?;

    // Call the embedding API WITHOUT holding the RAG lock so chat keeps working while indexing.
    let total = texts.len();
    let mut chunks = Vec::with_capacity(total);
    for (i, text) in texts.into_iter().enumerate() {
        let embedding = rag::embed(&state.http, &cfg, &text, false).await?;
        chunks.push(DocumentChunk {
            file_name: file_name.clone(),
            content: text,
            embedding,
            model: cfg.model.trim().to_string(),
            source_path: file_path.clone(),
        });
        let _ = app.emit("rag-progress", format!("{}/{}", i + 1, total));
    }

    let count = state.rag.lock().await.replace_document(&file_name, chunks)?;
    Ok(format!("Indexed {} chunks from {}", count, file_name))
}

#[tauri::command]
async fn test_connection_command(
    state: State<'_, AppState>,
    provider: String,
    base_url: String,
    api_key: String,
) -> Result<types::ConnectionInfo, String> {
    connection::test_connection(&state.http, &provider, &base_url, &api_key).await
}

#[tauri::command]
async fn test_embedding_command(
    state: State<'_, AppState>,
    provider: String,
    base_url: String,
    api_key: String,
    embedding: EmbeddingConfig,
) -> Result<String, String> {
    let cfg = resolve_embedding(&provider, &base_url, &api_key, &embedding);
    connection::test_embedding(&state.http, &cfg).await
}

#[tauri::command]
async fn list_documents(state: State<'_, AppState>) -> Result<Vec<DocumentInfo>, String> {
    Ok(state.rag.lock().await.list_documents())
}

#[tauri::command]
async fn delete_document(state: State<'_, AppState>, file_name: String) -> Result<(), String> {
    state.rag.lock().await.delete_document(&file_name)
}

#[tauri::command]
async fn load_settings(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<SettingsLoad>, String> {
    let path = app_file(&app, "settings.json")?;
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).map_err(|e| format!("Failed to read settings: {}", e))?;
    let mut settings: Settings = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(e) => {
            // Don't let the next save overwrite a file we couldn't parse
            let backup = path.with_extension("json.bak");
            let _ = fs::copy(&path, &backup);
            eprintln!("[settings] settings.json is invalid ({}); backed up to {:?}", e, backup);
            return Ok(None);
        }
    };

    // With a locked or missing vault there is nothing to read: keys stay empty until unlock
    let warnings = match state.secrets.store() {
        Some(store) => {
            let mut unreadable = state.unreadable_keys.lock().unwrap();
            secrets::apply_on_load(store, &mut settings, &mut unreadable)
        }
        None => Vec::new(),
    };
    Ok(Some(SettingsLoad { settings, warnings, secrets: state.secrets.status() }))
}

/// Returns warnings (for example a key that could not be stored securely).
/// API keys are never written to settings.json, whatever happens.
#[tauri::command]
async fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: Settings,
) -> Result<Vec<String>, String> {
    let path = app_file(&app, "settings.json")?;

    let previous_ids: Vec<String> = fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<Settings>(&c).ok())
        .map(|s| s.profiles.into_iter().map(|p| p.id).collect())
        .unwrap_or_default();

    let warnings = match state.secrets.store() {
        Some(store) => {
            let mut unreadable = state.unreadable_keys.lock().unwrap();
            secrets::move_on_save(store, &mut settings, &previous_ids, &mut unreadable)
        }
        None => {
            let dropped = secrets::withhold_keys(&mut settings);
            match (dropped, state.secrets.status()) {
                (0, _) => Vec::new(),
                (n, secrets::SecretsStatus::VaultLocked) => vec![format!(
                    "{} API key(s) were not saved because the vault is locked. Unlock it first.",
                    n
                )],
                (n, _) => vec![format!(
                    "{} API key(s) were not saved: no OS keychain is available. Set a master password in Settings to store them encrypted.",
                    n
                )],
            }
        }
    };

    let content = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    write_atomic(&path, &content)?;
    Ok(warnings)
}

#[tauri::command]
fn secrets_status(state: State<'_, AppState>) -> secrets::SecretsStatus {
    state.secrets.status()
}

/// Runs a (slow, CPU/memory-heavy) vault operation off the async runtime and returns the new status.
async fn run_vault<F>(state: &State<'_, AppState>, op: F) -> Result<secrets::SecretsStatus, String>
where
    F: FnOnce(&vault::Vault) -> Result<(), String> + Send + 'static,
{
    let vault = state.secrets.vault()?.clone();
    tauri::async_runtime::spawn_blocking(move || op(&vault))
        .await
        .map_err(|e| e.to_string())??;
    Ok(state.secrets.status())
}

#[tauri::command]
async fn vault_create(
    state: State<'_, AppState>,
    passphrase: String,
) -> Result<secrets::SecretsStatus, String> {
    let passphrase = zeroize::Zeroizing::new(passphrase);
    run_vault(&state, move |v| v.create(&passphrase)).await
}

#[tauri::command]
async fn vault_unlock(
    state: State<'_, AppState>,
    passphrase: String,
) -> Result<secrets::SecretsStatus, String> {
    let passphrase = zeroize::Zeroizing::new(passphrase);
    run_vault(&state, move |v| v.unlock(&passphrase)).await
}

#[tauri::command]
async fn vault_lock(state: State<'_, AppState>) -> Result<secrets::SecretsStatus, String> {
    run_vault(&state, |v| {
        v.lock();
        Ok(())
    })
    .await
}

#[tauri::command]
async fn vault_change_passphrase(
    state: State<'_, AppState>,
    old: String,
    new: String,
) -> Result<secrets::SecretsStatus, String> {
    let (old, new) = (zeroize::Zeroizing::new(old), zeroize::Zeroizing::new(new));
    run_vault(&state, move |v| v.change_passphrase(&old, &new)).await
}

/// Deletes the vault and all keys in it. For when the master password is lost.
#[tauri::command]
async fn vault_reset(state: State<'_, AppState>) -> Result<secrets::SecretsStatus, String> {
    run_vault(&state, |v| v.reset()).await
}

#[tauri::command]
async fn load_sessions(app: AppHandle) -> Result<Vec<ChatSession>, String> {
    let path = app_file(&app, "sessions.json")?;
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read sessions: {}", e))?;
    match serde_json::from_str(&content) {
        Ok(sessions) => Ok(sessions),
        Err(e) => {
            // Must not return an empty list: the next save would overwrite and lose the history
            let backup = path.with_extension("json.bak");
            let _ = fs::copy(&path, &backup);
            Err(format!(
                "sessions.json could not be read ({}). A backup was saved at {:?}",
                e, backup
            ))
        }
    }
}

#[tauri::command]
async fn save_sessions(app: AppHandle, sessions: Vec<ChatSession>) -> Result<(), String> {
    let path = app_file(&app, "sessions.json")?;
    let content = serde_json::to_string(&sessions)
        .map_err(|e| format!("Failed to serialize sessions: {}", e))?;
    write_atomic(&path, &content)
}

#[tauri::command]
fn stop_chat_command(state: State<'_, AppState>, session_id: String) {
    if let Some(handle) = state.tasks.lock().unwrap().remove(&session_id) {
        handle.abort();
    }
}

/// Merges all system messages into one (Anthropic accepts a single `system`,
/// and some local servers also require it to come first and be unique).
fn merge_system_messages(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let (system, rest): (Vec<_>, Vec<_>) = messages.into_iter().partition(|m| m.role == "system");
    let mut out = Vec::with_capacity(rest.len() + 1);
    if !system.is_empty() {
        let joined = system
            .into_iter()
            .map(|m| m.content)
            .collect::<Vec<_>>()
            .join("\n\n");
        out.push(ChatMessage {
            role: "system".to_string(),
            content: joined,
        });
    }
    out.extend(rest);
    out
}

/// Returns RAG context for `query`, or None when there is nothing usable.
/// Emits `rag-warning` whenever documents exist but cannot be used, so RAG is never skipped silently.
async fn rag_context(
    app: &AppHandle,
    rag: &Arc<Mutex<RagEngine>>,
    http: &Client,
    payload: &ChatPayload,
    query: &str,
) -> Option<String> {
    if rag.lock().await.is_empty() {
        return None;
    }

    let cfg = resolve_embedding(
        &payload.provider,
        &payload.base_url,
        &payload.api_key,
        &payload.embedding,
    );
    let model = cfg.model.trim().to_string();
    let warn = |msg: String| {
        let _ = app.emit("rag-warning", msg);
    };

    if model.is_empty() {
        warn("Documents are indexed but no embedding model is set, so RAG was skipped. Set one in Settings.".into());
        return None;
    }

    let (usable, stale) = rag.lock().await.compatibility(&model);
    if usable == 0 {
        warn(format!(
            "All indexed documents were created with a different embedding model than \"{}\", so RAG was skipped. Re-index them.",
            model
        ));
        return None;
    }
    if stale > 0 {
        warn(format!(
            "{} chunk(s) from documents indexed with another embedding model were skipped. Re-index them.",
            stale
        ));
    }

    match rag::embed(http, &cfg, query, true).await {
        Ok(q) => {
            let ctx = rag.lock().await.search(&q, &model, RAG_TOP_K);
            (!ctx.is_empty()).then_some(ctx)
        }
        Err(e) => {
            warn(format!("RAG skipped: {}", e));
            None
        }
    }
}

#[tauri::command]
async fn stream_chat_command(
    app: AppHandle,
    state: State<'_, AppState>,
    payload: ChatPayload,
) -> Result<(), String> {
    let rag_engine = state.rag.clone();
    let http = state.http.clone();
    let tasks = state.tasks.clone();
    let session_id = payload.session_id.clone();

    // Hold the lock until the handle is registered, to avoid racing with stop_chat_command
    let mut tasks_guard = tasks.lock().unwrap();
    if tasks_guard.contains_key(&session_id) {
        return Err("This session is already generating a response".to_string());
    }

    let sid = session_id.clone();
    let tasks_for_task = tasks.clone();
    let join = tokio::spawn(async move {
        struct DoneGuard {
            app: AppHandle,
            session_id: String,
            tasks: Arc<StdMutex<HashMap<String, AbortHandle>>>,
        }
        impl Drop for DoneGuard {
            // Runs even when the task is aborted (Stop button)
            fn drop(&mut self) {
                self.tasks.lock().unwrap().remove(&self.session_id);
                let _ = self.app.emit("chat-done", self.session_id.clone());
            }
        }
        let _guard = DoneGuard {
            app: app.clone(),
            session_id: sid.clone(),
            tasks: tasks_for_task,
        };

        let emit = |text: String| {
            let _ = app.emit(
                "chat-chunk",
                ChunkEvent {
                    session_id: sid.clone(),
                    text,
                },
            );
        };

        let mut payload = payload;

        // 1. RAG: embed the query outside the lock, only lock while scoring
        let user_query = payload
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone())
            .unwrap_or_default();

        if !user_query.is_empty() {
            if let Some(ctx) = rag_context(&app, &rag_engine, &http, &payload, &user_query).await {
                payload.messages.push(ChatMessage {
                    role: "system".to_string(),
                    content: format!(
                        "Use the document excerpts below to answer if they are relevant:
{}",
                        ctx
                    ),
                });
            }
        }
        let messages = merge_system_messages(payload.messages);

        // 2. Streaming
        let provider = match get_provider(&payload.provider) {
            Ok(p) => p,
            Err(e) => {
                emit(format!("[ERROR]: {}", e));
                return;
            }
        };

        match provider
            .stream_chat(&payload.base_url, &payload.api_key, &payload.model, messages)
            .await
        {
            Ok(mut stream) => loop {
                match timeout(CHUNK_TIMEOUT, stream.next()).await {
                    Ok(Some(Ok(text))) => emit(text),
                    Ok(Some(Err(err))) => {
                        emit(format!("\n[Stream Error]: {}", err));
                        break;
                    }
                    Ok(None) => break,
                    Err(_) => {
                        emit(format!(
                            "\n[Timeout Error]: Server did not respond for over {}s",
                            CHUNK_TIMEOUT.as_secs()
                        ));
                        break;
                    }
                }
            },
            Err(err) => emit(format!("[Request Error]: {}", err)),
        }
    });

    tasks_guard.insert(session_id, join.abort_handle());
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: called before any other thread is created
        unsafe {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = data_dir(app.handle())?;

            let mut rag = RagEngine::new();
            rag.init_storage(dir.clone());

            app.manage(AppState {
                rag: Arc::new(Mutex::new(rag)),
                secrets: secrets::Backend::detect(dir.join("secrets.vault")),
                unreadable_keys: StdMutex::new(HashSet::new()),
                http: Client::builder()
                    .connect_timeout(Duration::from_secs(15))
                    .timeout(Duration::from_secs(120))
                    .build()
                    .unwrap_or_else(|_| Client::new()),
                tasks: Arc::new(StdMutex::new(HashMap::new())),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            stream_chat_command,
            stop_chat_command,
            load_sessions,
            save_sessions,
            load_settings,
            save_settings,
            secrets_status,
            vault_create,
            vault_unlock,
            vault_lock,
            vault_change_passphrase,
            vault_reset,
            index_document_command,
            list_documents,
            delete_document,
            test_connection_command,
            test_embedding_command
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn merges_all_system_messages_to_front() {
        let out = merge_system_messages(vec![
            msg("system", "persona"),
            msg("user", "hi"),
            msg("system", "rag"),
        ]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, "system");
        assert_eq!(out[0].content, "persona\n\nrag");
        assert_eq!(out[1].role, "user");
    }

    #[test]
    fn anthropic_has_no_default_embedding_endpoint() {
        let cfg = resolve_embedding("anthropic", "https://api.anthropic.com/v1", "k", &EmbeddingConfig::default());
        assert!(cfg.base_url.is_empty());
        let cfg = resolve_embedding("openai", "https://x/v1", "k", &EmbeddingConfig::default());
        assert_eq!((cfg.base_url.as_str(), cfg.api_key.as_str()), ("https://x/v1", "k"));
    }
}
