use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub mod settings;
pub mod context;
pub mod tools;

pub mod status;

#[derive(Deserialize)]
struct ChatChunk {
    message: Option<Message>,
    #[serde(default)]
    done: bool,
}

#[derive(Deserialize)]
struct Message {
    content: String,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCallRaw>>,
}

#[derive(Deserialize, Clone)]
struct ToolCallRaw {
    function: ToolFunctionRaw,
}

#[derive(Deserialize, Clone)]
struct ToolFunctionRaw {
    name: String,
    /// Ollama renvoie un objet JSON ; certains modèles/anciennes versions renvoient une chaîne.
    arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Envoie un prompt à Ollama et appelle `on_token` pour chaque morceau reçu.
/// Si `cancel` passe à `true`, le stream s'arrête et renvoie une erreur.
pub async fn stream_chat(
    model: &str,
    messages: &[ChatMessage],
    mut on_token: impl FnMut(&str),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let res = reqwest::Client::new()
        .post("http://localhost:11434/api/chat")
        .json(&json!({
            "model": model,
            "messages": messages,
            "stream": true
        }))
        .send()
        .await?
        .error_for_status()?;

    let mut stream = res.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();

    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow::anyhow!("stream cancelled"));
        }
        buf.extend_from_slice(&chunk?);
        // Le NDJSON peut être coupé en plein milieu d'une ligne : on ne parse que les lignes complètes.
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            if line.trim_ascii().is_empty() {
                continue;
            }
            let parsed: ChatChunk = serde_json::from_slice(&line)?;
            if let Some(m) = parsed.message {
                on_token(&m.content);
                if cancel.load(Ordering::Relaxed) {
                    return Err(anyhow::anyhow!("stream cancelled"));
                }
            }
            if parsed.done {
                return Ok(());
            }
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(anyhow::anyhow!("stream cancelled"));
    }
    Ok(())
}

/// Envoie un prompt à Ollama avec des outils disponibles, stream les tokens vers `on_token`,
/// et retourne le message complet (incluant les appels d'outils détectés).
pub async fn stream_chat_tools(
    model: &str,
    messages: &[crate::tools::AgentMessage],
    tools: Option<&[crate::tools::ToolDefinition]>,
    mut on_token: impl FnMut(&str),
    cancel: &AtomicBool,
) -> anyhow::Result<crate::tools::AgentMessage> {
    let mut request = json!({
        "model": model,
        "messages": crate::tools::agent_messages_to_ollama(messages),
        "stream": true,
        "options": {
            // Un petit modèle doit rester déterministe et borné pour éviter les boucles.
            "temperature": 0,
            "num_predict": 1024
        }
    });

    if let Some(tools) = tools {
        request["tools"] = Value::Array(
            tools.iter().map(|tool| tool.to_ollama_json()).collect(),
        );
    }

    let res = reqwest::Client::new()
        .post("http://localhost:11434/api/chat")
        .json(&request)
        .send()
        .await?
        .error_for_status()?;

    let mut stream = res.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut full_content = String::new();
    let mut full_tool_calls: Vec<crate::tools::ToolCall> = Vec::new();

    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow::anyhow!("stream cancelled"));
        }
        buf.extend_from_slice(&chunk?);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            if line.trim_ascii().is_empty() {
                continue;
            }
            let parsed: ChatChunk = serde_json::from_slice(&line)?;
            if let Some(m) = parsed.message {
                on_token(&m.content);
                full_content.push_str(&m.content);
                if let Some(raw_calls) = m.tool_calls {
                    for call in raw_calls {
                        full_tool_calls.push(crate::tools::ToolCall {
                            name: call.function.name,
                            arguments: crate::tools::normalize_tool_arguments(
                                call.function.arguments,
                            ),
                        });
                    }
                }
                if cancel.load(Ordering::Relaxed) {
                    return Err(anyhow::anyhow!("stream cancelled"));
                }
            }
            if parsed.done {
                // Si aucun tool_call natif, tenter d'extraire depuis le contenu textuel
                if full_tool_calls.is_empty() {
                    full_tool_calls = crate::tools::extract_tool_calls(&full_content)
                        .unwrap_or_default();
                }
                return Ok(crate::tools::AgentMessage {
                    role: "assistant".to_string(),
                    content: full_content,
                    tool_calls: full_tool_calls,
                    tool_name: None,
                });
            }
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(anyhow::anyhow!("stream cancelled"));
    }
    // Fallback final sur le contenu textuel
    if full_tool_calls.is_empty() {
        full_tool_calls = crate::tools::extract_tool_calls(&full_content)
            .unwrap_or_default();
    }
    Ok(crate::tools::AgentMessage {
        role: "assistant".to_string(),
        content: full_content,
        tool_calls: full_tool_calls,
        tool_name: None,
    })
}

