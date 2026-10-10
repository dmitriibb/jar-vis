<script lang="ts">
  // First-run setup: home folder, opencode, permissions, key, models, a test.
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { api, keyLabel } from "../lib/ipc";
  import type { Config, ConnectionInfo, ModelStatus, Snapshot, WizardInfo } from "../lib/types";
  import KeyRecorder from "./KeyRecorder.svelte";
  import ModelList from "./ModelList.svelte";

  let { config = $bindable(), snap, ondone }: { config: Config; snap: Snapshot | null; ondone: () => void } = $props();

  const STEPS = ["Home folder", "opencode", "Permissions", "Push-to-talk key", "Speech models", "Try it", "Done"];

  let step = $state(0);
  // Saving a home folder creates it, so the choice is only applied on Next.
  let home = $state("");
  let info = $state<WizardInfo | null>(null);
  let connection = $state<ConnectionInfo | null>(null);
  let checking = $state(false);
  let models = $state<ModelStatus[]>([]);

  const isMac = $derived(info?.platform === "macos");
  const installed = (id: string) => models.some((m) => m.id === id && m.installed);
  const defaultsReady = $derived(installed(config.stt.model) && (!config.audio.tts_enabled || installed(config.tts.voice)));
  const last = $derived(snap?.history.at(-1));

  onMount(async () => {
    info = await api.wizardInfo();
    home = config.home || info.suggestedHome;
    if (!config.hotkey.key) config.hotkey.key = info.defaultHotkey;
    refreshModels();
  });

  async function refreshModels() {
    models = await api.listModels();
  }

  async function checkOpencode() {
    checking = true;
    connection = await api.connectionTest().catch((e) => ({ connected: false, info: null, binary: null, error: String(e) }));
    checking = false;
  }

  async function recheckPermissions() {
    info = await api.wizardInfo();
  }

  function downloadDefaults() {
    if (!installed(config.stt.model)) api.downloadModel(config.stt.model).catch(() => {});
    if (config.audio.tts_enabled && !installed(config.tts.voice)) api.downloadModel(config.tts.voice).catch(() => {});
  }

  async function browseHome() {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") home = picked;
  }

  function go(next: number) {
    if (STEPS[step] === "Home folder" && home.trim()) config.home = home.trim();
    step = next;
    if (STEPS[next] === "opencode" && !connection) checkOpencode();
    if (STEPS[next] === "Permissions") recheckPermissions();
    if (STEPS[next] === "Speech models") refreshModels();
  }

  async function finish() {
    if (!config.home && home.trim()) config.home = home.trim();
    config.wizard_done = true;
    // Give the auto-save in Settings a moment to land before the window closes.
    await new Promise((r) => setTimeout(r, 700));
    ondone();
    getCurrentWindow().close();
  }
</script>

