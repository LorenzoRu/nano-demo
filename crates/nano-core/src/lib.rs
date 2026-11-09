use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

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
