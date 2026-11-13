use serde::{Deserialize, Serialize};

/// Paramètres de chat par conversation (prompt système + limite de contexte).
///
/// nano-core reste sans état : ce struct ne contient que des données,
/// aucun accès disque, aucun appel réseau.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSettings {
    /// Instruction de rôle envoyée au modèle au début de chaque conversation.
    pub system_prompt: String,
    /// Nombre de derniers messages conservés en brut dans l'historique envoyé au modèle.
    /// Au-delà, les messages anciens passent en "contexte à froid" (résumé).
    pub context_limit: usize,
}

impl Default for ChatSettings {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            context_limit: 20,
        }
    }
}

impl ChatSettings {
    /// Crée des paramètres avec un prompt système personnalisé et la limite par défaut.
    pub fn with_system_prompt(system_prompt: String) -> Self {
        Self {
            system_prompt,
            ..Default::default()
        }
    }

    /// Crée des paramètres avec une limite de contexte personnalisée.
    pub fn with_context_limit(mut self, context_limit: usize) -> Self {
        self.context_limit = context_limit;
        self
    }
}
