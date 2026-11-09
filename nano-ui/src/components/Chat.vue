<script setup lang="ts">
import { ref, onMounted, nextTick } from "vue";
import { invoke, Channel } from "@tauri-apps/api/core";

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

onMounted(async () => {
  try {
    const status = await invoke<OllamaStatus>("get_ollama_status");
    models.value = status.models.map((m) => m.name);
    model.value = models.value[0] ?? "";
  } catch (e) {
    error.value = String(e);
  }
});

async function scrollDown() {
  await nextTick();
  list.value?.scrollTo({ top: list.value.scrollHeight });
}

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
    scrollDown();
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

    <div ref="list" class="messages">
      <p v-if="messages.length === 0" class="hint">Pose une question à ton modèle local.</p>
      <div v-for="(m, i) in messages" :key="i" class="msg" :class="m.role">
        {{ m.content }}
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

<style scoped>
.chat { display: flex; flex-direction: column; height: 100%; max-width: 820px; margin: 0 auto; padding: 1rem 1.5rem; box-sizing: border-box; }
header { display: flex; gap: 0.75rem; justify-content: space-between; margin-bottom: 0.75rem; }
.messages { flex: 1; min-height: 0; overflow-y: auto; display: flex; flex-direction: column; gap: 0.6rem; padding: 0.5rem 0; }
.msg { white-space: pre-wrap; padding: 0.6rem 0.9rem; border-radius: 10px; max-width: 85%; line-height: 1.45; }
.msg.user { align-self: flex-end; background: #3b82f633; }
.msg.assistant { align-self: flex-start; background: #8882; }
.stopped-hint { display: block; font-size: 0.7rem; color: #6b7280; margin-top: 0.25rem; }
.typing-indicator { display: flex; gap: 4px; align-items: center; margin: 0.5rem 0 0; font-size: 0.9rem; color: #666; align-self: flex-start; }
.typing-indicator span { width: 8px; height: 8px; border-radius: 50%; background: #9ca3af; animation: typing 1.4s infinite ease-in-out both; }
.typing-indicator span:nth-child(1) { animation-delay: 0s; }
.typing-indicator span:nth-child(2) { animation-delay: 0.2s; }
.typing-indicator span:nth-child(3) { animation-delay: 0.4s; }
@keyframes typing { 0%, 100% { opacity: 0.4; transform: scale(1); } 50% { opacity: 1; transform: scale(1.2); } }
button.stop { background: #d73a49; color: #fff; border: none; border-radius: 6px; padding: 0.25rem 0.75rem; cursor: pointer; }
button.stop:disabled { opacity: 0.5; cursor: not-allowed; }
.hint { opacity: 0.6; text-align: center; margin-top: 2rem; }
.error { color: #d73a49; margin: 0.4rem 0; }
form { display: flex; gap: 0.6rem; align-items: flex-end; margin-top: 0.5rem; }
textarea { flex: 1; resize: none; padding: 0.55rem; border-radius: 8px; border: 1px solid #8884; font: inherit; }
</style>