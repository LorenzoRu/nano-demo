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
  input.value = "";
  messages.value.push({ role: "user", content: text });
  const history = [...messages.value]; // historique envoyé, sans la réponse vide
  const idx = messages.value.push({ role: "assistant", content: "" }) - 1;

  const onToken = new Channel<string>();
  onToken.onmessage = (token) => {
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
  }
}

async function stop() {
  stopping.value = true;
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
      <button :disabled="!busy" @click="stop" class="stop">Stop</button>
    </header>

    <div ref="list" class="messages">
      <p v-if="messages.length === 0" class="hint">Pose une question à ton modèle local.</p>
      <div v-for="(m, i) in messages" :key="i" class="msg" :class="m.role">{{ m.content }}</div>
    </div>

    <p v-if="error" class="error">{{ error }}</p>

    <form @submit.prevent="send">
      <textarea
        v-model="input"
        rows="2"
        placeholder="Écris ton message (Entrée pour envoyer, Maj+Entrée pour un saut de ligne)"
        @keydown.enter.exact.prevent="send"
      />
      <button type="submit" :disabled="busy || !input.trim() || !model">Envoyer</button>
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
button.stop { background: #d73a49; color: #fff; border: none; border-radius: 6px; padding: 0.25rem 0.75rem; cursor: pointer; }
button.stop:disabled { opacity: 0.5; cursor: not-allowed; }
.hint { opacity: 0.6; text-align: center; margin-top: 2rem; }
.error { color: #d73a49; margin: 0.4rem 0; }
form { display: flex; gap: 0.6rem; align-items: flex-end; margin-top: 0.5rem; }
textarea { flex: 1; resize: none; padding: 0.55rem; border-radius: 8px; border: 1px solid #8884; font: inherit; }
</style>