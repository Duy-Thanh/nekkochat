mod providers;
mod types;

use futures::StreamExt;
use providers::get_provider;
use tauri::{AppHandle, Emitter};
use types::ChatPayload;
use std::time::Duration;
use tokio::time::timeout;

#[tauri::command]
async fn stream_chat_command(app: AppHandle, payload: ChatPayload) -> Result<(), String> {
    println!("\n>>> [COMMAND RECV] Provider: {}, BaseURL: {}, Model: {}", payload.provider, payload.base_url, payload.model);

    tokio::spawn(async move {
        struct DoneGuard(AppHandle);
        impl Drop for DoneGuard {
            fn drop(&mut self) {
                println!("<<< [THREAD DONE] Dropping guard & emitting chat-done");
                let _ = self.0.emit("chat-done", ());
            }
        }
        let _guard = DoneGuard(app.clone());

        let provider = match get_provider(&payload.provider) {
            Ok(p) => p,
            Err(e) => {
                println!("[E] Unknown Provider: {}", e);
                let _ = app.emit("chat-chunk", format!("[ERROR]: {}", e));
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
                println!("[STREAM LOOP STARTED] Waiting for first chunk...");
                let chunk_timeout = Duration::from_secs(15);

                loop {
                    match timeout(chunk_timeout, stream.next()).await {
                        Ok(Some(Ok(text))) => {
                            let _ = app.emit("chat-chunk", text);
                        }
                        Ok(Some(Err(err))) => {
                            println!("[E] Stream loop error: {}", err);
                            let _ = app.emit("chat-chunk", format!("\n[Stream Error]: {}", err));
                            break;
                        }
                        Ok(None) => {
                            println!("[STREAM ENDED NATIVELY]");
                            break;
                        }
                        Err(_) => {
                            println!("[E] Stream Idle Timeout (15s no data)");
                            let _ = app.emit(
                                "chat-chunk",
                                "\n[Timeout Error]: Server ngâm kết nối quá 15s đéo nhả token nào!",
                            );
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                println!("[E] stream_chat Returned Err: {}", err);
                let _ = app.emit("chat-chunk", format!("[Request Error]: {}", err));
            }
        }
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
