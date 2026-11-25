use std::env;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use nano_core::{ChatMessage, context::build_context, settings::ChatSettings, tools::{ToolCall, ToolResult, ToolDefinition}};
use serde::{Deserialize, Serialize};
use tauri::Listener;
use tauri::Manager;
use tauri::Position;
use tauri::Size;
use tauri::State;
use tauri::ipc::Channel;
use tauri::LogicalPosition;
use tauri::LogicalSize;
use tauri_plugin_dialog::DialogExt;

mod db;
mod permissions;
use db::{DbState, Conversation, ConversationListItem, ProjectConfig};
use permissions::{AccessKind, ProjectPolicy};

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
    system_prompt: String,
    context_limit: usize,
    on_token: Channel<String>,
) -> Result<(), String> {
    // Remise du drapeau d'annulation à false au début de chaque commande chat.
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut guard = state.cancel.lock().map_err(|e| e.to_string())?;
        *guard = Some(cancel.clone());
    }

    // Applique la stratégie de contexte : prompt système + troncature des messages anciens
    let settings = ChatSettings {
        system_prompt,
        context_limit,
    };
    let prepared_messages = build_context(&messages, &settings, &model)
        .await
        .map_err(|e| e.to_string())?;

    let result = nano_core::stream_chat(
        &model,
        &prepared_messages,
        |token| {
            let _ = on_token.send(token.to_string());
        },
        &cancel,
    )
    .await;

    // Détermine si l'annulation a été demandée avant de libérer l'état.
    let cancelled = {
        let guard = state.cancel.lock().map_err(|e| e.to_string())?;
        guard.as_ref().is_some_and(|c| c.load(Ordering::Relaxed))
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

/// Définitions des outils disponibles pour le modèle.
fn tool_read_file() -> ToolDefinition {
    ToolDefinition {
        name: "read_file".to_string(),
        description: "Lit le contenu d'un fichier du projet et le retourne.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Chemin du fichier à lire" }
            },
            "required": ["path"]
        }),
    }
}

fn tool_list_dir() -> ToolDefinition {
    ToolDefinition {
        name: "list_dir".to_string(),
        description: "Liste le contenu d'un dossier du projet (fichiers et sous-dossiers).".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Chemin du dossier à lister" }
            },
            "required": ["path"]
        }),
    }
}

fn tool_write_file() -> ToolDefinition {
    ToolDefinition {
        name: "write_file".to_string(),
        description: "Écrit un fichier du projet avec le contenu fourni (création ou remplacement)."
            .to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Chemin du fichier à écrire" },
                "content": { "type": "string", "description": "Contenu complet du fichier" }
            },
            "required": ["path", "content"]
        }),
    }
}

