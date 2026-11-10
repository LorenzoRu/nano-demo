<script setup lang="ts">
import { ref, onMounted, nextTick, watch } from "vue";
import { invoke, Channel } from "@tauri-apps/api/core";
import { marked } from "marked";
import hljs from "highlight.js";
import DOMPurify from "dompurify";

interface ChatMessage {
  role: "user" | "assistant" | "system";
  content: string;
}
interface OllamaStatus {
  connected: boolean;
  models: { name: string }[];
}

const models = ref<string[]>([]);
const model = ref("");
const messages = ref<ChatMessage[]>([]);
const input = ref("");
const busy = ref(false);
const stopping = ref(false);
const error = ref<string | null>(null);
const list = ref<HTMLElement | null>(null);
const stoppedByUserIdx = ref<number | null>(null);
const currentAssistantIdx = ref<number | null>(null);
const awaitingFirstToken = ref(false);

// Clé de persistance de la conversation courante dans localStorage
const STORAGE_KEY = "nano-conversation";

function saveMessages() {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(messages.value));
  } catch {
    // localStorage indisponible ou quota dépassé : on ignore silencieusement
  }
}

function loadMessages(): ChatMessage[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as ChatMessage[];
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

// Restaure la conversation sauvegardée au montage, puis autosauvegarde à chaque changement
watch(messages, () => {
  saveMessages();
}, { deep: true });

async function copyToClipboard(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    try {
      const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
      await writeText(text);
    } catch (e) {
      console.error("Échec copie presse-papiers:", e);
    }
  }
}

function renderMarkdown(content: string): string {
  const html = marked.parse(content, { breaks: true, gfm: true }) as string;
  const div = document.createElement("div");
  div.innerHTML = DOMPurify.sanitize(html);

  div.querySelectorAll("pre > code").forEach((block) => {
    const pre = block.parentElement as HTMLElement;

    const match = /language-([\w+-]+)/.exec(block.className);
    const language = match?.[1] ?? "";

    // Coloration seulement si la langue est connue (sinon on garde le texte brut)
    if (language && hljs.getLanguage(language)) {
      block.innerHTML = hljs.highlight(block.textContent ?? "", {
        language,
        ignoreIllegals: true,
      }).value;
    }
    block.classList.add("hljs");

    const wrapper = document.createElement("div");
    wrapper.className = "code-wrapper";

    const header = document.createElement("div");
    header.className = "code-header";

    const langLabel = document.createElement("span");
    langLabel.className = "code-lang";
    langLabel.textContent = language
      ? language.charAt(0).toUpperCase() + language.slice(1)
      : "Texte";

    const btn = document.createElement("button");
    btn.className = "copy-btn";
    btn.type = "button";
    btn.textContent = "Copier";

    header.append(langLabel, btn);

    // D'abord remplacer le <pre> par le wrapper, ensuite y ranger l'en-tête et le <pre>
    pre.replaceWith(wrapper);
    wrapper.append(header, pre);
  });

  return div.innerHTML;
}

function onMessageClick(evt: MouseEvent) {
  const btn = (evt.target as HTMLElement).closest(".copy-btn") as HTMLElement | null;
  if (!btn) return;
  const wrapper = btn.closest(".code-wrapper") as HTMLElement | null;
  if (!wrapper) return;
  const pre = wrapper.querySelector("pre");
  if (!pre) return;
  const code = pre.querySelector("code");
  const text = code ? code.textContent ?? "" : pre.textContent ?? "";
  copyToClipboard(text);
  btn.textContent = "Copié ✓";
  setTimeout(() => {
    btn.textContent = "Copier";
  }, 2000);
}

onMounted(async () => {
  // Restaure la conversation sauvegardée (si aucune Ollama n'est joignable,
  // l'historique local reste accessible).
  messages.value = loadMessages();

  try {
    const status = await invoke<OllamaStatus>("get_ollama_status");
    models.value = status.models.map((m) => m.name);
    model.value = models.value[0] ?? "";
  } catch (e) {
    error.value = String(e);
  }
});

