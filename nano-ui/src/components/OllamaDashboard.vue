<script setup lang="ts">
import { ref, onMounted, onUnmounted } from "vue";
import { invoke } from "@tauri-apps/api/core";

interface ModelInfo {
  name: string;
  size_bytes: number;
  parameter_size: string | null;
  quantization: string | null;
  modified_at: string;
}
interface OllamaStatus {
  connected: boolean;
  version: string | null;
  models: ModelInfo[];
  error: string | null;
}

const status = ref<OllamaStatus | null>(null);
let timer: number | undefined;

async function refresh() {
  try {
    status.value = await invoke<OllamaStatus>("get_ollama_status");
  } catch (e) {
    status.value = { connected: false, version: null, models: [], error: String(e) };
  }
}

onMounted(() => {
  refresh();
  timer = window.setInterval(refresh, 5000);
});
onUnmounted(() => window.clearInterval(timer));

function formatSize(bytes: number) {
  const gb = bytes / 1024 ** 3;
  return gb >= 1 ? `${gb.toFixed(1)} Go` : `${Math.round(bytes / 1024 ** 2)} Mo`;
}
function formatDate(iso: string) {
  return new Date(iso).toLocaleDateString("fr-FR");
}
</script>

<template>
  <section class="dashboard">
    <header>
      <h1>nano</h1>
      <span v-if="status" class="badge" :class="status.connected ? 'ok' : 'ko'">
        <span class="dot" />
        {{ status.connected ? "Ollama connecté" : "Ollama déconnecté" }}
        <small v-if="status.version">v{{ status.version }}</small>
      </span>
      <span v-else class="badge">Connexion…</span>
    </header>

    <p v-if="status && !status.connected" class="hint">
      Impossible de joindre Ollama sur localhost:11434. Vérifie qu'il est lancé
      (<code>ollama serve</code>).
    </p>

    <template v-if="status?.connected">
      <h2>Modèles installés ({{ status.models.length }})</h2>
      <p v-if="status.models.length === 0" class="hint">
        Aucun modèle. Installe-en un avec <code>ollama pull &lt;nom&gt;</code>.
      </p>
      <table v-else>
        <thead>
          <tr>
            <th>Nom</th>
            <th>Paramètres</th>
            <th>Quantif.</th>
            <th>Taille</th>
            <th>Modifié</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="m in status.models" :key="m.name">
            <td>{{ m.name }}</td>
            <td>{{ m.parameter_size ?? "—" }}</td>
            <td>{{ m.quantization ?? "—" }}</td>
            <td>{{ formatSize(m.size_bytes) }}</td>
            <td>{{ formatDate(m.modified_at) }}</td>
          </tr>
        </tbody>
      </table>
    </template>
  </section>
</template>

<style scoped>
.dashboard { width: 100%; max-width: none; padding: 2rem 1.5rem; box-sizing: border-box; }
header { display: flex; align-items: center; justify-content: space-between; margin-bottom: 1.5rem; }
h1 { margin: 0; font-size: 1.6rem; }
h2 { font-size: 1.05rem; margin: 1.5rem 0 0.75rem; }
.badge { display: inline-flex; align-items: center; gap: 0.5rem; padding: 0.35rem 0.8rem; border-radius: 999px; border: 1px solid var(--color-badge-border); font-size: 0.9rem; }
.badge small { opacity: 0.6; }
.dot { width: 0.6rem; height: 0.6rem; border-radius: 50%; background: var(--color-badge-dot); }
.ok .dot { background: var(--color-badge-dot-ok); }
.ko .dot { background: var(--color-badge-dot-ko); }
.hint { opacity: 0.7; }
table { width: 100%; border-collapse: collapse; font-size: 0.92rem; }
th, td { text-align: left; padding: 0.55rem 0.6rem; border-bottom: 1px solid var(--color-table-border); }
th { font-weight: 600; opacity: 0.7; }
code { background: var(--color-code-inline-bg); padding: 0.1rem 0.35rem; border-radius: 4px; }
</style>