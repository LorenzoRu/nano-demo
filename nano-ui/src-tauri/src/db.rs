use rusqlite::{Connection, OptionalExtension, params, Result as SqlResult};
use nano_core::{ChatMessage, settings::ChatSettings};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::Utc;

/// Seuil d'alerte de poids d'une conversation : 1 Go en octets.
pub const CONVERSATION_SIZE_ALERT_BYTES: i64 = 1_073_741_824;

/// Une conversation complète, avec ses messages.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub password_hash: Option<String>,
    pub size_bytes: i64,
    pub system_prompt: String,
    pub context_limit: i64,
    pub messages: Vec<ChatMessage>,
}

/// Une conversation allégée (pour la liste, sans les messages).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConversationListItem {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub size_bytes: i64,
    pub has_password: bool,
    pub size_alert: bool,
    pub system_prompt: String,
    pub context_limit: i64,
}

/// Dossier de projet lié à nano, avec ses exclusions.
/// Il n'y a qu'un seul projet pour toute l'application (ligne `id = 1`).
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ProjectConfig {
    pub root_path: Option<String>,
    /// Exclusions relatives au dossier de projet, séparateurs `/`.
    pub exclusions: Vec<String>,
}

/// État partagé de la base de données SQLite.
pub struct DbState {
    pub conn: std::sync::Mutex<Connection>,
}

/// Initialise les tables si elles n'existent pas encore.
/// Migration : ajoute les colonnes system_prompt et context_limit aux bases existantes,
/// uniquement si elles sont absentes (SQLite ne supporte pas ALTER TABLE ... IF NOT EXISTS).
pub fn init_db(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS conversations (
            id            TEXT PRIMARY KEY,
            title         TEXT NOT NULL,
            created_at    TEXT NOT NULL,
            updated_at    TEXT NOT NULL,
            password_hash TEXT,
            size_bytes    INTEGER NOT NULL DEFAULT 0,
            system_prompt TEXT NOT NULL DEFAULT '',
            context_limit INTEGER NOT NULL DEFAULT 20
        );

        CREATE TABLE IF NOT EXISTS messages (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
            role            TEXT NOT NULL,
            content         TEXT NOT NULL,
            created_at      TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS project (
            id        INTEGER PRIMARY KEY CHECK (id = 1),
            root_path TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS project_exclusions (
            path TEXT PRIMARY KEY
        );
        ",
    )?;

    // Migration conditionnelle : ajouter les colonnes seulement si absentes.
    let has_system_prompt: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('conversations') WHERE name = 'system_prompt'",
        [],
        |row| row.get(0),
    )?;
    if !has_system_prompt {
        conn.execute(
            "ALTER TABLE conversations ADD COLUMN system_prompt TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }

    let has_context_limit: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('conversations') WHERE name = 'context_limit'",
        [],
        |row| row.get(0),
    )?;
    if !has_context_limit {
        conn.execute(
            "ALTER TABLE conversations ADD COLUMN context_limit INTEGER NOT NULL DEFAULT 20",
            [],
        )?;
    }

    Ok(())
}

/// Calcule le poids total (en octets) d'une liste de messages.
pub fn compute_size(messages: &[ChatMessage]) -> i64 {
    messages.iter().map(|m| m.content.len() as i64).sum()
}

/// Crée une conversation vide dans la base.
pub fn conversation_new(conn: &Connection, title: Option<String>) -> SqlResult<Conversation> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let title = title.unwrap_or_else(|| "Nouvelle conversation".to_string());

    conn.execute(
        "INSERT INTO conversations (id, title, created_at, updated_at, password_hash, size_bytes, system_prompt, context_limit)
         VALUES (?1, ?2, ?3, ?3, NULL, 0, '', 20)",
        params![id, title, now],
    )?;

    Ok(Conversation {
        id,
        title,
        created_at: now.clone(),
        updated_at: now,
        password_hash: None,
        size_bytes: 0,
        system_prompt: String::new(),
        context_limit: 20,
        messages: vec![],
    })
}

