mod providers;
mod types;

use futures::StreamExt;
use providers::get_provider;
use tauri::{AppHandle, Emitter};
use types::ChatPayload;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
async fn stream_chat_command(app: AppHandle, payload: ChatPayload) -> Result<(), String> {
    tokio::spawn(async move {
        let provider = match get_provider(&payload.provider) {
            Ok(p) => p,
            Err(e) => {
                let _ = app.emit("chat-chunk", format!("[Lỗi]: {}", e));
                let _ = app.emit("chat-done", ());
                return;
            }
        };

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
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(text) => {
                            let _ = app.emit("chat-chunk", text);
                        }
                        Err(err) => {
                            let _ = app.emit("chat-chunk", format!("\n[Stream Error]: {}", err));
                        }
                    }
                }
            }
            Err(err) => {
                let _ = app.emit("chat-chunk", format!("[Error]: {}", err));
            }
        }

        let _ = app.emit("chat-done", ());
    });

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Tắt compositing mode/dmabuf của WebKit trên Linux nếu bị lỗi GBM
    #[cfg(target_os = "linux")]
    {
        unsafe
        {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![stream_chat_command])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
