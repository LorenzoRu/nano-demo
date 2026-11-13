use crate::{ChatMessage, settings::ChatSettings};

/// Sépare les messages en deux tranches :
/// - `anciens` : tout ce qui dépasse la limite de contexte (à résumer / purger ultérieurement)
/// - `récents` : les `limit` derniers messages, conservés en brut
///
/// Si le nombre de messages est <= `limit`, `anciens` est vide et `récents` contient tout.
///
/// Une limite de 0 signifie "pas de limite" : tous les messages sont conservés dans `récents`.
/// Cela évite d'envoyer une liste vide à Ollama (ce qui produirait un `done: true` sans token).
pub fn split_messages(messages: &[ChatMessage], limit: usize) -> (&[ChatMessage], &[ChatMessage]) {
    let limit = if limit == 0 { usize::MAX } else { limit };
    if messages.len() <= limit {
        return (&[], messages);
    }
    let split = messages.len() - limit;
    (&messages[..split], &messages[split..])
}

/// Prépare la liste des messages à envoyer à Ollama en appliquant la stratégie de contexte.
///
/// - Les `context_limit` derniers messages sont conservés en brut.
/// - Les messages plus anciens sont mis de côté pour un résumé différé (stratégie "contexte à froid").
///   Le résumé sera généré par une étape séparée ultérieurement.
///
/// Le résultat est un mélange : [prompt système (si défini), messages bruts récents].
pub async fn build_context(
    messages: &[ChatMessage],
    settings: &ChatSettings,
    _model: &str,
) -> anyhow::Result<Vec<ChatMessage>> {
    let mut out = Vec::new();

    // Prompt système en premier, si défini (non vide)
    if !settings.system_prompt.is_empty() {
        out.push(ChatMessage {
            role: "system".to_string(),
            content: settings.system_prompt.clone(),
        });
    }

    let (_anciens, récents) = split_messages(messages, settings.context_limit);

    out.extend_from_slice(récents);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    #[tokio::test]
    async fn split_messages_under_limit_keeps_all_in_order() {
        let messages = vec![
            msg("user", "a"),
            msg("assistant", "b"),
            msg("user", "c"),
        ];
        let (old, recent) = split_messages(&messages, 20);
        assert!(old.is_empty());
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].content, "a");
        assert_eq!(recent[1].content, "b");
        assert_eq!(recent[2].content, "c");
    }

    #[tokio::test]
    async fn split_messages_over_limit_keeps_last_n() {
        let messages: Vec<_> = (0..30).map(|i| msg("user", &format!("m{i}"))).collect();
        let (old, recent) = split_messages(&messages, 20);
        assert_eq!(old.len(), 10);
        assert_eq!(recent.len(), 20);
        assert_eq!(recent[0].content, "m10");
        assert_eq!(recent.last().unwrap().content, "m29");
        assert_eq!(recent.last().unwrap().role, "user");
    }

    #[tokio::test]
    async fn split_messages_limit_zero_keeps_all() {
        let messages = vec![msg("user", "a"), msg("user", "b")];
        let (old, recent) = split_messages(&messages, 0);
        assert!(old.is_empty());
        assert_eq!(recent.len(), 2);
    }

    #[tokio::test]
    async fn build_context_with_system_prompt_first_once() {
        let messages = vec![msg("user", "bonjour")];
        let settings = ChatSettings::with_system_prompt("Tu es un assistant.".to_string());
        let out = build_context(&messages, &settings, "model").await.unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, "system");
        assert_eq!(out[0].content, "Tu es un assistant.");
        assert_eq!(out[1].role, "user");
        assert_eq!(out.iter().filter(|m| m.role == "system").count(), 1);
    }

    #[tokio::test]
    async fn build_context_empty_system_prompt_no_system_message() {
        let messages = vec![msg("user", "bonjour")];
        let settings = ChatSettings::default();
        let out = build_context(&messages, &settings, "model").await.unwrap();
        assert!(!out.iter().any(|m| m.role == "system"));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "user");
    }

    #[tokio::test]
    async fn build_context_limit_zero_never_empty() {
        let messages = vec![msg("user", "seul")];
        let settings = ChatSettings {
            system_prompt: String::new(),
            context_limit: 0,
        };
        let out = build_context(&messages, &settings, "model").await.unwrap();
        assert!(!out.is_empty());
        assert_eq!(out.last().unwrap().content, "seul");
    }

    #[tokio::test]
    async fn build_context_ends_with_last_input_message() {
        let messages = vec![
            msg("user", "premier"),
            msg("assistant", "deuxième"),
            msg("user", "dernier"),
        ];
        let settings = ChatSettings::default();
        let out = build_context(&messages, &settings, "model").await.unwrap();
        assert!(!out.is_empty());
        let last = out.last().unwrap();
        assert_eq!(last.content, "dernier");
        assert_eq!(last.role, "user");
    }
}