async function send() {
  const text = input.value.trim();
  if (!text || busy.value || !model.value) return;

  error.value = null;
  stoppedByUserIdx.value = null;
  awaitingFirstToken.value = true;
  input.value = "";
  messages.value.push({ role: "user", content: text });
  const history = [...messages.value]; // historique envoyé, sans la réponse vide
  const idx = messages.value.push({ role: "assistant", content: "" }) - 1;
  currentAssistantIdx.value = idx;

  const onToken = new Channel<string>();
  onToken.onmessage = (token) => {
    if (awaitingFirstToken.value) awaitingFirstToken.value = false;
    messages.value[idx].content += token;
    nextTick(() => {
      list.value?.scrollTo({ top: list.value.scrollHeight });
    });
  };

  busy.value = true;
  stopping.value = false;
  try {
    await invoke("chat", { model: model.value, messages: history, onToken });
  } catch (e) {
    // Si l'annulation a été demandée, ce n'est pas une erreur utilisateur.
    if (!stopping.value) {
      error.value = String(e);
    }
    if (messages.value[idx].content === "") messages.value.splice(idx, 1);
  } finally {
    busy.value = false;
    stopping.value = false;
    awaitingFirstToken.value = false;
    currentAssistantIdx.value = null;
  }
}

async function stop() {
  stopping.value = true;
  awaitingFirstToken.value = false;
  if (currentAssistantIdx.value !== null) {
    stoppedByUserIdx.value = currentAssistantIdx.value;
  }
  try {
    await invoke("cancel_chat");
  } catch (e) {
    error.value = String(e);
  } finally {
    busy.value = false;
    stopping.value = false;
  }
}
</script>

<template>
  <section class="chat">
    <header>
      <select v-model="model" :disabled="busy || models.length === 0">
        <option v-if="models.length === 0" value="">Aucun modèle</option>
        <option v-for="m in models" :key="m" :value="m">{{ m }}</option>
      </select>
      <button :disabled="busy || messages.length === 0" @click="messages = []">
        Nouvelle conversation
      </button>
    </header>

    <div ref="list" class="messages" @click="onMessageClick">
      <p v-if="messages.length === 0" class="hint">Pose une question à ton modèle local.</p>
      <div v-for="(m, i) in messages" :key="i" class="msg" :class="m.role">
        <div v-if="m.role === 'assistant'" class="markdown-content" v-html="renderMarkdown(m.content)"></div>
        <div v-else>{{ m.content }}</div>
        <span v-if="m.role === 'assistant' && i === currentAssistantIdx && busy && awaitingFirstToken" class="typing-indicator">
          <span></span><span></span><span></span>
        </span>
        <span v-if="stoppedByUserIdx !== null && i === stoppedByUserIdx" class="stopped-hint">
          Message arrêté par l'utilisateur
        </span>
      </div>
    </div>

    <p v-if="error" class="error">{{ error }}</p>

    <form @submit.prevent="send">
      <textarea
        v-model="input"
        rows="2"
        placeholder="Écris ton message (Entrée pour envoyer, Maj+Entrée pour un saut de ligne)"
        @keydown.enter.exact.prevent="send"
      />
      <button type="submit" v-if="!busy" :disabled="!input.trim() || !model">Envoyer</button>
      <button v-else @click="stop" class="stop">Stop</button>
    </form>
  </section>
</template>

<style>
/* Thème highlight.js sombre cohérent avec le fond #0d1117 */
@import "highlight.js/styles/github-dark.css";
</style>