/// Exécute un tour complet de boucle agent :
/// 1. Demande au modèle une réponse (éventuellement avec appels d'outils)
/// 2. Si le modèle demande des outils, les exécute via `execute_tool`
/// 3. Réinjecte les résultats et redemande au modèle
///
/// Répète jusqu'à ce qu'aucun outil ne soit demandé.
///
/// `execute_tool` reçoit un `ToolCall` et doit retourner un `ToolResult` (async).
/// `on_token` est appelé pour la réponse finale (les échanges d'outils sont masqués).
pub async fn run_agent_turn(
    model: &str,
    messages: &[ChatMessage],
    tools: &[crate::tools::ToolDefinition],
    execute_tool: impl Fn(crate::tools::ToolCall) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::tools::ToolResult> + Send>>,
    mut on_token: impl FnMut(&str),
    cancel: &AtomicBool,
) -> anyhow::Result<Vec<crate::tools::AgentMessage>> {
    let mut all_messages: Vec<crate::tools::AgentMessage> =
        messages.iter().map(Into::into).collect();
    let mut max_iterations = 10; // évite les boucles infinies

    while max_iterations > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow::anyhow!("stream cancelled"));
        }
        max_iterations -= 1;

        // On bufferise le contenu pendant le stream pour décider s'il faut l'afficher.
        // Si le modèle demande des outils, le JSON brut est masqué.
        let mut buffered = String::new();
        let response = stream_chat_tools(
            model,
            &all_messages,
            Some(tools),
            |token| { buffered.push_str(token); },
            cancel,
        ).await?;

        // Si aucun outil n'est demandé, on affiche la réponse finale
        if response.tool_calls.is_empty() {
            for token in buffered.split_inclusive(' ') {
                if cancel.load(Ordering::Relaxed) {
                    return Err(anyhow::anyhow!("stream cancelled"));
                }
                on_token(token);
            }
            all_messages.push(crate::tools::AgentMessage::assistant(
                response.content,
                Vec::new(),
            ));
            return Ok(all_messages);
        }

        // Réinjecte l'appel d'outil comme message assistant (format Ollama natif)
        for tool_call in &response.tool_calls {
            all_messages.push(crate::tools::tool_call_to_message(tool_call));
        }

        // Exécute chaque outil et réinjecte le résultat
        let mut results = Vec::new();
        for tool_call in &response.tool_calls {
            let result = execute_tool(tool_call.clone()).await;
            all_messages.push(crate::tools::tool_result_to_message(&tool_call.name, &result));
            results.push(result);
        }

        // Un refus de sécurité est une réponse finale en soi : on ne laisse pas le
        // modèle inventer une réussite à partir du message d'erreur.
        if let Some(message) = crate::tools::fatal_error_message(&results) {
            for token in message.split_inclusive(' ') {
                if cancel.load(Ordering::Relaxed) {
                    return Err(anyhow::anyhow!("stream cancelled"));
                }
                on_token(token);
            }
            all_messages.push(crate::tools::AgentMessage::assistant(message, Vec::new()));
            return Ok(all_messages);
        }
    }

    Err(anyhow::anyhow!("boucle agent : nombre maximum d'itérations atteint"))
}