/// Extrait le chemin des arguments d'un outil.
/// Accepte une chaîne simple (anciens modèles) ou les clés `path`, `file_name`, `dir`, `directory`.
fn extract_path(arguments: &serde_json::Value) -> Option<String> {
    arguments.as_str().map(str::to_string).or_else(|| {
        ["path", "file_name", "dir", "directory"]
            .iter()
            .find_map(|key| {
                arguments
                    .get(*key)
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
    })
}

/// Extrait le contenu d'un fichier à écrire.
fn extract_content(arguments: &serde_json::Value) -> Option<String> {
    ["content", "text", "body"]
        .iter()
        .find_map(|key| {
            arguments
                .get(*key)
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
}

/// Exécute un outil demandé par le modèle, dans les limites du dossier de projet lié.
async fn execute_tool(tool_call: ToolCall, project: &ProjectConfig) -> ToolResult {
    let path = extract_path(&tool_call.arguments);

    let Some(path) = path.filter(|value| !value.trim().is_empty()) else {
        return ToolResult {
            name: tool_call.name,
            content: "Argument « path » manquant ou vide.".to_string(),
            is_error: true,
        };
    };

    let policy = match ProjectPolicy::new(project) {
        Ok(policy) => policy,
        Err(error) => {
            return ToolResult {
                name: tool_call.name,
                content: error,
                is_error: true,
            };
        }
    };

    let resolved = match tool_call.name.as_str() {
        "read_file" | "list_dir" => policy.resolve(&path, AccessKind::Read),
        "write_file" => policy.resolve(&path, AccessKind::Write),
        _ => {
            return ToolResult {
                name: tool_call.name,
                content: "Outil inconnu".to_string(),
                is_error: true,
            };
        }
    };

    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(error) => {
            return ToolResult {
                name: tool_call.name,
                content: error,
                is_error: true,
            };
        }
    };

    match tool_call.name.as_str() {
        "read_file" => match std::fs::read_to_string(&resolved) {
            Ok(content) => ToolResult {
                name: tool_call.name,
                content,
                is_error: false,
            },
            Err(e) => ToolResult {
                name: tool_call.name,
                content: e.to_string(),
                is_error: true,
            },
        },
        "list_dir" => match std::fs::read_dir(&resolved) {
            Ok(entries) => {
                let mut items: Vec<String> = entries
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| !policy.is_excluded(&entry.path()))
                    .map(|entry| {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if entry.path().is_dir() {
                            format!("{name}/")
                        } else {
                            name
                        }
                    })
                    .collect();
                items.sort();
                ToolResult {
                    name: tool_call.name,
                    content: items.join("\n"),
                    is_error: false,
                }
            }
            Err(e) => ToolResult {
                name: tool_call.name,
                content: e.to_string(),
                is_error: true,
            },
        },
        "write_file" => {
            let Some(content) = extract_content(&tool_call.arguments) else {
                return ToolResult {
                    name: tool_call.name,
                    content: "Argument « content » manquant.".to_string(),
                    is_error: true,
                };
            };
            match std::fs::write(&resolved, content.as_bytes()) {
                Ok(()) => ToolResult {
                    name: tool_call.name,
                    content: format!(
                        "Fichier écrit : {} ({} octets).",
                        resolved.display(),
                        content.len()
                    ),
                    is_error: false,
                },
                Err(e) => ToolResult {
                    name: tool_call.name,
                    content: e.to_string(),
                    is_error: true,
                },
            }
        }
        _ => unreachable!("outil déjà filtré"),
    }
}

// --- Commande chat agent (boucle avec outils) ---

#[tauri::command]
async fn chat_agent(
    state: State<'_, AppState>,
    db_state: State<'_, DbState>,
    model: String,
    messages: Vec<ChatMessage>,
    system_prompt: String,
    context_limit: usize,
    on_token: Channel<String>,
) -> Result<(), String> {
    // Remise du drapeau d'annulation à false au début de chaque commande chat.
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut guard = state.cancel.lock().map_err(|e| e.to_string())?;
        *guard = Some(cancel.clone());
    }

    // Photographie des permissions au début du tour : une modification du projet
    // pendant la génération ne change pas les droits en cours de route.
    let project = {
        let conn = db_state.conn.lock().map_err(|e| e.to_string())?;
        db::project_get(&conn).map_err(|e| e.to_string())?
    };

    // En mode agent, on ajoute toujours les instructions d'outils au prompt système :
    // si l'utilisateur a défini son propre prompt, un petit modèle peut sinon ignorer les outils.
    let project_hint = match project.root_path.as_deref() {
        Some(root) => format!(
            "Dossier de projet lié : {root}. Les chemins hors de ce dossier sont interdits, ainsi que les exclusions."
        ),
        None => "Aucun dossier de projet n'est lié. Les outils refuseront tout accès ; demande à l'utilisateur de lier un dossier dans le panneau Projet.".to_string(),
    };
    let tool_instructions = format!(
        "Tu disposes de trois outils : read_file (lire un fichier), write_file (écrire un fichier) et list_dir (lister un dossier).\n\
Règles :\n\
- {project_hint}\n\
- Si l'utilisateur demande de lire un fichier, appelle read_file.\n\
- Reprends le chemin exactement tel que l'utilisateur l'a écrit, sans corriger ni réorthographier.\n\
- Si le chemin est hors du dossier de projet, réponds directement que tu n'y as pas accès, sans appeler d'outil.\n\
- Pour un chemin dans le projet, appelle l'outil même s'il pourrait être exclu : c'est l'outil qui décide.\n\
- Si l'utilisateur demande d'écrire ou de modifier un fichier, appelle write_file avec le contenu complet.\n\
- Si l'utilisateur demande de lister un dossier, appelle list_dir.\n\
- N'invente jamais le contenu d'un fichier : lis-le d'abord avec l'outil.\n\
- Pour appeler un outil, réponds uniquement avec un JSON de la forme {{\"name\":\"read_file\",\"arguments\":{{\"path\":\"/chemin/absolu\"}}}}.\n\
- Après le résultat d'un outil, réponds uniquement avec la synthèse demandée, de façon concise, sans répéter.\n\
- Réponds toujours en français.\n\
- Base-toi uniquement sur le contenu réellement renvoyé par l'outil ; n'invente aucun détail.\n\n\
Exemple :\n\
utilisateur : « Lis /tmp/a.txt »\n\
toi : {{\"name\":\"read_file\",\"arguments\":{{\"path\":\"/tmp/a.txt\"}}}}"
    );
    let system_prompt = if system_prompt.is_empty() {
        tool_instructions
    } else {
        format!("{system_prompt}\n\n{tool_instructions}")
    };

    let settings = ChatSettings {
        system_prompt,
        context_limit,
    };
    let prepared_messages = build_context(&messages, &settings, &model)
        .await
        .map_err(|e| e.to_string())?;

    let tools = vec![tool_read_file(), tool_write_file(), tool_list_dir()];

    let result = nano_core::run_agent_turn(
        &model,
        &prepared_messages,
        &tools,
        |tool_call| {
            let project = project.clone();
            Box::pin(async move { execute_tool(tool_call, &project).await })
        },
        |token| {
            let _ = on_token.send(token.to_string());
        },
        &cancel,
    )
    .await;

    // Détermine si l'annulation a été demandée avant de libérer l'état.
    let cancelled = {
        let guard = state.cancel.lock().map_err(|e| e.to_string())?;
        guard.as_ref().is_some_and(|c| c.load(Ordering::Relaxed))
    };

    // Libère l'état même en cas d'erreur (annulation ou échec réseau).
    {
        let mut guard = state.cancel.lock().map_err(|e| e.to_string())?;
        *guard = None;
    }

    if cancelled {
        return Ok(());
    }
    result.map(|_| ()).map_err(|e| e.to_string())
}

// --- Commandes de gestion des paramètres de conversation (prompt système + contexte) ---

#[tauri::command]
fn get_settings(state: State<'_, DbState>, id: String) -> Result<ChatSettings, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::conversation_settings_get(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_settings(
    state: State<'_, DbState>,
    id: String,
    settings: ChatSettings,
) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::conversation_settings_save(&conn, &id, &settings).map_err(|e| e.to_string())
}

