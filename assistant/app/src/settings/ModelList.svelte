<script lang="ts">
  // The catalog for one kind of model: pick the active one, download, delete.
  import { onMount } from "svelte";
  import { api, on } from "../lib/ipc";
  import type { ModelStatus } from "../lib/types";

  let {
    kind,
    selected = $bindable(),
    onchange,
  }: { kind: "stt" | "tts"; selected: string; onchange?: () => void } = $props();

  let models = $state<ModelStatus[]>([]);
  let progress = $state<Record<string, number>>({});
  let errors = $state<Record<string, string>>({});

  const rows = $derived(models.filter((m) => m.kind === kind));

  async function refresh() {
    models = await api.listModels();
    onchange?.();
  }

  onMount(() => {
    refresh();
    const unlisten = on.download((p) => {
      if (p.done) {
        delete progress[p.modelId];
        if (p.error && p.error !== "download cancelled") errors[p.modelId] = p.error;
        refresh();
      } else {
        progress[p.modelId] = p.total > 0 ? Math.round((p.bytes / p.total) * 100) : 0;
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  });

  function download(id: string) {
    delete errors[id];
    progress[id] = 0;
    // The result arrives through `download-progress`; this only catches early refusals.
    api.downloadModel(id).catch((e) => {
      delete progress[id];
      if (String(e) !== "download cancelled") errors[id] = String(e);
    });
  }

  async function remove(id: string) {
    await api.deleteModel(id).catch((e) => (errors[id] = String(e)));
    refresh();
  }

  export function isInstalled(id: string): boolean {
    return models.some((m) => m.id === id && m.installed);
  }
</script>

<table>
  <thead>
    <tr><th></th><th>Model</th><th>Disk</th><th>Memory</th><th></th></tr>
  </thead>
  <tbody>
    {#each rows as m (m.id)}
      {@const pct = progress[m.id]}
      <tr class:active={selected === m.id}>
        <td>
          <input type="radio" name="model-{kind}" value={m.id} bind:group={selected} disabled={!m.installed} />
        </td>
        <td>
          <div>{m.display}</div>
          <div class="id">{m.id}{selected === m.id && !m.installed ? " — selected, not downloaded" : ""}</div>
          {#if errors[m.id]}<div class="error">{errors[m.id]}</div>{/if}
        </td>
        <td>{m.sizeMb} MB</td>
        <td>~{m.ramMbEst} MB</td>
        <td class="actions">
          {#if pct !== undefined}
            <progress max="100" value={pct}></progress>
            <span class="pct">{pct}%</span>
            <button onclick={() => api.cancelDownload(m.id)}>Cancel</button>
          {:else if m.installed}
            {#if kind === "tts"}
              <button onclick={() => api.previewVoice(m.id).catch((e) => (errors[m.id] = String(e)))}>Preview</button>
            {/if}
            <button class="danger" onclick={() => remove(m.id)}>Delete</button>
          {:else}
            <button class="primary" onclick={() => download(m.id)}>Download</button>
          {/if}
        </td>
      </tr>
    {/each}
  </tbody>
</table>

<style>
  table {
    width: 100%;
    border-collapse: collapse;
  }
  th {
    text-align: left;
    font-weight: 500;
    font-size: 11px;
    color: var(--muted);
    padding: 2px 6px;
  }
  td {
    padding: 6px;
    border-top: 1px solid var(--border);
    vertical-align: middle;
  }
  tr.active td {
    background: rgba(94, 234, 212, 0.06);
  }
  .id {
    font-size: 11px;
    color: var(--muted);
  }
  .error {
    font-size: 11px;
    color: var(--danger);
  }
  .actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 6px;
    white-space: nowrap;
  }
  progress {
    width: 90px;
    accent-color: var(--accent);
  }
  .pct {
    font-size: 11px;
    width: 30px;
    text-align: right;
  }
</style>