<style scoped>
.chat { display: flex; flex-direction: column; height: 100%; max-width: 820px; margin: 0 auto; padding: 1rem 1.5rem; box-sizing: border-box; }
header { display: flex; gap: 0.75rem; justify-content: space-between; margin-bottom: 0.75rem; }
.messages { flex: 1; min-height: 0; overflow-y: auto; display: flex; flex-direction: column; gap: 0.4rem; padding: 0.5rem 0; }
.msg { padding: 0.5rem 0.8rem; border-radius: 10px; max-width: 85%; line-height: 1.45; }
.msg.user { white-space: pre-wrap; align-self: flex-end; background: var(--color-user-msg); }
.msg.assistant { align-self: flex-start; background: var(--color-assistant-msg); }
.stopped-hint { display: block; font-size: 0.7rem; color: var(--color-hint); margin-top: 0.25rem; }
.typing-indicator { display: flex; gap: 4px; align-items: center; margin: 0.5rem 0 0; font-size: 0.9rem; color: var(--color-text-muted); align-self: flex-start; }
.typing-indicator span { width: 8px; height: 8px; border-radius: 50%; background: var(--color-typing-dot); animation: typing 1.4s infinite ease-in-out both; }
.typing-indicator span:nth-child(1) { animation-delay: 0s; }
.typing-indicator span:nth-child(2) { animation-delay: 0.2s; }
.typing-indicator span:nth-child(3) { animation-delay: 0.4s; }
@keyframes typing { 0%, 100% { opacity: 0.4; transform: scale(1); } 50% { opacity: 1; transform: scale(1.2); } }
button.stop { background: var(--color-stop-bg); color: var(--color-stop-text); border: none; border-radius: 6px; padding: 0.25rem 0.75rem; cursor: pointer; }
button.stop:disabled { opacity: 0.5; cursor: not-allowed; }
.hint { opacity: 0.6; text-align: center; margin-top: 2rem; }
.error { color: var(--color-error); margin: 0.4rem 0; }
form { display: flex; gap: 0.6rem; align-items: flex-end; margin-top: 0.5rem; }
textarea { flex: 1; resize: none; padding: 0.55rem; border-radius: 8px; border: 1px solid var(--color-textarea-border); font: inherit; }

