<script lang="ts">
  // "Press a key…" control for the push-to-talk key.
  import { api, isRiskyKey, keyLabel } from "../lib/ipc";

  let { value = $bindable() }: { value: string } = $props();
  let recording = $state(false);
  let error = $state("");

  async function record() {
    recording = true;
    error = "";
    try {
      value = await api.startHotkeyCapture();
    } catch (e) {
      error = String(e);
    } finally {
      recording = false;
    }
  }

  function cancel() {
    api.cancelHotkeyCapture();
  }
</script>

<div class="recorder">
  <span class="key" class:recording>{recording ? "Press a key…" : keyLabel(value)}</span>
  {#if recording}
    <button onclick={cancel}>Cancel</button>
  {:else}
    <button onclick={record}>Change…</button>
  {/if}
</div>
{#if error}
  <p class="note error">{error}</p>
{:else if isRiskyKey(value)}
  <p class="note warn">
    {#if value === "AltGr" || value === "Alt"}
      Alt keys type characters such as @ and € on German layouts. A spare key like Right Ctrl works better.
    {:else}
      This key also types into the focused app each time you press it. A modifier or a spare F-key works better.
    {/if}
  </p>
{/if}

<style>
  .recorder {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .key {
    min-width: 130px;
    padding: 5px 10px;
    text-align: center;
    font-weight: 600;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .key.recording {
    border-color: var(--accent);
    color: var(--accent);
  }
  .note {
    margin: 4px 0 0;
    font-size: 12px;
  }
  .warn {
    color: var(--warn);
  }
  .error {
    color: var(--danger);
  }
</style>
