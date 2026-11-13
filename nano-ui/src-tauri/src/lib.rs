use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use nano_core::ChatMessage;
use serde::{Deserialize, Serialize};
use tauri::Listener;
use tauri::Manager;
use tauri::Position;
use tauri::Size;
use tauri::State;
use tauri::ipc::Channel;
use tauri::LogicalPosition;
use tauri::LogicalSize;
use uuid::Uuid;
use chrono::Utc;

/// État partagé entre les invocations : permet d'annuler un stream en cours
/// depuis une autre commande (`cancel_chat`).
struct AppState {
    cancel: Mutex<Option<Arc<AtomicBool>>>,
}

/// Une conversation complète, avec ses messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub password_hash: Option<String>,
    pub size_bytes: i64,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub context_limit: i64,
    pub messages: Vec<ChatMessage>,
}

/// Une conversation allégée (pour la liste, sans les messages).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationListItem {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub size_bytes: i64,
    pub has_password: bool,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub context_limit: i64,
}

/// État partagé des conversations en mémoire.
pub struct DbState {
    pub conversations: Mutex<HashMap<String, Conversation>>,
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

    let cancelled = {
        let guard = state.cancel.lock().map_err(|e| e.to_string())?;
        guard.as_ref().map_or(false, |c| c.load(Ordering::Relaxed))
    };

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

// --- Commandes de gestion des conversations (implémentation en mémoire) ---

#[tauri::command]
fn conversation_new(
    state: State<'_, DbState>,
    title: Option<String>,
) -> Result<Conversation, String> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let title = title.unwrap_or_else(|| "Nouvelle conversation".to_string());
    let conv = Conversation {
        id: id.clone(),
        title: title.clone(),
        created_at: now.clone(),
        updated_at: now.clone(),
        password_hash: None,
        size_bytes: 0,
        system_prompt: String::new(),
        context_limit: 20,
        messages: vec![],
    };
    state
        .conversations
        .lock()
        .map_err(|e| e.to_string())?
        .insert(id.clone(), conv.clone());
    Ok(conv)
}

#[tauri::command]
fn conversation_list(state: State<'_, DbState>) -> Result<Vec<ConversationListItem>, String> {
    let conversations = state.conversations.lock().map_err(|e| e.to_string())?;
    let mut out: Vec<ConversationListItem> = conversations
        .values()
        .map(|c| ConversationListItem {
            id: c.id.clone(),
            title: c.title.clone(),
            created_at: c.created_at.clone(),
            updated_at: c.updated_at.clone(),
            size_bytes: c.size_bytes,
            has_password: c.password_hash.is_some(),
            system_prompt: c.system_prompt.clone(),
            context_limit: c.context_limit,
        })
        .collect();
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(out)
}

#[tauri::command]
fn conversation_get(
    state: State<'_, DbState>,
    id: String,
    password: Option<String>,
) -> Result<Conversation, String> {
    let conversations = state.conversations.lock().map_err(|e| e.to_string())?;
    let conv = conversations.get(&id).ok_or("Conversation introuvable")?;
    if let Some(ref hash) = conv.password_hash {
        let provided = password.ok_or("Mot de passe requis")?;
        if *hash != provided {
            return Err("Mot de passe incorrect".to_string());
        }
    }
    Ok(conv.clone())
}

#[tauri::command]
fn conversation_save(
    state: State<'_, DbState>,
    conv: Conversation,
) -> Result<(), String> {
    let mut conversations = state.conversations.lock().map_err(|e| e.to_string())?;
    conversations.insert(conv.id.clone(), conv);
    Ok(())
}

#[tauri::command]
fn conversation_delete(state: State<'_, DbState>, id: String) -> Result<(), String> {
    state
        .conversations
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&id)
        .ok_or("Conversation introuvable")?;
    Ok(())
}

#[tauri::command]
fn conversation_rename(
    state: State<'_, DbState>,
    id: String,
    title: String,
) -> Result<(), String> {
    let mut conversations = state.conversations.lock().map_err(|e| e.to_string())?;
    let conv = conversations
        .get_mut(&id)
        .ok_or("Conversation introuvable")?;
    conv.title = title;
    conv.updated_at = Utc::now().to_rfc3339();
    Ok(())
}

#[tauri::command]
fn conversation_unlock(
    state: State<'_, DbState>,
    id: String,
    password: String,
) -> Result<bool, String> {
    let conversations = state.conversations.lock().map_err(|e| e.to_string())?;
    match conversations.get(&id) {
        Some(conv) => Ok(conv.password_hash.as_deref() == Some(password.as_str())),
        None => Ok(false),
    }
}

/// Bascule la fenêtre principale entre mode fenêtré et plein écran.
#[tauri::command]
fn toggle_fullscreen(window: tauri::Window) -> Result<(), String> {
    let is_fullscreen = window.is_fullscreen().map_err(|e| e.to_string())?;
    window.set_fullscreen(!is_fullscreen).map_err(|e| e.to_string())?;
    Ok(())
}

/// État de la fenêtre (taille/position) persistant entre deux lancements.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WindowState {
    width: f64,
    height: f64,
    x: Option<f64>,
    y: Option<f64>,
    maximized: Option<bool>,
    fullscreen: Option<bool>,
}

fn window_state_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("window-state.json")
}

fn load_window_state(data_dir: &Path) -> Option<WindowState> {
    let path = window_state_path(data_dir);
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_window_state(data_dir: &Path, state: &WindowState) -> std::io::Result<()> {
    let path = window_state_path(data_dir);
    let content = serde_json::to_string_pretty(state)?;
    std::fs::write(path, content)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WSL_DISTRO_NAME").is_some()
            && std::env::var_os("GDK_BACKEND").is_none()
        {
            unsafe { std::env::set_var("GDK_BACKEND", "x11") };
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = dirs::data_dir().unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;

            if let Some(window) = app.get_webview_window("main") {
                if let Some(state) = load_window_state(&data_dir) {
                    let _ = window.set_size(Size::Logical(LogicalSize::new(state.width, state.height)));
                    if let (Some(x), Some(y)) = (state.x, state.y) {
                        let _ = window.set_position(Position::Logical(LogicalPosition::new(x, y)));
                    }
                    if state.maximized.unwrap_or(false) {
                        let _ = window.maximize();
                    }
                }
            }

            let app_handle = app.app_handle().clone();
            let app_handle_for_close = app_handle.clone();
            let state_dir = data_dir.clone();
            app_handle.listen_any("tauri://window-close", move |_event| {
                if let Some(window) = app_handle_for_close.get_webview_window("main") {
                    let size = window.inner_size().unwrap_or_default();
                    let pos = window.outer_position().unwrap_or_default();
                    let state = WindowState {
                        width: size.width as f64,
                        height: size.height as f64,
                        x: Some(pos.x as f64),
                        y: Some(pos.y as f64),
                        maximized: Some(window.is_maximized().unwrap_or(false)),
                        fullscreen: Some(window.is_fullscreen().unwrap_or(false)),
                    };
                    let _ = save_window_state(&state_dir, &state);
                }
            });

            Ok(())
        })
        .manage(AppState {
            cancel: Mutex::new(None),
        })
        .manage(DbState {
            conversations: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            get_ollama_status,
            chat,
            cancel_chat,
            conversation_new,
            conversation_list,
            conversation_get,
            conversation_save,
            conversation_delete,
            conversation_rename,
            conversation_unlock,
            toggle_fullscreen,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