/* Rendu markdown des réponses assistant (:deep car v-html injecte du HTML sans data-v) */
.msg.assistant :deep(.markdown-content p),
.msg.assistant :deep(.markdown-content ul),
.msg.assistant :deep(.markdown-content ol),
.msg.assistant :deep(.markdown-content blockquote),
.msg.assistant :deep(.markdown-content pre),
.msg.assistant :deep(.markdown-content h1),
.msg.assistant :deep(.markdown-content h2),
.msg.assistant :deep(.markdown-content h3),
.msg.assistant :deep(.markdown-content h4),
.msg.assistant :deep(.markdown-content h5),
.msg.assistant :deep(.markdown-content h6),
.msg.assistant :deep(.markdown-content hr),
.msg.assistant :deep(.markdown-content table),
.msg.assistant :deep(.markdown-content dl) {
  margin: 0.4rem 0;
}
.msg.assistant :deep(.markdown-content p:first-child),
.msg.assistant :deep(.markdown-content ul:first-child),
.msg.assistant :deep(.markdown-content ol:first-child),
.msg.assistant :deep(.markdown-content blockquote:first-child),
.msg.assistant :deep(.markdown-content pre:first-child),
.msg.assistant :deep(.markdown-content h1:first-child),
.msg.assistant :deep(.markdown-content h2:first-child),
.msg.assistant :deep(.markdown-content h3:first-child),
.msg.assistant :deep(.markdown-content h4:first-child),
.msg.assistant :deep(.markdown-content h5:first-child),
.msg.assistant :deep(.markdown-content h6:first-child),
.msg.assistant :deep(.markdown-content hr:first-child),
.msg.assistant :deep(.markdown-content table:first-child),
.msg.assistant :deep(.markdown-content dl:first-child) {
  margin-top: 0;
}
.msg.assistant :deep(.markdown-content p:last-child),
.msg.assistant :deep(.markdown-content ul:last-child),
.msg.assistant :deep(.markdown-content ol:last-child),
.msg.assistant :deep(.markdown-content blockquote:last-child),
.msg.assistant :deep(.markdown-content pre:last-child),
.msg.assistant :deep(.markdown-content h1:last-child),
.msg.assistant :deep(.markdown-content h2:last-child),
.msg.assistant :deep(.markdown-content h3:last-child),
.msg.assistant :deep(.markdown-content h4:last-child),
.msg.assistant :deep(.markdown-content h5:last-child),
.msg.assistant :deep(.markdown-content h6:last-child),
.msg.assistant :deep(.markdown-content hr:last-child),
.msg.assistant :deep(.markdown-content table:last-child),
.msg.assistant :deep(.markdown-content dl:last-child) {
  margin-bottom: 0;
}
.msg.assistant :deep(.markdown-content ul),
.msg.assistant :deep(.markdown-content ol) {
  padding-left: 1.5rem;
}
.msg.assistant :deep(.markdown-content li) {
  margin: 0.25rem 0;
}
.msg.assistant :deep(.markdown-content blockquote) {
  border-left: 4px solid var(--color-blockquote-border);
  padding-left: 0.8rem;
  color: var(--color-blockquote-text);
  font-style: italic;
}
.msg.assistant :deep(.markdown-content h1), .msg.assistant :deep(.markdown-content h2), .msg.assistant :deep(.markdown-content h3) {
  font-weight: 600;
  color: var(--color-heading);
}
.msg.assistant :deep(.markdown-content h1) { font-size: 1.3rem; }
.msg.assistant :deep(.markdown-content h2) { font-size: 1.2rem; }
.msg.assistant :deep(.markdown-content h3) { font-size: 1.1rem; }
.msg.assistant :deep(.markdown-content hr) {
  border: none;
  border-top: 1px solid var(--color-hr);
}

/* Blocs de code : fond sombre fixe, identique clair/sombre */
.msg.assistant :deep(.markdown-content pre) {
  background: var(--color-code-bg) !important;
  color: var(--color-code-text) !important;
  padding: 0.75rem 1rem !important;
  border-radius: 0 0 6px 6px !important;
  overflow-x: auto;
  margin: 0 !important;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 0.85rem;
  line-height: 1.5;
}
.msg.assistant :deep(.markdown-content pre code) {
  background: transparent !important;
  padding: 0 !important;
  font-size: inherit !important;
  color: inherit !important;
  font-family: inherit !important;
}

/* Barre d'en-tête du bloc de code */
.msg.assistant :deep(.code-wrapper) {
  position: relative;
  margin-top: 0;
}
.msg.assistant :deep(.code-header) {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0.4rem 0.75rem;
  background: var(--color-code-header-bg);
  border: 1px solid var(--color-code-header-border);
  border-bottom: none;
  border-top-left-radius: 6px;
  border-top-right-radius: 6px;
  gap: 0.75rem;
}
.msg.assistant :deep(.code-lang) {
  font-size: 0.75rem;
  color: var(--color-code-lang);
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, monospace;
}

/* Bouton Copier : style sobre, pas en position absolue */
.msg.assistant :deep(.copy-btn) {
  display: inline-flex;
  align-items: center;
  font-size: 0.75rem;
  padding: 0.15rem 0.5rem;
  background: transparent;
  color: var(--color-code-lang);
  border: 1px solid var(--color-code-header-border);
  border-radius: 4px;
  cursor: pointer;
  transition: all 0.15s;
  margin-left: auto;
}
.msg.assistant :deep(.copy-btn:hover) {
  border-color: var(--color-code-lang);
  color: var(--color-code-text);
  background: var(--color-bg-panel);
}
.msg.assistant :deep(.copy-btn:focus) {
  outline: 2px solid #58a6ff;
  outline-offset: 1px;
}
</style>