/// Liste toutes les conversations (sans les messages).
pub fn conversation_list(conn: &Connection) -> SqlResult<Vec<ConversationListItem>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.title, c.created_at, c.updated_at, c.password_hash, c.size_bytes, c.system_prompt, c.context_limit
         FROM conversations c
         ORDER BY c.updated_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        let password_hash: Option<String> = row.get(4)?;
        let size_bytes: i64 = row.get(5)?;
        let system_prompt: String = row.get(6)?;
        let context_limit: i64 = row.get(7)?;
        Ok(ConversationListItem {
            id: row.get(0)?,
            title: row.get(1)?,
            created_at: row.get(2)?,
            updated_at: row.get(3)?,
            size_bytes,
            has_password: password_hash.is_some(),
            size_alert: size_bytes >= CONVERSATION_SIZE_ALERT_BYTES,
            system_prompt,
            context_limit,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Charge une conversation complète (avec ses messages).
pub fn conversation_get(
    conn: &Connection,
    id: &str,
) -> SqlResult<Option<Conversation>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, created_at, updated_at, password_hash, size_bytes, system_prompt, context_limit
         FROM conversations WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map(params![id], |row| {
        Ok(Conversation {
            id: row.get(0)?,
            title: row.get(1)?,
            created_at: row.get(2)?,
            updated_at: row.get(3)?,
            password_hash: row.get(4)?,
            size_bytes: row.get(5)?,
            system_prompt: row.get(6)?,
            context_limit: row.get(7)?,
            messages: vec![],
        })
    })?;
    let conv = match rows.next() {
        Some(r) => r?,
        None => return Ok(None),
    };

    // Charge les messages associés
    let mut stmt = conn.prepare(
        "SELECT role, content FROM messages WHERE conversation_id = ?1 ORDER BY id ASC",
    )?;
    let msg_rows = stmt.query_map(params![&conv.id], |row| {
        Ok(ChatMessage {
            role: row.get(0)?,
            content: row.get(1)?,
        })
    })?;
    let mut messages = Vec::new();
    for row in msg_rows {
        messages.push(row?);
    }

    Ok(Some(Conversation { messages, ..conv }))
}

/// Sauvegarde (crée ou met à jour) une conversation complète.
pub fn conversation_save(conn: &Connection, conv: &Conversation) -> SqlResult<()> {
    let now = Utc::now().to_rfc3339();
    let size_bytes = compute_size(&conv.messages);

    conn.execute(
        "INSERT INTO conversations (id, title, created_at, updated_at, password_hash, size_bytes, system_prompt, context_limit)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
           title = excluded.title,
           updated_at = excluded.updated_at,
           password_hash = excluded.password_hash,
           size_bytes = excluded.size_bytes,
           system_prompt = excluded.system_prompt,
           context_limit = excluded.context_limit",
        params![
            conv.id,
            conv.title,
            conv.created_at,
            now,
            conv.password_hash,
            size_bytes,
            conv.system_prompt,
            conv.context_limit
        ],
    )?;

    conn.execute("DELETE FROM messages WHERE conversation_id = ?1", params![conv.id])?;
    let mut stmt = conn.prepare(
        "INSERT INTO messages (conversation_id, role, content, created_at)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for m in &conv.messages {
        stmt.execute(params![conv.id, m.role, m.content, now])?;
    }
    Ok(())
}

/// Supprime une conversation et ses messages.
pub fn conversation_delete(conn: &Connection, id: &str) -> SqlResult<()> {
    conn.execute("DELETE FROM conversations WHERE id = ?1", params![id])?;
    Ok(())
}

/// Renomme une conversation.
pub fn conversation_rename(conn: &Connection, id: &str, title: &str) -> SqlResult<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
        params![title, now, id],
    )?;
    Ok(())
}

