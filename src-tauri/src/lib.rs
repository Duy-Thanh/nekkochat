mod providers;
mod rag;
mod types;

use futures::StreamExt;
use providers::get_provider;
use rag::RagEngine;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;
use tokio::time::timeout;

use types::{ChatMessage, ChatPayload, ChatSession};

// Managed App State chứa RAG Engine
pub struct AppState {
    pub rag: Arc<Mutex<RagEngine>>,
}

fn get_sessions_file_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {}", e))?;

    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create directory {:?}: {}", dir, e))?;
    }

    Ok(dir.join("sessions.json"))
}

#[tauri::command]
async fn index_document_command(
    state: State<'_, AppState>,
    base_url: String,
    api_key: String,
    file_path: String,
) -> Result<String, String> {
    let mut rag = state.rag.lock().await;
    let count = rag.index_file(&base_url, &api_key, &file_path).await?;
    Ok(format!("Đã index thành công {} đoạn tài liệu!", count))
}

#[tauri::command]
async fn load_sessions(app: AppHandle) -> Result<Vec<ChatSession>, String> {
    let path = get_sessions_file_path(&app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content =
        fs::read_to_string(path).map_err(|e| format!("Failed to read sessions: {}", e))?;
    let sessions: Vec<ChatSession> = serde_json::from_str(&content).unwrap_or_default();
    Ok(sessions)
}

#[tauri::command]
async fn save_sessions(app: AppHandle, sessions: Vec<ChatSession>) -> Result<(), String> {
    let path = get_sessions_file_path(&app)?;
    let content = serde_json::to_string_pretty(&sessions)
        .map_err(|e| format!("Failed to serialize sessions: {}", e))?;
    fs::write(path, content).map_err(|e| format!("Failed to write sessions: {}", e))?;
    Ok(())
}

#[tauri::command]
async fn stream_chat_command(
    app: AppHandle,
    state: State<'_, AppState>,
    mut payload: ChatPayload,
) -> Result<(), String> {
    // Clone Arc<Mutex<RagEngine>> ra ngoài để đạt 'static lifetime cho tokio::spawn
    let rag_engine = state.rag.clone();

    tokio::spawn(async move {
        struct DoneGuard(AppHandle);
        impl Drop for DoneGuard {
            fn drop(&mut self) {
                let _ = self.0.emit("chat-done", ());
            }
        }
        let _guard = DoneGuard(app.clone());

        // 1. Lấy user query mới nhất
        let user_query = payload
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone())
            .unwrap_or_default();

        // 2. Query RAG context và inject vào system message trước khi bắn sang LLM
        if !user_query.is_empty() {
            let rag_guard = rag_engine.lock().await;
            if let Ok(rag_context) = rag_guard
                .search_relevant_context(&payload.base_url, &payload.api_key, &user_query, 3)
                .await
            {
                if !rag_context.is_empty() {
                    println!("[RAG INJECTED]: \n{}", rag_context);
                    payload.messages.insert(
                        0,
                        ChatMessage {
                            role: "system".to_string(),
                            content: format!(
                                "Bạn là trợ lý AI. Hãy dựa vào thông tin tài liệu bên dưới để trả lời câu hỏi nếu có liên quan:\n{}",
                                rag_context
                            ),
                        },
                    );
                }
            }
        }

        let provider = match get_provider(&payload.provider) {
            Ok(p) => p,
            Err(e) => {
                let _ = app.emit("chat-chunk", format!("[ERROR]: {}", e));
                return;
            }
        };

        // 3. Streaming Chat Response
        match provider
            .stream_chat(
                &payload.base_url,
                &payload.api_key,
                &payload.model,
                payload.messages,
            )
            .await
        {
            Ok(mut stream) => {
                let chunk_timeout = Duration::from_secs(15);
                loop {
                    match timeout(chunk_timeout, stream.next()).await {
                        Ok(Some(Ok(text))) => {
                            let _ = app.emit("chat-chunk", text);
                        }
                        Ok(Some(Err(err))) => {
                            let _ = app.emit("chat-chunk", format!("\n[Stream Error]: {}", err));
                            break;
                        }
                        Ok(None) => break,
                        Err(_) => {
                            let _ = app.emit(
                                "chat-chunk",
                                "\n[Timeout Error]: Server ngâm kết nối quá 15s!",
                            );
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                let _ = app.emit("chat-chunk", format!("[Request Error]: {}", err));
            }
        }
    });

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    {
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    // Khởi tạo RAG Engine trong Managed State
    let rag_engine = Arc::new(Mutex::new(RagEngine::new()));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { rag: rag_engine }) // <-- Bắt buộc phải manage AppState ở đây!
        .invoke_handler(tauri::generate_handler![
            stream_chat_command,
            load_sessions,
            save_sessions,
            index_document_command // <-- Register command index file RAG!
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}