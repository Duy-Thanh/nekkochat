mod providers;
mod rag;
mod types;

use futures::StreamExt;
use providers::get_provider;
use rag::RagEngine;
use reqwest::Client;
use std::collections::HashMap;
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
    EmbeddingConfig, Settings,
};

// Thời gian tối đa chờ chunk kế tiếp (model reasoning có thể "nghĩ" khá lâu)
const CHUNK_TIMEOUT: Duration = Duration::from_secs(90);
const RAG_TOP_K: usize = 3;

pub struct AppState {
    pub rag: Arc<Mutex<RagEngine>>,
    pub http: Client,
    /// session_id -> task đang stream, để nút Stop có thể hủy
    pub tasks: Arc<StdMutex<HashMap<String, AbortHandle>>>,
}

/// Ghi ra file tạm rồi rename để crash giữa chừng không làm hỏng dữ liệu.
pub(crate) fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content).map_err(|e| format!("Failed to write {:?}: {}", tmp, e))?;
    fs::rename(&tmp, path).map_err(|e| format!("Failed to replace {:?}: {}", path, e))
}

fn app_file(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {}", e))?;

    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create directory {:?}: {}", dir, e))?;
    }
    Ok(dir.join(name))
}

/// Embedding mặc định dùng chung endpoint với chat (trừ Anthropic - không có embeddings API).
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

    // Gọi API embedding KHÔNG giữ lock RAG để chat vẫn dùng được trong lúc index.
    let total = texts.len();
    let mut chunks = Vec::with_capacity(total);
    for (i, text) in texts.into_iter().enumerate() {
        let embedding = rag::embed(&state.http, &cfg, &text, false).await?;
        chunks.push(DocumentChunk {
            file_name: file_name.clone(),
            content: text,
            embedding,
        });
        let _ = app.emit("rag-progress", format!("{}/{}", i + 1, total));
    }

    let count = state.rag.lock().await.replace_document(&file_name, chunks)?;
    Ok(format!("Indexed {} chunks from {}", count, file_name))
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
async fn load_settings(app: AppHandle) -> Result<Option<Settings>, String> {
    let path = app_file(&app, "settings.json")?;
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).map_err(|e| format!("Failed to read settings: {}", e))?;
    Ok(serde_json::from_str(&content).ok())
}

#[tauri::command]
async fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let path = app_file(&app, "settings.json")?;
    let content = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    write_atomic(&path, &content)
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
            // Không được trả về rỗng rồi để lần save sau ghi đè mất lịch sử
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

/// Gộp mọi system message thành một (Anthropic chỉ nhận một `system`,
/// một số server local cũng yêu cầu system nằm đầu và duy nhất).
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

    // Giữ lock tới khi đã đăng ký handle, tránh race với stop_chat_command
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
            // Chạy cả khi task bị abort (nút Stop)
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

        // 1. RAG: embed query ngoài lock, chỉ lock lúc chấm điểm
        let user_query = payload
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone())
            .unwrap_or_default();

        if !user_query.is_empty() && !rag_engine.lock().await.is_empty() {
            let cfg = resolve_embedding(
                &payload.provider,
                &payload.base_url,
                &payload.api_key,
                &payload.embedding,
            );
            match rag::embed(&http, &cfg, &user_query, true).await {
                Ok(q) => {
                    let ctx = rag_engine.lock().await.search(&q, RAG_TOP_K);
                    if !ctx.is_empty() {
                        payload.messages.push(ChatMessage {
                            role: "system".to_string(),
                            content: format!(
                                "Use the document excerpts below to answer if they are relevant:\n{}",
                                ctx
                            ),
                        });
                    }
                }
                Err(e) => eprintln!("[RAG] skipping context due to embedding error: {}", e),
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
        // SAFETY: gọi trước khi tạo thread nào khác
        unsafe {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }

    let rag_engine = Arc::new(Mutex::new(RagEngine::new()));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup({
            let rag_engine = rag_engine.clone();
            move |app| {
                if let Ok(dir) = app.path().app_data_dir() {
                    let _ = fs::create_dir_all(&dir);
                    rag_engine.blocking_lock().init_storage(dir);
                }
                Ok(())
            }
        })
        .manage(AppState {
            rag: rag_engine,
            http: Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(120))
                .build()
                .unwrap_or_else(|_| Client::new()),
            tasks: Arc::new(StdMutex::new(HashMap::new())),
        })
        .invoke_handler(tauri::generate_handler![
            stream_chat_command,
            stop_chat_command,
            load_sessions,
            save_sessions,
            load_settings,
            save_settings,
            index_document_command,
            list_documents,
            delete_document
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
