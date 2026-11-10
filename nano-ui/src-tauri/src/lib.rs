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

mod db;
use db::{DbState, Conversation, ConversationListItem};

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

// --- Commandes de gestion des conversations ---

#[tauri::command]
fn conversation_new(state: State<'_, DbState>, title: Option<String>) -> Result<Conversation, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::conversation_new(&conn, title).map_err(|e| e.to_string())
}

#[tauri::command]
fn conversation_list(state: State<'_, DbState>) -> Result<Vec<ConversationListItem>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::conversation_list(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
fn conversation_get(
    state: State<'_, DbState>,
    id: String,
    password: Option<String>,
) -> Result<Conversation, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let conv = db::conversation_get(&conn, &id).map_err(|e| e.to_string())?;
    match conv {
        Some(c) => {
            // Si la conversation est protégée, vérifier le mot de passe
            if let Some(ref hash) = c.password_hash {
                let provided = password.ok_or("Mot de passe requis")?;
                if *hash != provided {
                    return Err("Mot de passe incorrect".to_string());
                }
            }
            Ok(c)
        }
        None => Err("Conversation introuvable".to_string()),
    }
}

#[tauri::command]
fn conversation_save(
    state: State<'_, DbState>,
    conv: Conversation,
) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::conversation_save(&conn, &conv).map_err(|e| e.to_string())
}

#[tauri::command]
fn conversation_delete(state: State<'_, DbState>, id: String) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::conversation_delete(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
fn conversation_rename(
    state: State<'_, DbState>,
    id: String,
    title: String,
) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::conversation_rename(&conn, &id, &title).map_err(|e| e.to_string())
}

#[tauri::command]
fn conversation_unlock(
    state: State<'_, DbState>,
    id: String,
    password: String,
) -> Result<bool, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::conversation_unlock(&conn, &id, &password).map_err(|e| e.to_string())
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

fn window_state_path(db_dir: &Path) -> std::path::PathBuf {
    db_dir.join("window-state.json")
}

fn load_window_state(db_dir: &Path) -> Option<WindowState> {
    let path = window_state_path(db_dir);
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_window_state(db_dir: &Path, state: &WindowState) -> std::io::Result<()> {
    let path = window_state_path(db_dir);
    let content = serde_json::to_string_pretty(state)?;
    std::fs::write(path, content)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    {
        // Sous WSLg, GTK utilise par défaut le backend Wayland qui dessine la
        // barre de titre côté application (client-side decorations). Cela ne
        // laisse souvent que la croix fermer. Forcer X11 fait dessiner le cadre
        // par WSLg, qui affiche réduire/agrandir/fermer.
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
            let db_dir = data_dir.join("nano");
            std::fs::create_dir_all(&db_dir).map_err(|e| e.to_string())?;
            let db_path = db_dir.join("nano.db");
            let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
            db::init_db(&conn).map_err(|e| e.to_string())?;
            app.manage(DbState {
                conn: Mutex::new(conn),
            });

            // Restaure les dimensions de la dernière fermeture (si connues).
            if let Some(window) = app.get_webview_window("main") {
                if let Some(state) = load_window_state(&db_dir) {
                    let _ = window.set_size(Size::Logical(LogicalSize::new(state.width, state.height)));
                    if let (Some(x), Some(y)) = (state.x, state.y) {
                        let _ = window.set_position(Position::Logical(LogicalPosition::new(x, y)));
                    }
                    if state.maximized.unwrap_or(false) {
                        let _ = window.maximize();
                    }
                }
            }

            // Sauvegarde l'état de la fenêtre à chaque fermeture.
            let app_handle = app.app_handle().clone();
            let app_handle_for_close = app_handle.clone();
            let state_dir = db_dir.clone();
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
        .invoke_handler(tauri::generate_handler![
            get_ollama_status,
            chat,
            cancel_chat,
            toggle_fullscreen,
            conversation_new,
            conversation_list,
            conversation_get,
            conversation_save,
            conversation_delete,
            conversation_rename,
            conversation_unlock,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
