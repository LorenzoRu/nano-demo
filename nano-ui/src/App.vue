<script setup lang="ts">
import { ref, watch, onMounted } from "vue";
import { invoke } from "@tauri-apps/api/core";
import Chat from "./components/Chat.vue";
import OllamaDashboard from "./components/OllamaDashboard.vue";

const tab = ref<"chat" | "dashboard">("chat");

// Thème sombre/clair : persistant dans localStorage, sinon sombre par défaut
const stored = localStorage.getItem("nano-theme");
const isDark = ref(stored ? stored === "dark" : true);

function toggleTheme() {
  isDark.value = !isDark.value;
}

async function toggleFullscreen() {
  await invoke("toggle_fullscreen");
}

const fullscreenIcon = "Plein écran";
const fullscreenLabel = "Basculer en plein écran";

onMounted(() => {
  updateRootClass();
});

watch(isDark, () => {
  localStorage.setItem("nano-theme", isDark.value ? "dark" : "light");
  updateRootClass();
});

function updateRootClass() {
  document.documentElement.classList.toggle("dark", isDark.value);
  document.documentElement.classList.toggle("light", !isDark.value);
}
</script>

<template>
  <div class="app" :class="{ dark: isDark, light: !isDark }">
    <nav>
      <button :class="{ active: tab === 'chat' }" @click="tab = 'chat'">Chat</button>
      <button :class="{ active: tab === 'dashboard' }" @click="tab = 'dashboard'">
        Tableau de bord
      </button>
      <button class="theme-toggle" @click="toggleTheme" :aria-label="isDark ? 'Passer en mode clair' : 'Passer en mode sombre'">
        {{ isDark ? "Passer en mode clair" : "Passer en mode sombre" }}
      </button>
      <button class="fullscreen-toggle" @click="toggleFullscreen" :aria-label="fullscreenLabel">
        {{ fullscreenIcon }}
      </button>
    </nav>
    <div class="content">
      <Chat v-show="tab === 'chat'" />
      <div v-if="tab === 'dashboard'" class="scroll"><OllamaDashboard /></div>
    </div>
  </div>
</template>

<style>
:root {
  color-scheme: light dark;
  font-family: system-ui, sans-serif;

  /* Couleurs thème sombre (par défaut) */
  --color-bg: #0d1117;
  --color-bg-panel: #161b22;
  --color-text: #c9d1d9;
  --color-text-muted: #8b949e;
  --color-border: #30363d;
  --color-border-hover: #8b949e;
  --color-nav-border: #8883;
  --color-user-msg: #3b82f633;
  --color-assistant-msg: #8882;
  --color-textarea-border: #8884;
  --color-code-bg: #111827;
  --color-code-text: #c9d1d9;
  --color-code-header-bg: #1f2937;
  --color-code-header-border: #30363d;
  --color-code-lang: #8b949e;
  --color-hint: #6b7280;
  --color-error: #d73a49;
  --color-stop-bg: #d73a49;
  --color-stop-text: #fff;
  --color-typing-dot: #9ca3af;
  --color-blockquote-border: #3b82f6;
  --color-blockquote-text: #4b5563;
  --color-heading: #111827;
  --color-hr: #e5e7eb;
  --color-table-border: #8883;
  --color-code-inline-bg: #8882;
  --color-badge-border: #8884;
  --color-badge-dot: #888;
  --color-badge-dot-ok: #2ea043;
  --color-badge-dot-ko: #d73a49;
}

:root.light {
  /* Couleurs thème clair */
  --color-bg: #ffffff;
  --color-bg-panel: #f3f4f6;
  --color-text: #111827;
  --color-text-muted: #6b7280;
  --color-border: #d1d5db;
  --color-border-hover: #374151;
  --color-nav-border: #8883;
  --color-user-msg: #3b82f64d;
  --color-assistant-msg: #e5e7eb;
  --color-textarea-border: #d1d5db;
  --color-code-bg: #1f2937;
  --color-code-text: #e5e7eb;
  --color-code-header-bg: #111827;
  --color-code-header-border: #374151;
  --color-code-lang: #9ca3af;
  --color-hint: #6b7280;
  --color-error: #dc2626;
  --color-stop-bg: #dc2626;
  --color-stop-text: #fff;
  --color-typing-dot: #9ca3af;
  --color-blockquote-border: #3b82f6;
  --color-blockquote-text: #4b5563;
  --color-heading: #111827;
  --color-hr: #e5e7eb;
  --color-table-border: #d1d5db;
  --color-code-inline-bg: #e5e7eb;
  --color-badge-border: #d1d5db;
  --color-badge-dot: #9ca3af;
  --color-badge-dot-ok: #16a34a;
  --color-badge-dot-ko: #dc2626;
}

html, body, #app { height: 100%; margin: 0; }

.app {
  display: flex;
  flex-direction: column;
  height: 100vh;
}

body {
  background: var(--color-bg);
  color: var(--color-text);
  font-family: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Noto Color Emoji", "Segoe UI Emoji", "Apple Color Emoji", sans-serif;
  transition: background-color 0.2s, color 0.2s;
}

nav {
  display: flex;
  gap: 0.5rem;
  padding: 0.6rem 1rem;
  border-bottom: 1px solid var(--color-nav-border);
}

nav button.active { font-weight: 600; text-decoration: underline; }
.theme-toggle { background: none; border: none; cursor: pointer; font-size: 1.1rem; padding: 0.2rem; color: var(--color-text); }
.theme-toggle:hover { opacity: 0.8; }
.fullscreen-toggle { background: none; border: none; cursor: pointer; font-size: 1.1rem; padding: 0.2rem; color: var(--color-text); }
.fullscreen-toggle:hover { opacity: 0.8; }
.content { flex: 1; min-height: 0; display: flex; flex-direction: column; }
.content > * { flex: 1; min-height: 0; }
.scroll { overflow-y: auto; }
</style>
