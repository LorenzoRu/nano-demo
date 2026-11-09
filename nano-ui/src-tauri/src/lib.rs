use nano_core::ChatMessage;
use tauri::ipc::Channel;
#[tauri::command]
async fn get_ollama_status() -> nano_core::status::OllamaStatus {
    nano_core::status::ollama_status().await
}
#[tauri::command]
async fn chat(
    model: String,
    messages: Vec<ChatMessage>,
    on_token: Channel<String>,
) -> Result<(), String> {
    nano_core::stream_chat(&model, &messages, |token| {
        let _ = on_token.send(token.to_string());
    })
    .await
    .map_err(|e| e.to_string())
}


#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
 tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![get_ollama_status, chat])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
