<script setup lang="ts">
import { ref, onMounted, nextTick } from "vue";
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
interface ConversationListItem {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  size_bytes: number;
  has_password: boolean;
  size_alert: boolean;
}
interface Conversation {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  password_hash: string | null;
  size_bytes: number;
  messages: ChatMessage[];
}

/* ---------- État ---------- */
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

/* Multi-conversation */
const conversations = ref<ConversationListItem[]>([]);
const currentConversationId = ref<string | null>(null);
const currentConversationTitle = ref("");
const isPasswordModalOpen = ref(false);
const passwordInput = ref("");
const pendingConversationId = ref<string | null>(null); // id à déverrouiller
const editingConversationId = ref<string | null>(null); // id en cours de renommage
const editTitle = ref(""); // titre en cours d'édition

/* ---------- Formatage ---------- */
function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} o`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} Ko`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} Mo`;
  // Au-delà de 1 Go on affiche en Go (unité max pour lisibilité)
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} Go`;
}

/* ---------- Copie presse-papiers ---------- */
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

/* ---------- Markdown ---------- */
function renderMarkdown(content: string): string {
  const html = marked.parse(content, { breaks: true, gfm: true }) as string;
  const div = document.createElement("div");
  div.innerHTML = DOMPurify.sanitize(html);

  div.querySelectorAll("pre > code").forEach((block) => {
    const pre = block.parentElement as HTMLElement;

    const match = /language-([\w+-]+)/.exec(block.className);
    const language = match?.[1] ?? "";

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
  btn.textContent = "Copié";
  setTimeout(() => {
    btn.textContent = "Copier";
  }, 2000);
}

/* ---------- Conversations ---------- */
async function loadConversations() {
  try {
    const list = await invoke<ConversationListItem[]>("conversation_list");
    conversations.value = list;
    // Ouvre la première conversation si aucune n'est sélectionnée
    if (currentConversationId.value === null && list.length > 0) {
      await openConversation(list[0].id);
    }
  } catch (e) {
    error.value = String(e);
  }
}

async function openConversation(id: string) {
  if (busy.value) return;
  const target = conversations.value.find((c) => c.id === id);
  if (!target) return;

  // Si protégée par mot de passe, on demande le mot de passe avant de charger
  if (target.has_password) {
    pendingConversationId.value = id;
    isPasswordModalOpen.value = true;
    passwordInput.value = "";
    return;
  }

  await loadConversationMessages(id);
}

async function loadConversationMessages(id: string) {
  try {
    const conv = await invoke<Conversation>("conversation_get", { id, password: null });
    currentConversationId.value = id;
    currentConversationTitle.value = conv.title;
    messages.value = conv.messages;
    model.value = models.value[0] ?? "";
    error.value = null;
  } catch (e) {
    error.value = String(e);
  }
}

async function confirmPassword() {
  const id = pendingConversationId.value;
  if (!id) return;
  try {
    const conv = await invoke<Conversation>("conversation_get", { id, password: passwordInput.value });
    currentConversationId.value = id;
    messages.value = conv.messages;
    model.value = models.value[0] ?? "";
    isPasswordModalOpen.value = false;
    pendingConversationId.value = null;
    passwordInput.value = "";
    error.value = null;
  } catch (e) {
    error.value = String(e);
    passwordInput.value = "";
  }
}

async function createConversation() {
  if (busy.value) return;
  try {
    const conv = await invoke<Conversation>("conversation_new", { title: null });
    currentConversationTitle.value = conv.title;
    await loadConversations();
    await loadConversationMessages(conv.id);
    error.value = null;
  } catch (e) {
    error.value = String(e);
  }
}

async function saveCurrentConversation() {
  if (!currentConversationId.value || busy.value) return;
  // Titre automatique si vide ou par défaut, depuis le premier message utilisateur
  let title = "";
  for (const m of messages.value) {
    if (m.role === "user") {
      const t = m.content.trim();
      if (t) {
        title = t.length > 40 ? t.slice(0, 40) + "…" : t;
        break;
      }
    }
  }
  if (!title) title = "Nouvelle conversation";

  try {
    const size_bytes = messages.value.reduce((acc, m) => acc + m.content.length, 0);
    await invoke("conversation_save", {
      conv: {
        id: currentConversationId.value,
        title,
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
        password_hash: null,
        size_bytes,
        messages: messages.value,
      },
    });
    await loadConversations();
    error.value = null;
  } catch (e) {
    error.value = String(e);
  }
}

async function deleteConversation(id: string) {
  if (!confirm("Supprimer cette conversation ?")) return;
  try {
    await invoke("conversation_delete", { id });
    await loadConversations();
    if (currentConversationId.value === id) {
      currentConversationId.value = null;
      messages.value = [];
      if (conversations.value.length > 0) {
        await openConversation(conversations.value[0].id);
      }
    }
  } catch (e) {
    error.value = String(e);
  }
}

async function startRename(id: string, currentTitle: string) {
  editingConversationId.value = id;
  editTitle.value = currentTitle;
  // Focus sur l'input après le prochain rendu
  nextTick(() => {
    const input = document.querySelector(`input.edit-title[data-id="${id}"]`) as HTMLInputElement | null;
    input?.focus();
    input?.select();
  });
}

async function saveRename(id: string) {
  const newTitle = editTitle.value.trim();
  if (!newTitle || newTitle === conversations.value.find((c) => c.id === id)?.title) {
    cancelRename();
    return;
  }
  try {
    await invoke("conversation_rename", { id, title: newTitle });
    await loadConversations();
    if (currentConversationId.value === id) {
      currentConversationTitle.value = newTitle;
    }
  } catch (e) {
    error.value = String(e);
  } finally {
    cancelRename();
  }
}

function cancelRename() {
  editingConversationId.value = null;
  editTitle.value = "";
}

/* ---------- Initialisation ---------- */
onMounted(async () => {
  try {
    const status = await invoke<OllamaStatus>("get_ollama_status");
    models.value = status.models.map((m) => m.name);
    model.value = models.value[0] ?? "";
  } catch (e) {
    error.value = String(e);
  }
  await loadConversations();
});

/* ---------- Envoi / arrêt ---------- */
async function send() {
  const text = input.value.trim();
  if (!text || busy.value || !model.value) return;

  // Crée une conversation si aucune n'est ouverte
  if (!currentConversationId.value) {
    const conv = await invoke<Conversation>("conversation_new", { title: null });
    currentConversationId.value = conv.id;
    currentConversationTitle.value = conv.title;
    await loadConversations();
  }

  error.value = null;
  stoppedByUserIdx.value = null;
  awaitingFirstToken.value = true;
  input.value = "";
  messages.value.push({ role: "user", content: text });
  const history = [...messages.value];
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
    // Met à jour le titre affiché si un premier message utilisateur existe
    const firstUser = messages.value.find((m) => m.role === "user");
    if (firstUser) {
      const t = firstUser.content.trim();
      if (t) {
        currentConversationTitle.value = t.length > 40 ? t.slice(0, 40) + "…" : t;
      }
    }
  } catch (e) {
    if (!stopping.value) {
      error.value = String(e);
    }
    if (messages.value[idx].content === "") messages.value.splice(idx, 1);
  } finally {
    busy.value = false;
    stopping.value = false;
    awaitingFirstToken.value = false;
    currentAssistantIdx.value = null;
    // Sauvegarde automatique dans SQLite après réception complète (busy est déjà false)
    await saveCurrentConversation();
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
    <!-- Sidebar : liste des conversations -->
    <aside class="sidebar">
      <div class="sidebar-header">
        <h2>Conversations</h2>
        <button class="btn-new" @click="createConversation" :disabled="busy">+ Nouvelle</button>
      </div>
      <ul class="sidebar-list">
        <li v-for="c in conversations" :key="c.id" class="sidebar-item" :class="{ active: currentConversationId === c.id }">
          <button
            @click="openConversation(c.id)"
            class="sidebar-item-btn"
            :title="c.title"
          >
            <span
              v-if="editingConversationId !== c.id"
              class="item-title"
            >
              {{ c.title }}
            </span>
            <input
              v-else
              type="text"
              class="item-title edit-title"
              v-model="editTitle"
              :data-id="c.id"
              @keyup.enter="saveRename(c.id)"
              @keyup.esc="cancelRename"
            />
            <span class="item-size">{{ formatSize(c.size_bytes) }}</span>
            <span v-if="c.has_password" class="item-lock">Verrou</span>
          </button>
          <div class="item-actions">
            <button
              v-if="editingConversationId !== c.id"
              class="btn-rename"
              @click.stop="startRename(c.id, c.title)"
            >
              Renommer
            </button>
            <button
              v-if="editingConversationId === c.id"
              class="btn-save"
              @click.stop="saveRename(c.id)"
            >
              Enregistrer
            </button>
            <button
              v-if="editingConversationId === c.id"
              class="btn-cancel"
              @click.stop="cancelRename"
            >
              Annuler
            </button>
            <button class="btn-delete" @click.stop="deleteConversation(c.id)">Supprimer</button>
          </div>
        </li>
        <li v-if="conversations.length === 0" class="sidebar-empty">
          Aucune conversation. Cliquez sur "+ Nouvelle".
        </li>
      </ul>
    </aside>

    <!-- Zone de chat principale -->
    <main class="chat-main">
      <header>
        <select v-model="model" :disabled="busy || models.length === 0">
          <option v-if="models.length === 0" value="">Aucun modèle</option>
          <option v-for="m in models" :key="m" :value="m">{{ m }}</option>
        </select>
        <div class="conv-title" :title="currentConversationTitle || ''">
          {{ currentConversationTitle || "Nouvelle conversation" }}
        </div>
        <div class="header-right">
          <span v-if="currentConversationId" class="conv-size">
            {{ formatSize(messages.reduce((acc: number, m: ChatMessage) => acc + m.content.length, 0)) }}
          </span>
          <button :disabled="busy" @click="createConversation">
            Nouvelle conversation
          </button>
        </div>
      </header>

      <div class="chat-bottom" :class="{ empty: messages.length === 0 }">
        <div ref="list" class="messages" @click="onMessageClick">
          <p v-if="messages.length === 0 && currentConversationId" class="hint">
            Pose une question à ton modèle local.
          </p>
          <div
            v-for="(m, i) in messages"
            :key="i"
            class="msg"
            :class="m.role"
          >
            <div v-if="m.role === 'assistant'" class="markdown-content" v-html="renderMarkdown(m.content)"></div>
            <div v-else>{{ m.content }}</div>
            <span
              v-if="m.role === 'assistant' && i === currentAssistantIdx && busy && awaitingFirstToken"
              class="typing-indicator"
            >
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
          <button type="submit" v-if="!busy" :disabled="!input.trim() || !model">
            Envoyer
          </button>
          <button v-else @click="stop" class="stop">Stop</button>
        </form>
      </div>
    </main>

    <!-- Modal mot de passe -->
    <div v-if="isPasswordModalOpen" class="modal-overlay" @click.self="isPasswordModalOpen = false">
      <div class="modal">
        <h3>Mot de passe requis</h3>
        <p>Cette conversation est protégée.</p>
        <input
          v-model="passwordInput"
          type="password"
          placeholder="Entrez le mot de passe"
          @keydown.enter="confirmPassword"
        />
        <div class="modal-actions">
          <button @click="isPasswordModalOpen = false; pendingConversationId = null">Annuler</button>
          <button @click="confirmPassword" :disabled="!passwordInput.trim()">Déverrouiller</button>
        </div>
      </div>
    </div>
  </section>
</template>

<style>
/* Thème highlight.js sombre cohérent avec le fond #0d1117 */
@import "highlight.js/styles/github-dark.css";
</style>

<style scoped>
.chat {
  display: flex;
  flex-direction: column;
  min-height: 0;
  max-width: none;
  margin: 0;
  padding: 0;
  box-sizing: border-box;
}

/* Layout : sidebar + main */
@media (min-width: 768px) {
  .chat {
    flex-direction: row;
  }
  .sidebar {
    width: 340px;
    border-right: 1px solid var(--color-border);
    flex-shrink: 0;
  }
  .chat-main {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
    margin: 0;
    padding: 0;
  }
}

@media (max-width: 767px) {
  .sidebar {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 280px;
    border-right: 1px solid var(--color-border);
    background: var(--color-bg-panel);
    z-index: 10;
    transform: translateX(-100%);
    transition: transform 0.2s;
  }
  .sidebar.open { transform: translateX(0); }
  .chat-main { padding: 1rem 1.5rem; }
}

/* Sidebar */
.sidebar {
  display: flex;
  flex-direction: column;
  flex: none;
  overflow-y: auto;
  background: var(--color-bg-panel);
}
.sidebar-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0.75rem 1rem;
  border-bottom: 1px solid var(--color-border);
}
.sidebar-header h2 {
  margin: 0;
  font-size: 1rem;
  color: var(--color-text);
}
.btn-new {
  background: none;
  border: 1px solid var(--color-border);
  color: var(--color-text);
  padding: 0.25rem 0.5rem;
  border-radius: 4px;
  cursor: pointer;
  font-size: 0.85rem;
}
.btn-new:hover { background: var(--color-bg); }
.btn-new:disabled { opacity: 0.5; cursor: not-allowed; }

.sidebar-list {
  list-style: none;
  margin: 0;
  padding: 0;
  overflow-y: auto;
  flex: 1;
}
.sidebar-item { border-bottom: 1px solid var(--color-border); }
.sidebar-item.active { background: var(--color-user-msg); }
.sidebar-item-btn {
  width: 100%;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0.6rem 1rem;
  background: none;
  border: none;
  color: var(--color-text);
  cursor: pointer;
  text-align: left;
}
.sidebar-item-btn:hover { background: var(--color-bg); }
.item-title { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.item-size { margin-left: 0.5rem; font-size: 0.75rem; color: var(--color-text-muted); }
.item-lock { margin-left: 0.5rem; font-size: 0.8rem; }
.item-actions { display: flex; gap: 0.25rem; opacity: 0; }
.sidebar-item:hover .item-actions { opacity: 1; }
.btn-rename, .btn-delete {
  background: none;
  border: none;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 0.15rem;
}
.btn-rename:hover { color: var(--color-text); }
.btn-delete:hover { color: var(--color-error); }
.sidebar-empty {
  padding: 2rem 1rem;
  text-align: center;
  color: var(--color-text-muted);
  font-size: 0.9rem;
}

/* Header du chat */
header {
  display: flex;
  gap: 0.75rem;
  align-items: center;
  padding: 0.75rem 1rem;
  border-bottom: 1px solid var(--color-border);
  background: var(--color-bg-panel);
}
.conv-title {
  flex: 1;
  font-weight: 600;
  color: var(--color-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  padding: 0 1rem;
}
.header-right {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 0.5rem;
}
.conv-size { font-size: 0.75rem; color: var(--color-text-muted); }

/* Conteneur messages + formulaire : centre quand vide, reprend le flux quand il y a des messages */
.chat-bottom {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.chat-bottom.empty {
  justify-content: center;
}
.chat-bottom.empty .messages {
  flex: none;
}
.messages {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
  padding: 0.5rem 0;
}
.msg {
  padding: 0.5rem 0.8rem;
  border-radius: 10px;
  max-width: 85%;
  line-height: 1.45;
}
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
form {
  flex: none;
  display: flex;
  gap: 0.6rem;
  align-items: flex-end;
  margin-top: 0.5rem;
  padding-bottom: 1rem;
}
textarea {
  flex: 1;
  resize: none;
  padding: 0.55rem;
  border-radius: 8px;
  border: 1px solid var(--color-textarea-border);
  font: inherit;
}

/* Markdown (inchangé) */
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
.msg.assistant :deep(.markdown-content h1),
.msg.assistant :deep(.markdown-content h2),
.msg.assistant :deep(.markdown-content h3) {
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

/* Blocs de code */
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

/* Bouton Copier */
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

/* Modal mot de passe */
.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
}
.modal {
  background: var(--color-bg-panel);
  border: 1px solid var(--color-border);
  border-radius: 8px;
  padding: 1.5rem;
  width: 320px;
  color: var(--color-text);
}
.modal h3 { margin: 0 0 0.5rem 0; font-size: 1.1rem; }
.modal p { margin: 0 0 1rem 0; font-size: 0.9rem; color: var(--color-text-muted); }
.modal input {
  width: 100%;
  padding: 0.5rem;
  border: 1px solid var(--color-border);
  border-radius: 6px;
  background: var(--color-bg);
  color: var(--color-text);
  margin-bottom: 1rem;
  font: inherit;
}
.modal-actions { display: flex; gap: 0.5rem; justify-content: flex-end; }
.modal-actions button {
  padding: 0.4rem 0.8rem;
  border: 1px solid var(--color-border);
  border-radius: 4px;
  background: none;
  color: var(--color-text);
  cursor: pointer;
}
.modal-actions button:hover { background: var(--color-bg); }

/* Input d'édition du titre dans la sidebar */
.item-title.edit-title {
  flex: 1;
  font: inherit;
  color: var(--color-text);
  background: transparent;
  border: 1px solid var(--color-border-hover);
  border-radius: 4px;
  padding: 0.2rem 0.4rem;
  outline: none;
}
.item-title.edit-title:focus { border-color: #58a6ff; }

.btn-save, .btn-cancel {
  background: none;
  border: none;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 0.15rem;
}
.btn-save:hover { color: var(--color-text); }
.btn-cancel:hover { color: var(--color-error); }
</style>
