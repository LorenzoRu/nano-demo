use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use nano_core::ChatMessage;
use tauri::State;
use tauri::ipc::Channel;

/// État partagé entre les invocations : permet d'annuler un stream en cours
/// depuis une autre commande (`cancel_chat`).
struct AppState {
    cancel: Mutex<Option<Arc<AtomicBool>>>,
}

#[tauri::command]
async fn get_ollama_status() -> nano_core::status::OllamaStatus {
    nano_core::status::ollama_status().await
}

#[tauri::command]
async fn chat(
    state: State<'_, AppState>,
    model: String,
    messages: Vec<ChatMessage>,
    on_token: Channel<String>,
) -> Result<(), String> {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut guard = state.cancel.lock().map_err(|e| e.to_string())?;
        *guard = Some(cancel.clone());
    }

    let result = nano_core::stream_chat(
        &model,
        &messages,
        |token| {
            let _ = on_token.send(token.to_string());
        },
        &cancel,
    )
    .await;

    // Détermine si l'annulation a été demandée avant de libérer l'état.
    let cancelled = {
        let guard = state.cancel.lock().map_err(|e| e.to_string())?;
        guard.as_ref().map_or(false, |c| c.load(Ordering::Relaxed))
    };

    // Libère l'état même en cas d'erreur (annulation ou échec réseau).
    {
        let mut guard = state.cancel.lock().map_err(|e| e.to_string())?;
        *guard = None;
    }

    if cancelled {
        return Ok(());
    }
    result.map_err(|e| e.to_string())
}

#[tauri::command]
fn cancel_chat(state: State<'_, AppState>) -> Result<(), String> {
    let guard = state.cancel.lock().map_err(|e| e.to_string())?;
    if let Some(cancel) = guard.as_ref() {
        cancel.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            cancel: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            get_ollama_status,
            chat,
            cancel_chat
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
