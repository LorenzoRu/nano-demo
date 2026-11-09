use serde::{Deserialize, Serialize};
use std::time::Duration;

const OLLAMA_URL: &str = "http://localhost:11434";

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub size_bytes: u64,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
    pub modified_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OllamaStatus {
    pub connected: bool,
    pub version: Option<String>,
    pub models: Vec<ModelInfo>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<TagModel>,
}

#[derive(Deserialize)]
struct TagModel {
    name: String,
    size: u64,
    modified_at: String,
    #[serde(default)]
    details: TagDetails,
}

#[derive(Deserialize, Default)]
struct TagDetails {
    parameter_size: Option<String>,
    quantization_level: Option<String>,
}

#[derive(Deserialize)]
struct VersionResponse {
    version: String,
}

/// Ne renvoie jamais d'erreur : un Ollama injoignable est un état normal (`connected: false`).
pub async fn ollama_status() -> OllamaStatus {
    match fetch_status().await {
        Ok(status) => status,
        Err(e) => OllamaStatus {
            connected: false,
            version: None,
            models: vec![],
            error: Some(e.to_string()),
        },
    }
}

async fn fetch_status() -> anyhow::Result<OllamaStatus> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;

    let tags: TagsResponse = client
        .get(format!("{OLLAMA_URL}/api/tags"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    // La version est un bonus : on n'échoue pas si elle manque.
    let version = match client.get(format!("{OLLAMA_URL}/api/version")).send().await {
        Ok(res) => res.json::<VersionResponse>().await.ok().map(|v| v.version),
        Err(_) => None,
    };

    let models = tags
        .models
        .into_iter()
        .map(|m| ModelInfo {
            name: m.name,
            size_bytes: m.size,
            parameter_size: m.details.parameter_size,
            quantization: m.details.quantization_level,
            modified_at: m.modified_at,
        })
        .collect();

    Ok(OllamaStatus {
        connected: true,
        version,
        models,
        error: None,
    })
}