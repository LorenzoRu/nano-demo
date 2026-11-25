# nano

Application desktop locale qui discute avec un modèle Ollama. La version démo comprend le chat en streaming, une boucle agent avec outils sur un dossier projet lié et la sauvegarde des conversations en SQLite.

## Branche de référence

- **`demo` est la branche de la démo** : c'est la seule branche à pousser sur GitHub (`origin`).
- **Ne jamais pousser la branche `main` sur GitHub.** Si `main` existe sur le dépôt distant (premier publication erronée), changer la branche par défaut du dépôt en `demo` dans les réglages GitHub, puis supprimer `main`.

```bash
git switch demo
git pull origin demo
git push -u origin demo
```

## Prérequis

- Rust (édition 2024) et Node.js/npm.
- Dépendances système Tauri 2 (WebKitGTK sous Linux/WSL).
- Ollama accessible sur `http://localhost:11434`, avec au moins un modèle installé. Le modèle par défaut est choisi depuis la liste renvoyée par `ollama_status()` ; `qwen2.5-coder:3b` reste le modèle de test du CLI de debug.

## Lancer l'application

```bash
cd nano-ui
npm ci
npm run tauri dev
```

Sous WSL, utiliser `npm run tauri:wsl` (backend X11). Pour le seul frontend Vite sur `http://localhost:1420` : `npm run dev`.

CLI de debug (streaming vers stdout) :

```bash
cargo run -p nano-app
```

## Vérifications

```bash
cargo check --workspace
cargo test --workspace
cd nano-ui && npm run build
```

`npm run build` doit être exécuté avant un build Tauri : Tauri lit `frontendDist: ../dist`.

## Architecture

- `crates/nano-core` : logique pure (client Ollama, boucle agent, outils), sans dépendance à une interface.
- `crates/nano-app` : CLI de debug au-dessus du cœur.
- `nano-ui/src-tauri` : adaptateur Tauri 2, commandes et validation des permissions de fichiers (`ProjectPolicy`).
- `nano-ui/src` : interface Vue 3 + TypeScript.

Le flux du chat passe par `invoke("chat", ...)` → commande Tauri → `nano_core::stream_chat` → `/api/chat` d'Ollama ; les tokens remontent via un `Channel<String>`. Le mode agent utilise `chat_agent`, `build_context` et `run_agent_turn`, avec les outils `read_file`, `write_file` et `list_dir` validés côté Tauri et limités au dossier du projet lié.

## Données

La base SQLite est créée au premier lancement : aucune conversation ni donnée n'est hardcodée. Par défaut, `system_prompt = ""` et `context_limit = 20` ; les conversations sont créées depuis l'interface.

## Périmètre de la démo

Inclus : dashboard Ollama, chat en streaming avec sélecteur de modèle, bouton Stop, rendu markdown, thème clair/sombre, conversations SQLite, prompt système et contexte par conversation, option Projet (dossier lié, permissions, exclusions, `write_file`).

Hors périmètre : bot Discord, résumé du contexte à froid (les anciens messages sont seulement tronqués) et optimisations dédiées aux modèles 3B.
