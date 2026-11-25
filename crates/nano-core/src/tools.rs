use crate::ChatMessage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Un appel d'outil demandé par le modèle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// Résultat d'exécution d'un outil.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub name: String,
    pub content: String,
    pub is_error: bool,
    /// Refus de sécurité : la boucle agent ne doit pas laisser le modèle conclure.
    pub fatal: bool,
}

impl ToolResult {
    pub fn ok(name: String, content: String) -> Self {
        Self { name, content, is_error: false, fatal: false }
    }

    pub fn error(name: String, content: impl Into<String>) -> Self {
        Self { name, content: content.into(), is_error: true, fatal: false }
    }

    /// Erreur de permissions : réponse déterministe, sans génération du modèle.
    pub fn denied(name: String, content: impl Into<String>) -> Self {
        Self { name, content: content.into(), is_error: true, fatal: true }
    }
}

/// Déclaration d'un outil disponible pour le modèle.
///
/// Le format reste neutre : `to_ollama_json` fait la conversion vers le format
/// attendu par l'API Ollama (`{"type": "function", "function": {...}}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl ToolDefinition {
    /// Convertit la définition vers le format `tools` de `/api/chat`.
    pub fn to_ollama_json(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.input_schema,
            }
        })
    }
}

/// Un message de la boucle agent, pouvant porter des appels d'outils
/// ou un nom d'outil pour les messages de rôle `tool`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

impl AgentMessage {
    pub fn assistant(content: String, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            tool_calls,
            tool_name: None,
        }
    }
}

impl From<&ChatMessage> for AgentMessage {
    fn from(message: &ChatMessage) -> Self {
        Self {
            role: message.role.clone(),
            content: message.content.clone(),
            tool_calls: Vec::new(),
            tool_name: None,
        }
    }
}

/// Convertit une liste de messages agent vers le JSON attendu par `/api/chat`.
pub fn agent_messages_to_ollama(messages: &[AgentMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| {
            let mut value = json!({
                "role": message.role,
                "content": message.content,
            });
            if !message.tool_calls.is_empty() {
                value["tool_calls"] = Value::Array(
                    message
                        .tool_calls
                        .iter()
                        .map(|call| {
                            json!({
                                "function": {
                                    "name": call.name,
                                    // Ollama attend un objet JSON ici, pas une chaîne.
                                    "arguments": call.arguments,
                                }
                            })
                        })
                        .collect(),
                );
            }
            if let Some(tool_name) = &message.tool_name {
                value["tool_name"] = Value::String(tool_name.clone());
            }
            value
        })
        .collect()
}

/// Extrait le premier objet JSON équilibré d'un texte.
/// Gère le texte libre autour et les blocs de code markdown.
fn first_json_object(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, byte) in bytes.iter().enumerate() {
        if start.is_none() {
            if *byte == b'{' {
                start = Some(index);
                depth = 1;
            }
            continue;
        }

        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }

        match *byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start.expect("start défini")..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Normalise les arguments d'un outil : Ollama peut renvoyer un objet JSON
/// (format actuel) ou une chaîne contenant du JSON (anciens modèles).
pub fn normalize_tool_arguments(arguments: Value) -> Value {
    match arguments {
        Value::String(raw) => serde_json::from_str(&raw).unwrap_or(Value::String(raw)),
        other => other,
    }
}

/// Tente d'extraire des appels d'outils depuis le contenu brut d'un message.
/// Accepte les formats :
/// - {"tool_calls": [{"function": {"name": "...", "arguments": {...}}}]} (format Ollama natif)
/// - {"tool_calls": [{"name": "...", "arguments": {...}}]}
/// - {"name": "...", "arguments": {...}}
///
/// Le JSON peut être entouré de texte libre ou d'un bloc de code markdown.
/// Retourne None si aucun appel n'est détecté.
pub fn extract_tool_calls(text: &str) -> Option<Vec<ToolCall>> {
    let candidate = first_json_object(text)?;
    let parsed: Value = serde_json::from_str(candidate).ok()?;

    // Format 1 : {"name": "...", "arguments": {...}} (appel unique)
    if let Some(name) = parsed.get("name").and_then(|v| v.as_str())
        && let Some(arguments) = parsed.get("arguments")
    {
        return Some(vec![ToolCall {
            name: name.to_string(),
            arguments: normalize_tool_arguments(arguments.clone()),
        }]);
    }

    // Format 2 : {"tool_calls": [...]}
    if let Some(calls_raw) = parsed.get("tool_calls").and_then(|v| v.as_array()) {
        let calls: Vec<ToolCall> = calls_raw
            .iter()
            .filter_map(|call| {
                // Sous-format natif Ollama : {"function": {"name": "...", "arguments": ...}}
                if let Some(function) = call.get("function") {
                    let name = function.get("name")?.as_str()?.to_string();
                    let arguments = function.get("arguments")?.clone();
                    return Some(ToolCall {
                        name,
                        arguments: normalize_tool_arguments(arguments),
                    });
                }
                // Sous-format simple : {"name": "...", "arguments": ...}
                let name = call.get("name")?.as_str()?.to_string();
                let arguments = call.get("arguments")?.clone();
                Some(ToolCall {
                    name,
                    arguments: normalize_tool_arguments(arguments),
                })
            })
            .collect();
        if !calls.is_empty() {
            return Some(calls);
        }
    }

    None
}

