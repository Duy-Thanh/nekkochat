mod providers;
mod types;

use futures::StreamExt;
use providers::get_provider;
use tauri::{AppHandle, Emitter};
use types::ChatPayload;
use std::time::Duration;
use tokio::time::timeout;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
async fn stream_chat_command(app: AppHandle, payload: ChatPayload) -> Result<(), String> {
    tokio::spawn(async move {
        // Guard đảm bảo LUÔN LUÔN emit "chat-done" duy nhất 1 lần khi thread kết thúc
        struct DoneGuard(AppHandle);
        impl Drop for DoneGuard {
            fn drop(&mut self) {
                let _ = self.0.emit("chat-done", ());
            }
        }
        let _guard = DoneGuard(app.clone());

        let provider = match get_provider(&payload.provider) {
            Ok(p) => p,
            Err(e) => {
                let _ = app.emit("chat-chunk", format!("[ERROR]: {}", e));
                return; // Guard sẽ tự drop và emit chat-done
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
                // Đặt timeout 15 giây cho MỖI chunk dữ liệu trả về
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
                        Ok(None) => {
                            // Stream kết thúc tự nhiên từ phía server
                            break;
                        }
                        Err(_) => {
                            // Dính Idle Timeout: Server ngâm kết nối quá 15s mà không nhả token nào
                            let _ = app.emit(
                                "chat-chunk",
                                "\n[Timeout Error]: Server ngâm kết nối quá lâu không trả dữ liệu!",
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
