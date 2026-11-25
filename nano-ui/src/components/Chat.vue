<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick, watch } from "vue";
import { invoke, Channel } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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
interface ProjectConfig {
  root_path: string | null;
  exclusions: string[];
}
interface ToolTrace {
  tool: string;
  arguments: string;
  result: string;
  isError: boolean;
  fatal: boolean;
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

/* Traces des outils agent, indexées par message (non persistées) */
const toolTraces = ref<Record<number, ToolTrace[]>>({});
const activeTraceIdx = ref<number | null>(null);
let unlistenToolTraces: (() => void) | null = null;

/* Multi-conversation */
const conversations = ref<ConversationListItem[]>([]);
const currentConversationId = ref<string | null>(null);
const currentConversationTitle = ref("");
const isPasswordModalOpen = ref(false);
const passwordInput = ref("");
const pendingConversationId = ref<string | null>(null); // id à déverrouiller
const editingConversationId = ref<string | null>(null); // id en cours de renommage
const editTitle = ref(""); // titre en cours d'édition

/* ---------- Paramètres de conversation ---------- */
const systemPrompt = ref(""); // prompt système de la conversation courante
const contextLimit = ref(20); // limite de contexte (nb messages en brut)
const isSettingsOpen = ref(false); // panneau paramètres ouvert ?
const useAgent = ref(false); // mode agent avec outils activé ?

/* ---------- Projet lié ---------- */
const isProjectOpen = ref(false); // panneau projet ouvert ?
const project = ref<ProjectConfig>({ root_path: null, exclusions: [] });
const exclusionInput = ref("");
const projectError = ref<string | null>(null);

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
    toolTraces.value = {};
    model.value = models.value[0] ?? "";
    // Charge les paramètres de la conversation (prompt système + contexte)
    const settings = await invoke<{ system_prompt: string; context_limit: number }>("get_settings", { id });
    systemPrompt.value = settings.system_prompt;
    contextLimit.value = settings.context_limit ?? 20;
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
    toolTraces.value = {};
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
        system_prompt: systemPrompt.value,
        context_limit: contextLimit.value,
        messages: messages.value,
      },
    });
    await loadConversations();
    error.value = null;
  } catch (e) {
    error.value = String(e);
  }
}

/* Sauvegarde différée après le dernier token : la résolution de l'invoke peut arriver
   avant que tous les messages du canal Tauri soient appliqués côté Vue. */
let saveTimer: ReturnType<typeof setTimeout> | null = null;
let saveChain: Promise<void> = Promise.resolve();

function queueSave(): Promise<void> {
  saveChain = saveChain.then(() => saveCurrentConversation());
  return saveChain;
}

watch(
  messages,
  () => {
    if (!currentConversationId.value || busy.value) return;
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
      saveTimer = null;
      void queueSave();
    }, 400);
  },
  { deep: true },
);

async function saveSettings() {
  if (!currentConversationId.value) return;
  try {
    await invoke("save_settings", {
      id: currentConversationId.value,
      settings: {
        system_prompt: systemPrompt.value,
        context_limit: contextLimit.value,
      },
    });
    error.value = null;
  } catch (e) {
    error.value = String(e);
  }
}

/* ---------- Projet lié ---------- */
async function loadProject() {
  try {
    project.value = await invoke<ProjectConfig>("project_get");
    projectError.value = null;
  } catch (e) {
    projectError.value = String(e);
  }
}

async function pickProjectFolder() {
  if (busy.value) return;
  try {
    project.value = await invoke<ProjectConfig>("project_pick_folder");
    projectError.value = null;
  } catch (e) {
    projectError.value = String(e);
  }
}

async function clearProject() {
  if (busy.value) return;
  if (!confirm("Délier le dossier de projet ? L'agent n'aura plus accès aux fichiers.")) return;
  try {
    project.value = await invoke<ProjectConfig>("project_clear");
    projectError.value = null;
  } catch (e) {
    projectError.value = String(e);
  }
}

async function addExclusion() {
  const path = exclusionInput.value.trim();
  if (!path || !project.value.root_path || busy.value) return;
  try {
    project.value = await invoke<ProjectConfig>("project_add_exclusion", { path });
    exclusionInput.value = "";
    projectError.value = null;
  } catch (e) {
    projectError.value = String(e);
  }
}