/// Message déterministe à afficher quand un outil a refusé l'accès.
/// Le modèle ne doit pas être rappelé pour commenter un refus de sécurité.
pub fn fatal_error_message(results: &[ToolResult]) -> Option<String> {
    results
        .iter()
        .find(|result| result.fatal)
        .map(|result| format!("{} : {}", result.name, result.content))
}

/// Convertit un ToolResult en message à réinjecter dans la conversation.
pub fn tool_result_to_message(tool_name: &str, result: &ToolResult) -> AgentMessage {
    let content = if result.is_error {
        format!("[ERREUR] {}", result.content)
    } else {
        result.content.clone()
    };
    AgentMessage {
        role: "tool".to_string(),
        content,
        tool_calls: Vec::new(),
        tool_name: Some(tool_name.to_string()),
    }
}

/// Convertit un ToolCall en message assistant à réinjecter (format Ollama natif).
pub fn tool_call_to_message(tool_call: &ToolCall) -> AgentMessage {
    AgentMessage::assistant(String::new(), vec![tool_call.clone()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definition_uses_ollama_function_shape() {
        let definition = ToolDefinition {
            name: "read_file".to_string(),
            description: "Lit un fichier.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        };

        let value = definition.to_ollama_json();
        assert_eq!(value["type"], "function");
        assert_eq!(value["function"]["name"], "read_file");
        assert_eq!(value["function"]["description"], "Lit un fichier.");
        assert_eq!(value["function"]["parameters"]["required"][0], "path");
        assert!(value.get("input_schema").is_none());
    }

    #[test]
    fn extracts_flat_call_with_object_arguments() {
        let text = r#"Je vais lister le dossier.
        {"name": "list_dir", "arguments": {"path": "/tmp"}}"#;
        let calls = extract_tool_calls(text).expect("appel attendu");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "list_dir");
        assert_eq!(calls[0].arguments["path"], "/tmp");
    }

    #[test]
    fn extracts_native_call_with_string_arguments() {
        let text = r#"{"tool_calls":[{"function":{"name":"read_file","arguments":"{\"path\":\"/tmp/a.txt\"}"}}]}"#;
        let calls = extract_tool_calls(text).expect("appel attendu");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments["path"], "/tmp/a.txt");
    }

    #[test]
    fn extracts_native_call_with_object_arguments() {
        let text = r#"```json
        {"tool_calls":[{"function":{"name":"read_file","arguments":{"path":"/tmp/a.txt"}}}]}
        ```"#;
        let calls = extract_tool_calls(text).expect("appel attendu");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments["path"], "/tmp/a.txt");
    }

    #[test]
    fn ignores_plain_text() {
        assert!(extract_tool_calls("Pas d'outil ici.").is_none());
    }

    #[test]
    fn agent_messages_use_ollama_native_tool_format() {
        let call = ToolCall {
            name: "list_dir".to_string(),
            arguments: json!({ "path": "/tmp" }),
        };
        let messages = vec![
            tool_call_to_message(&call),
            tool_result_to_message("list_dir", &ToolResult {
                name: "list_dir".to_string(),
                content: "a.txt\nb.txt".to_string(),
                is_error: false,
                fatal: false,
            }),
        ];

        let value = agent_messages_to_ollama(&messages);
        assert_eq!(value[0]["role"], "assistant");
        assert_eq!(value[0]["tool_calls"][0]["function"]["name"], "list_dir");
        assert_eq!(value[0]["tool_calls"][0]["function"]["arguments"]["path"], "/tmp");
        assert_eq!(value[1]["role"], "tool");
        assert_eq!(value[1]["tool_name"], "list_dir");
        assert_eq!(value[1]["content"], "a.txt\nb.txt");
    }

    #[test]
    fn fatal_error_message_is_deterministic() {
        let results = vec![
            ToolResult::ok("read_file".to_string(), "contenu".to_string()),
            ToolResult::error("list_dir".to_string(), "chemin introuvable"),
            ToolResult::denied(
                "write_file".to_string(),
                "Accès refusé : hors du dossier de projet.",
            ),
        ];

        assert_eq!(
            fatal_error_message(&results).as_deref(),
            Some("write_file : Accès refusé : hors du dossier de projet.")
        );
        assert!(fatal_error_message(&results[..2]).is_none());
    }
}