<main>
  <aside>
    <h1>Welcome to Jar-vis</h1>
    <ol>
      {#each STEPS as name, i (name)}
        <li class:current={i === step} class:done={i < step}>{name}</li>
      {/each}
    </ol>
  </aside>

  <section>
    {#if STEPS[step] === "Home folder"}
      <h2>Choose the Jar-vis home folder</h2>
      <p>
        Jar-vis keeps its personalities, skills, pets and speech models in one folder. It is created with defaults if
        it is empty.
      </p>
      <div class="inline">
        <input type="text" bind:value={home} />
        <button onclick={browseHome}>Browse…</button>
      </div>
    {:else if STEPS[step] === "opencode"}
      <h2>Connect to opencode</h2>
      <p>
        Jar-vis is a voice front-end for opencode and uses its background service, the same one the opencode TUI
        uses. opencode must be installed and signed in to a model provider.
      </p>
      <div class="box">
        {#if checking}
          Checking…
        {:else if connection?.connected}
          <span class="ok">✔ Connected to opencode {connection.info?.version}</span>
          <div class="muted">{connection.info?.url}</div>
        {:else if connection}
          <span class="error">✘ {connection.error}</span>
          <div class="muted">
            {#if connection.binary}
              Found opencode at {connection.binary}.
            {:else}
              opencode was not found. Install it from opencode.ai, or set its path under Settings → OpenCode.
            {/if}
          </div>
        {/if}
        <div><button onclick={checkOpencode} disabled={checking}>Check again</button></div>
      </div>
      <p class="muted">
        If answers fail with an authentication error later, run <code>opencode auth login</code> in a terminal.
      </p>
    {:else if STEPS[step] === "Permissions"}
      <h2>Permissions</h2>
      <div class="box">
        <strong>Microphone</strong>
        <div class="muted">
          {#if isMac}
            macOS asks for microphone access the first time you hold the key. Choose Allow.
          {:else}
            Windows must allow desktop apps to use the microphone (Settings → Privacy & security → Microphone).
          {/if}
        </div>
      </div>
      {#if isMac}
        <div class="box">
          <strong>Accessibility</strong>
          {#if info?.accessibility}
            <div class="ok">✔ Granted</div>
          {:else}
            <div class="muted">
              Needed to see the push-to-talk key while another app has focus. Add Jar-vis under Privacy & Security →
              Accessibility, then restart Jar-vis.
            </div>
            <div class="inline">
              <button onclick={() => api.openAccessibilitySettings()}>Open System Settings</button>
              <button onclick={recheckPermissions}>Check again</button>
            </div>
          {/if}
        </div>
      {:else}
        <p class="muted">
          No other permission is needed on Windows. Note that the key is not seen while a window running as
          administrator has focus.
        </p>
      {/if}
    {:else if STEPS[step] === "Push-to-talk key"}
      <h2>Choose your push-to-talk key</h2>
      <p>Hold this one key to talk and release it to send. A key you rarely use otherwise works best.</p>
      <KeyRecorder bind:value={config.hotkey.key} />
      {#if snap && !snap.hotkeyListening}
        <p class="error">The global key listener is not running yet. Grant the permission from the previous step and restart Jar-vis.</p>
      {/if}
    {:else if STEPS[step] === "Speech models"}
      <h2>Download the speech models</h2>
      <p>Both run on this machine. The defaults are about 130 MB together; you can switch to bigger ones any time.</p>
      <div class="inline">
        <button class="primary" disabled={defaultsReady} onclick={downloadDefaults}>
          {defaultsReady ? "Defaults are installed" : "Download the defaults"}
        </button>
        <label class="check">
          <input type="checkbox" bind:checked={config.audio.tts_enabled} />
          Speak answers aloud
        </label>
      </div>
      <h3>Speech recognition</h3>
      <ModelList kind="stt" bind:selected={config.stt.model} onchange={refreshModels} />
      {#if config.audio.tts_enabled}
        <h3>Voice</h3>
        <ModelList kind="tts" bind:selected={config.tts.voice} onchange={refreshModels} />
      {/if}
    {:else if STEPS[step] === "Try it"}
      <h2>Try it</h2>
      <p>Hold <strong>{keyLabel(config.hotkey.key)}</strong>, say <em>“hello”</em>, and release.</p>
      <div class="box">
        <div>State: <strong>{snap?.state ?? "…"}</strong>{snap?.detail ? ` — ${snap.detail}` : ""}</div>
        {#if last}
          <div class="muted">You said: {last.transcript}</div>
          {#if last.pending}
            <div class="muted">Waiting for the answer…</div>
          {:else if last.error}
            <div class="error">{last.error}</div>
          {:else}
            <div class="ok">{last.summary}</div>
          {/if}
        {:else}
          <div class="muted">Nothing heard yet.</div>
        {/if}
      </div>
      {#if !defaultsReady}
        <p class="error">The selected speech model is not downloaded yet. Go back one step.</p>
      {/if}
    {:else}
      <h2>All set</h2>
      <p>
        Jar-vis lives in the corner of your screen and in the tray. Click the pet for the panel, right-click it for
        the menu, and say “start new session” to begin a fresh conversation.
      </p>
      <p class="muted">Everything here can be changed later under Settings in the tray menu.</p>
    {/if}

    <footer>
      <button disabled={step === 0} onclick={() => go(step - 1)}>Back</button>
      <div class="spacer"></div>
      {#if step < STEPS.length - 1}
        <button class="ghost" onclick={finish}>Skip setup</button>
        <button class="primary" disabled={STEPS[step] === "Home folder" && !home.trim()} onclick={() => go(step + 1)}>Next</button>
      {:else}
        <button class="primary" onclick={finish}>Finish</button>
      {/if}
    </footer>
  </section>
</main>

<style>
  :global(html),
  :global(body) {
    background: var(--bg);
  }
  main {
    height: 100%;
    display: flex;
  }
  aside {
    width: 190px;
    padding: 18px 14px;
    background: var(--bg-raised);
    border-right: 1px solid var(--border);
  }
  h1 {
    font-size: 15px;
    margin: 0 0 14px;
  }
  ol {
    margin: 0;
    padding-left: 20px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    color: var(--muted);
  }
  li.current {
    color: var(--accent);
    font-weight: 600;
  }
  li.done {
    color: var(--text);
  }
  section {
    flex: 1;
    display: flex;
    flex-direction: column;
    padding: 18px 24px 14px;
    overflow-y: auto;
  }
  h2 {
    margin: 0 0 8px;
    font-size: 17px;
  }
  h3 {
    margin: 14px 0 4px;
    font-size: 13px;
  }
  p {
    line-height: 1.5;
    margin: 6px 0;
  }
  .inline {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 8px 0;
  }
  .inline input[type="text"] {
    flex: 1;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
  }
  .box {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 12px;
    margin: 8px 0;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .muted {
    color: var(--muted);
  }
  .ok {
    color: var(--accent);
  }
  .error {
    color: var(--danger);
  }
  code {
    font-family: "Cascadia Code", Consolas, Menlo, monospace;
    font-size: 12px;
  }
  footer {
    display: flex;
    gap: 8px;
    margin-top: auto;
    padding-top: 14px;
  }
  .spacer {
    flex: 1;
  }
</style>