async function removeExclusion(path: string) {
  if (busy.value) return;
  try {
    project.value = await invoke<ProjectConfig>("project_remove_exclusion", { path });
    projectError.value = null;
  } catch (e) {
    projectError.value = String(e);
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
      toolTraces.value = {};
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
  unlistenToolTraces = await listen<ToolTrace>("agent-tool-trace", (event) => {
    const idx = activeTraceIdx.value;
    if (idx === null) return;
    const traces = (toolTraces.value[idx] ??= []);
    traces.push(event.payload);
    nextTick(() => {
      list.value?.scrollTo({ top: list.value.scrollHeight });
    });
  });
  try {
    const status = await invoke<OllamaStatus>("get_ollama_status");
    models.value = status.models.map((m) => m.name);
    model.value = models.value[0] ?? "";
  } catch (e) {
    error.value = String(e);
  }
  await loadProject();
  await loadConversations();
});

onUnmounted(() => {
  unlistenToolTraces?.();
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
  activeTraceIdx.value = idx;
  toolTraces.value[idx] = [];

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
    // Valeur par défaut 20 si le champ est vide ou invalide (0, NaN, null).
    const rawLimit = contextLimit.value;
    const limit =
      rawLimit != null && !Number.isNaN(rawLimit) && rawLimit > 0
        ? rawLimit
        : 20;
    await invoke(
      useAgent.value ? "chat_agent" : "chat",
      {
        model: model.value,
        messages: history,
        systemPrompt: systemPrompt.value,
        contextLimit: limit,
        onToken,
      },
    );
    // Met à jour le titre affiché si un premier message utilisateur existe
    const firstUser = messages.value.find((m) => m.role === "user");
    if (firstUser) {
      const t = firstUser.content.trim();
      if (t) {
        currentConversationTitle.value = t.length > 40 ? t.slice(0, 40) + "…" : t;
      }
    }
  } catch (e) {
    error.value = String(e);
    if (messages.value[idx].content === "") messages.value.splice(idx, 1);
  } finally {
    busy.value = false;
    stopping.value = false;
    awaitingFirstToken.value = false;
    currentAssistantIdx.value = null;
    activeTraceIdx.value = null;
    // Sauvegarde automatique dans SQLite après réception complète
    await queueSave();
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
          <button
            class="btn-agent"
            :class="{ active: useAgent }"
            @click="useAgent = !useAgent"
            :aria-pressed="useAgent"
            :title="useAgent ? 'Mode agent avec outils activé' : 'Mode agent avec outils désactivé'"
          >
            Mode agent
          </button>
          <button :disabled="busy" @click="createConversation">
            Nouvelle conversation
          </button>
          <button
            class="btn-project"
            :class="{ active: isProjectOpen }"
            @click="isProjectOpen = !isProjectOpen"
            :aria-pressed="isProjectOpen"
            :title="project.root_path ? `Projet : ${project.root_path}` : 'Aucun dossier de projet lié'"
          >
            Projet<span v-if="project.root_path" class="project-dot"></span>
          </button>
          <button
            class="btn-settings"
            @click="isSettingsOpen = !isSettingsOpen"
            :aria-label="isSettingsOpen ? 'Fermer les paramètres' : 'Ouvrir les paramètres'"
          >
            Paramètres
          </button>
        </div>
      </header>

      <!-- Panneau paramètres de la conversation -->
      <div v-if="isSettingsOpen && currentConversationId" class="settings-panel">
        <div class="settings-section">
          <label class="settings-label">Prompt système</label>
          <textarea
            v-model="systemPrompt"
            rows="3"
            placeholder="Instruction de rôle pour ce modèle (ex: Tu es un expert développeur...)"
          ></textarea>
          <p class="settings-hint">Envoyé au modèle au début de chaque réponse de cette conversation.</p>
        </div>
        <div class="settings-section">
          <label class="settings-label">Contexte (messages en brut)</label>
          <input
            type="number"
            v-model.number="contextLimit"
            min="1"
            max="200"
          />
          <p class="settings-hint">Nombre de derniers messages conservés en brut. Au-delà, les anciens sont résumés (contexte à froid).</p>
        </div>
        <button class="btn-settings-save" @click="saveSettings">Enregistrer</button>
        <button class="btn-settings-close" @click="isSettingsOpen = false">Fermer</button>
      </div>

      <!-- Panneau du dossier de projet lié (global, indépendant des conversations) -->
      <div v-if="isProjectOpen" class="settings-panel project-panel">
        <div class="settings-section">
          <label class="settings-label">Dossier de projet</label>
          <code v-if="project.root_path" class="project-path">{{ project.root_path }}</code>
          <p v-else class="settings-hint">Aucun dossier lié. En mode agent, les outils refuseront tout accès aux fichiers.</p>
          <div class="project-actions">
            <button class="btn-settings-save" :disabled="busy" @click="pickProjectFolder">
              {{ project.root_path ? "Changer de dossier" : "Choisir un dossier" }}
            </button>
            <button v-if="project.root_path" class="btn-settings-close" :disabled="busy" @click="clearProject">
              Délier
            </button>
          </div>
          <p class="settings-hint">L'agent peut lire et modifier ces fichiers sans confirmation. Les chemins hors dossier sont refusés.</p>
        </div>
        <div class="settings-section">
          <label class="settings-label">Exclusions</label>
          <ul v-if="project.exclusions.length" class="exclusion-list">
            <li v-for="p in project.exclusions" :key="p">
              <code>{{ p }}</code>
              <button class="btn-delete" :disabled="busy" @click="removeExclusion(p)">Retirer</button>
            </li>
          </ul>
          <p v-else class="settings-hint">Aucune exclusion.</p>
          <div class="project-actions">
            <input
              v-model="exclusionInput"
              type="text"
              placeholder="chemin/relatif (fichier ou dossier)"
              :disabled="!project.root_path || busy"
              @keydown.enter.prevent="addExclusion"
            />
            <button
              class="btn-settings-close"
              :disabled="!exclusionInput.trim() || !project.root_path || busy"
              @click="addExclusion"
            >
              Exclure
            </button>
          </div>
          <p class="settings-hint">Un fichier ou dossier exclu devient inaccessible en lecture comme en écriture.</p>
        </div>
        <p v-if="projectError" class="error">{{ projectError }}</p>
        <button class="btn-settings-close" @click="isProjectOpen = false">Fermer</button>
      </div>

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
            <div v-if="m.role === 'assistant' && toolTraces[i]?.length" class="tool-traces">
              <details v-for="(trace, t) in toolTraces[i]" :key="t">
                <summary>
                  <span class="trace-icon" :class="{ failure: trace.isError || trace.fatal }">
                    {{ trace.isError || trace.fatal ? "✗" : "✓" }}
                  </span>
                  <span>{{ trace.tool }}</span>
                  <span v-if="trace.fatal" class="trace-tag">refus</span>
                </summary>
                <div class="trace-detail">
                  <div v-if="trace.arguments" class="trace-block">
                    <span class="trace-label">Arguments</span>
                    <pre>{{ trace.arguments }}</pre>
                  </div>
                  <div v-if="trace.result" class="trace-block">
                    <span class="trace-label">Résultat</span>
                    <pre>{{ trace.result }}</pre>
                  </div>
                </div>
              </details>
            </div>
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

/* Traces des outils agent (texte brut, jamais de v-html) */
.tool-traces { margin-top: 0.5rem; display: flex; flex-direction: column; gap: 0.25rem; }
.tool-traces details { border: 1px solid var(--color-border); border-radius: 4px; background: var(--color-bg); font-size: 0.75rem; }
.tool-traces summary { cursor: pointer; padding: 0.25rem 0.5rem; display: flex; gap: 0.4rem; align-items: center; color: var(--color-text-muted); }
.trace-icon { color: #3fb950; font-weight: 700; }
.trace-icon.failure { color: var(--color-error); }
.trace-tag { border: 1px solid var(--color-error); border-radius: 3px; padding: 0 0.25rem; color: var(--color-error); }
.trace-detail { padding: 0 0.5rem 0.5rem; border-top: 1px solid var(--color-border); max-height: 16rem; overflow-y: auto; }
.trace-block { margin-top: 0.4rem; }
.trace-label { display: block; color: var(--color-text-muted); font-weight: 600; }
.trace-detail pre { margin: 0.2rem 0 0; white-space: pre-wrap; overflow-wrap: anywhere; font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, monospace; color: var(--color-text); }
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

/* Bouton paramètres dans le header */
.btn-settings {
  background: none;
  border: 1px solid var(--color-border);
  color: var(--color-text);
  cursor: pointer;
  padding: 0.25rem 0.5rem;
  border-radius: 4px;
  font-size: 1rem;
  margin-left: 0.5rem;
}
.btn-settings:hover { background: var(--color-bg); }

/* Bouton projet dans le header */
.btn-project {
  position: relative;
  background: none;
  border: 1px solid var(--color-border);
  color: var(--color-text);
  cursor: pointer;
  padding: 0.25rem 0.5rem;
  border-radius: 4px;
  font-size: 0.85rem;
}
.btn-project:hover { background: var(--color-bg); }
.btn-project.active {
  background: var(--color-user-msg);
  border-color: var(--color-user-msg);
  font-weight: 600;
}
.project-dot {
  display: inline-block;
  width: 6px;
  height: 6px;
  margin-left: 0.35rem;
  border-radius: 50%;
  background: #3fb950;
  vertical-align: middle;
}

/* Bouton mode agent */
.btn-agent {
  background: none;
  border: 1px solid var(--color-border);
  color: var(--color-text);
  cursor: pointer;
  padding: 0.25rem 0.5rem;
  border-radius: 4px;
  font-size: 0.85rem;
  margin-left: 0.5rem;
}
.btn-agent:hover { background: var(--color-bg); }
.btn-agent.active {
  background: var(--color-user-msg);
  border-color: var(--color-user-msg);
  font-weight: 600;
}

/* Panneau paramètres */
.settings-panel {
  padding: 1rem;
  border: 1px solid var(--color-border);
  background: var(--color-bg-panel);
  margin: 0.5rem 1rem;
  border-radius: 6px;
  display: flex;
  flex-direction: column;
  gap: 1rem;
  animation: fadeIn 0.2s ease;
}
@keyframes fadeIn {
  from { opacity: 0; transform: translateY(-4px); }
  to { opacity: 1; transform: translateY(0); }
}
.settings-section {
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
}
.settings-label {
  font-size: 0.85rem;
  font-weight: 600;
  color: var(--color-text-muted);
}
.settings-panel textarea,
.settings-panel input[type="number"] {
  font: inherit;
  padding: 0.5rem;
  border: 1px solid var(--color-border);
  border-radius: 4px;
  background: var(--color-bg);
  color: var(--color-text);
}
.settings-panel textarea:focus,
.settings-panel input[type="number"]:focus {
  outline: none;
  border-color: #58a6ff;
}
.settings-hint {
  font-size: 0.75rem;
  color: var(--color-text-muted);
  margin: 0;
}
.btn-settings-save {
  align-self: flex-start;
  background: var(--color-user-msg);
  border: 1px solid var(--color-user-msg);
  color: var(--color-text);
  padding: 0.25rem 0.75rem;
  border-radius: 4px;
  cursor: pointer;
  font-weight: 600;
}
.btn-settings-save:hover { background: var(--color-border-hover); }
.btn-settings-close {
  align-self: flex-start;
  background: none;
  border: 1px solid var(--color-border);
  color: var(--color-text);
  padding: 0.25rem 0.75rem;
  border-radius: 4px;
  cursor: pointer;
}
.btn-settings-close:hover { background: var(--color-bg); }
.btn-settings-save:disabled,
.btn-settings-close:disabled { opacity: 0.5; cursor: not-allowed; }

/* Panneau projet */
.project-panel {
  max-height: 60vh;
  overflow-y: auto;
}
.project-path {
  font-size: 0.8rem;
  color: var(--color-text);
  word-break: break-all;
  background: var(--color-bg);
  padding: 0.35rem 0.5rem;
  border-radius: 4px;
}
.project-actions {
  display: flex;
  gap: 0.5rem;
  align-items: center;
  flex-wrap: wrap;
}
.settings-panel input[type="text"] {
  flex: 1;
  min-width: 160px;
  font: inherit;
  padding: 0.5rem;
  border: 1px solid var(--color-border);
  border-radius: 4px;
  background: var(--color-bg);
  color: var(--color-text);
}
.settings-panel input[type="text"]:focus {
  outline: none;
  border-color: #58a6ff;
}
.exclusion-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  max-height: 12rem;
  overflow-y: auto;
}
.exclusion-list li {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
  background: var(--color-bg);
  border-radius: 4px;
  padding: 0.25rem 0.5rem;
}
.exclusion-list code {
  font-size: 0.8rem;
  word-break: break-all;
}
</style>