/// Vérifie si un mot de passe correspond à celui stocké.
pub fn conversation_unlock(conn: &Connection, id: &str, password: &str) -> SqlResult<bool> {
    let mut stmt = conn.prepare("SELECT password_hash FROM conversations WHERE id = ?1")?;
    let row = stmt.query_row(params![id], |row| row.get::<_, Option<String>>(0))?;
    match row {
        Some(stored) => Ok(stored == password),
        None => Ok(false),
    }
}

/// Lit les paramètres (prompt système + limite de contexte) d'une conversation.
pub fn conversation_settings_get(conn: &Connection, id: &str) -> SqlResult<ChatSettings> {
    let mut stmt = conn.prepare(
        "SELECT system_prompt, context_limit FROM conversations WHERE id = ?1",
    )?;
    let row = stmt.query_row(params![id], |row| {
        Ok(ChatSettings {
            system_prompt: row.get(0)?,
            context_limit: row.get::<_, i64>(1)? as usize,
        })
    })?;
    Ok(row)
}

/// Sauvegarde les paramètres (prompt système + limite de contexte) d'une conversation.
pub fn conversation_settings_save(
    conn: &Connection,
    id: &str,
    settings: &ChatSettings,
) -> SqlResult<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE conversations SET system_prompt = ?1, context_limit = ?2, updated_at = ?3 WHERE id = ?4",
        params![settings.system_prompt, settings.context_limit as i64, now, id],
    )?;
    Ok(())
}

// --- Projet lié ---

/// Lit le dossier de projet et ses exclusions.
pub fn project_get(conn: &Connection) -> SqlResult<ProjectConfig> {
    let root_path: Option<String> = conn
        .query_row("SELECT root_path FROM project WHERE id = 1", [], |row| row.get(0))
        .optional()?;

    let mut stmt = conn.prepare("SELECT path FROM project_exclusions ORDER BY path ASC")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut exclusions = Vec::new();
    for row in rows {
        exclusions.push(row?);
    }

    Ok(ProjectConfig {
        root_path,
        exclusions,
    })
}

/// Lie un dossier de projet. Changer de dossier efface les exclusions précédentes.
pub fn project_set_root(conn: &Connection, root_path: &str) -> SqlResult<()> {
    conn.execute(
        "INSERT INTO project (id, root_path) VALUES (1, ?1)
         ON CONFLICT(id) DO UPDATE SET root_path = excluded.root_path",
        params![root_path],
    )?;
    conn.execute("DELETE FROM project_exclusions", [])?;
    Ok(())
}

/// Délie le dossier de projet et efface ses exclusions.
pub fn project_clear(conn: &Connection) -> SqlResult<()> {
    conn.execute("DELETE FROM project WHERE id = 1", [])?;
    conn.execute("DELETE FROM project_exclusions", [])?;
    Ok(())
}

/// Ajoute une exclusion (chemin relatif au projet, séparateurs `/`).
pub fn project_add_exclusion(conn: &Connection, path: &str) -> SqlResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO project_exclusions (path) VALUES (?1)",
        params![path],
    )?;
    Ok(())
}

/// Retire une exclusion.
pub fn project_remove_exclusion(conn: &Connection, path: &str) -> SqlResult<()> {
    conn.execute(
        "DELETE FROM project_exclusions WHERE path = ?1",
        params![path],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projet_persiste_en_base() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let empty = project_get(&conn).unwrap();
        assert_eq!(empty.root_path, None);
        assert!(empty.exclusions.is_empty());

        project_set_root(&conn, "/tmp/projet").unwrap();
        project_add_exclusion(&conn, "secret").unwrap();
        let config = project_get(&conn).unwrap();
        assert_eq!(config.root_path.as_deref(), Some("/tmp/projet"));
        assert_eq!(config.exclusions, vec!["secret".to_string()]);

        project_clear(&conn).unwrap();
        let config = project_get(&conn).unwrap();
        assert_eq!(config.root_path, None);
        assert!(config.exclusions.is_empty());
    }
}
