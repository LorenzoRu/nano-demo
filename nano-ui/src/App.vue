<script setup lang="ts">
import { ref } from "vue";
import Chat from "./components/Chat.vue";
import OllamaDashboard from "./components/OllamaDashboard.vue";

const tab = ref<"chat" | "dashboard">("chat");
</script>

<template>
  <div class="app">
    <nav>
      <button :class="{ active: tab === 'chat' }" @click="tab = 'chat'">Chat</button>
      <button :class="{ active: tab === 'dashboard' }" @click="tab = 'dashboard'">
        Tableau de bord
      </button>
    </nav>
    <div class="content">
      <Chat v-show="tab === 'chat'" />
      <div v-if="tab === 'dashboard'" class="scroll"><OllamaDashboard /></div>
    </div>
  </div>
</template>

<style>
:root { color-scheme: light dark; font-family: system-ui, sans-serif; }
html, body, #app { height: 100%; margin: 0; }
</style>

<style scoped>
.app { display: flex; flex-direction: column; height: 100%; }
nav { display: flex; gap: 0.5rem; padding: 0.6rem 1rem; border-bottom: 1px solid #8883; }
nav button.active { font-weight: 600; text-decoration: underline; }
.content { flex: 1; min-height: 0; }
.content > * { height: 100%; }
.scroll { overflow-y: auto; }
</style>