// --- Commandes du projet lié ---

#[tauri::command]
fn project_get(state: State<'_, DbState>) -> Result<ProjectConfig, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::project_get(&conn).map_err(|e| e.to_string())
}

/// Ouvre le sélecteur de dossier natif et lie le dossier choisi.
/// Si l'utilisateur annule, la configuration actuelle est renvoyée inchangée.
#[tauri::command]
async fn project_pick_folder(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
) -> Result<ProjectConfig, String> {
    let folder = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;

    if let Some(folder) = folder {
        let path = folder.into_path().map_err(|e| e.to_string())?;
        let path = std::fs::canonicalize(&path)
            .map_err(|e| format!("Dossier inaccessible : {e}"))?;
        if !path.is_dir() {
            return Err("Le chemin choisi n'est pas un dossier.".to_string());
        }
        let conn = state.conn.lock().map_err(|e| e.to_string())?;
        db::project_set_root(&conn, &path.to_string_lossy()).map_err(|e| e.to_string())?;
    }

    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::project_get(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
fn project_clear(state: State<'_, DbState>) -> Result<ProjectConfig, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::project_clear(&conn).map_err(|e| e.to_string())?;
    db::project_get(&conn).map_err(|e| e.to_string())
}

/// Ajoute une exclusion (chemin relatif au projet ou absolu, fichier ou dossier).
#[tauri::command]
fn project_add_exclusion(
    state: State<'_, DbState>,
    path: String,
) -> Result<ProjectConfig, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let project = db::project_get(&conn).map_err(|e| e.to_string())?;
    let root_str = project
        .root_path
        .as_deref()
        .ok_or_else(|| "Aucun dossier de projet lié.".to_string())?;
    let root = std::fs::canonicalize(root_str)
        .map_err(|e| format!("Dossier de projet inaccessible : {e}"))?;

    let path = path.trim();
    if path.is_empty() {
        return Err("Chemin d'exclusion vide.".to_string());
    }
    let candidate = std::path::Path::new(path);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let canonical = std::fs::canonicalize(&joined)
        .map_err(|_| format!("Chemin introuvable dans le projet : {path}"))?;

    if canonical == root {
        return Err("Le dossier de projet lui-même ne peut pas être exclu.".to_string());
    }
    let relative = canonical
        .strip_prefix(&root)
        .map_err(|_| format!("Ce chemin est hors du dossier de projet : {path}"))?;
    let relative = relative.to_string_lossy().replace('\\', "/");

    db::project_add_exclusion(&conn, &relative).map_err(|e| e.to_string())?;
    db::project_get(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
fn project_remove_exclusion(
    state: State<'_, DbState>,
    path: String,
) -> Result<ProjectConfig, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::project_remove_exclusion(&conn, &path).map_err(|e| e.to_string())?;
    db::project_get(&conn).map_err(|e| e.to_string())
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
        .plugin(tauri_plugin_dialog::init())
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
            chat_agent,
            get_settings,
            save_settings,
            project_get,
            project_pick_folder,
            project_clear,
            project_add_exclusion,
            project_remove_exclusion,
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
