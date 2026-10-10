<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, keyLabel, on } from "../lib/ipc";
  import type { Config, ConnectionInfo, Personality, Snapshot, Toast } from "../lib/types";
  import KeyRecorder from "./KeyRecorder.svelte";
  import ModelList from "./ModelList.svelte";
  import Sessions from "./Sessions.svelte";
  import Wizard from "./Wizard.svelte";

  const TABS = [
    ["general", "General"],
    ["input", "Voice input"],
    ["output", "Voice output"],
    ["opencode", "OpenCode"],
    ["persona", "Personality & pet"],
    ["sessions", "Sessions"],
    ["advanced", "Advanced"],
  ] as const;

  let tab = $state(location.hash.slice(1) || "general");
  let config = $state<Config | null>(null);
  let snap = $state<Snapshot | null>(null);
  let devices = $state<string[]>([]);
  let personalities = $state<Personality[]>([]);
  let pets = $state<string[]>([]);
  let connection = $state<ConnectionInfo | null>(null);
  let testing = $state(false);
  let toasts = $state<Toast[]>([]);
  let status = $state("");
  // JSON of the config as last saved, to tell edits from echoes.
  let saved = "";
  let saveTimer: ReturnType<typeof setTimeout> | undefined;

  onMount(() => {
    const unlisten = [
      on.snapshot((s) => (snap = s)),
      on.settingsTab((t) => (tab = t)),
      on.toast((t) => {
        toasts = [...toasts.slice(-2), t];
        setTimeout(() => (toasts = toasts.filter((x) => x !== t)), 6000);
      }),
      // Mute, personality and the like can change from the tray while this is open.
      on.configChanged((c) => {
        if (JSON.stringify(c) !== JSON.stringify(config)) {
          saved = JSON.stringify(c);
          config = c;
        }
      }),
    ];
    api.getConfig().then((c) => {
      saved = JSON.stringify(c);
      config = c;
    });
    api.getSnapshot().then((s) => (snap = s));
    api.listInputDevices().then((d) => (devices = d));
    refreshPersonas();
    return () => unlisten.forEach((u) => u.then((f) => f()));
  });

  function refreshPersonas() {
    api.listPersonalities().then((p) => (personalities = p));
    api.listPets().then((p) => (pets = p));
  }

  // Every edit is saved and applied after a short pause; there is no Save button.
  $effect(() => {
    if (!config) return;
    const json = JSON.stringify(config);
    if (json === saved) return;
    clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
      try {
        const applied = await api.saveConfig(JSON.parse(json));
        saved = JSON.stringify(applied);
        // Validation may have corrected a value.
        if (saved !== JSON.stringify(config)) config = applied;
        status = "Saved";
        setTimeout(() => (status = ""), 1500);
      } catch (e) {
        status = `Not saved: ${e}`;
      }
    }, 400);
  });

  async function browse(apply: (path: string) => void, directory = true) {
    const picked = await open({ directory, multiple: false });
    if (typeof picked === "string") apply(picked);
  }

  async function testConnection() {
    testing = true;
    connection = await api.connectionTest().catch((e) => ({ connected: false, info: null, binary: null, error: String(e) }));
    testing = false;
  }

  async function reset() {
    config = await api.resetConfig();
    saved = JSON.stringify(config);
  }

  const lastHeard = $derived(snap?.history.at(-1)?.transcript ?? "");
</script>

{#if !config}
  <main class="loading">Loading…</main>
{:else if tab === "wizard"}
  <Wizard bind:config {snap} ondone={() => (tab = "general")} />
{:else}
  <main>
    <nav>
      {#each TABS as [id, label] (id)}
        <button class:active={tab === id} onclick={() => (tab = id)}>{label}</button>
      {/each}
      <div class="spacer"></div>
      <span class="status">{status}</span>
      <button class="ghost" onclick={() => (tab = "wizard")}>Run setup again</button>
    </nav>

    <section>
      {#if tab === "general"}
        <h2>General</h2>
        <label class="field">
          <span>Jar-vis home folder</span>
          <div class="inline">
            <input type="text" readonly value={config.home} />
            <button onclick={() => browse((p) => (config!.home = p))}>Change…</button>
            <button onclick={() => api.openPath("home")}>Open</button>
          </div>
          <small>Holds <code>assistant/</code> (personalities, skills, pets), <code>sessions/</code> and <code>models/</code>.</small>
        </label>
        <div class="field">
          <span>Push-to-talk key</span>
          <KeyRecorder bind:value={config.hotkey.key} />
          {#if snap && !snap.hotkeyListening}
            <small class="warn">The global key listener is not running. On macOS, grant Accessibility access and restart Jar-vis.</small>
          {/if}
        </div>
        <label class="check">
          <input type="checkbox" bind:checked={config.hotkey.cancel_on_chord} />
          Cancel the recording when another key is pressed while holding it
        </label>
        <label class="field">
          <span>Wake word (optional, stripped from the start of what you say)</span>
          <input type="text" class="short" bind:value={config.commands.wake_word} />
        </label>
        <h3>Pet and panel</h3>
        <label class="field">
          <span>Fall asleep after (minutes idle, 0 = never)</span>
          <input type="number" class="short" min="0" max="600" bind:value={config.ui.sleep_after_minutes} />
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={config.ui.panel_auto_show} />
          Show the panel when an answer arrives
        </label>
        <label class="field">
          <span>Hide the panel again after (seconds, 0 = never)</span>
          <input type="number" class="short" min="0" max="3600" bind:value={config.ui.panel_auto_hide_seconds} />
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={config.ui.stream_text} />
          Show the answer text in the panel while it is being written
        </label>
      {:else if tab === "input"}
        <h2>Voice input</h2>
        <label class="field">
          <span>Microphone</span>
          <select bind:value={config.audio.input_device}>
            <option value="">System default</option>
            {#each devices as d (d)}<option value={d}>{d}</option>{/each}
            {#if config.audio.input_device && !devices.includes(config.audio.input_device)}
              <option value={config.audio.input_device}>{config.audio.input_device} (not connected)</option>
            {/if}
          </select>
        </label>
        <h3>Speech recognition model</h3>
        <p class="muted">English-only Whisper models, run on this machine. Audio never leaves it.</p>
        <ModelList kind="stt" bind:selected={config.stt.model} />
        <label class="field">
          <span>Vocabulary (terms you use often; improves recognition)</span>
          <textarea rows="2" bind:value={config.stt.vocabulary}></textarea>
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={config.stt.preload} />
          Load the model at startup, so the first request is fast
        </label>
        <label class="field">
          <span>Unload the model after (minutes idle, 0 = keep loaded)</span>
          <input type="number" class="short" min="0" max="1440" bind:value={config.stt.unload_after_idle_minutes} />
        </label>
        <div class="inline">
          <label class="field">
            <span>Ignore clips shorter than (s)</span>
            <input type="number" class="short" min="0" max="5" step="0.05" bind:value={config.stt.min_seconds} />
          </label>
          <label class="field">
            <span>Longest recording (s)</span>
            <input type="number" class="short" min="5" max="600" bind:value={config.stt.max_seconds} />
          </label>
        </div>
        <h3>Try it</h3>
        <div class="test">
          <div>Hold <strong>{keyLabel(config.hotkey.key)}</strong> and speak. What you say is sent to the current session.</div>
          <div class="muted">State: {snap?.state ?? "…"}{snap?.detail ? ` — ${snap.detail}` : ""}</div>
          <div class="muted">Last heard: {lastHeard || "—"}</div>
        </div>
      {:else if tab === "output"}
        <h2>Voice output</h2>
        <label class="check">
          <input type="checkbox" bind:checked={config.audio.tts_enabled} />
          Speak answers aloud
        </label>
        <p class="muted">When this is off the voice model is not loaded at all.</p>
        <h3>Voice</h3>
        <p class="muted">A personality can name its own voice; that one is used when it is downloaded.</p>
        <ModelList kind="tts" bind:selected={config.tts.voice} />
        <label class="field">
          <span>Speed: {config.tts.speed.toFixed(2)}×</span>
          <input type="range" min="0.5" max="2" step="0.05" bind:value={config.tts.speed} />
        </label>
        <label class="field">
          <span>Longest spoken answer (characters)</span>
          <input type="number" class="short" min="40" max="4000" bind:value={config.tts.max_spoken_chars} />
        </label>
        <label class="field">
          <span>Unload the voice after (minutes idle, 0 = keep loaded)</span>
          <input type="number" class="short" min="0" max="1440" bind:value={config.tts.unload_after_idle_minutes} />
        </label>
      {:else if tab === "opencode"}
        <h2>OpenCode</h2>
        <div class="test">
          <div>
            {#if snap?.backend}
              Connected to opencode <strong>{snap.backend.version}</strong> at <code>{snap.backend.url}</code>
            {:else}
              <span class="warn">Not connected.</span>
            {/if}
          </div>
          <div class="inline">
            <button onclick={testConnection} disabled={testing}>{testing ? "Testing…" : "Test connection"}</button>
            {#if connection}
              {#if connection.connected}
                <span class="ok">OK — opencode {connection.info?.version}</span>
              {:else}
                <span class="error">{connection.error}</span>
              {/if}
            {/if}
          </div>
          {#if connection}<div class="muted">Binary: {connection.binary ?? "not found"}</div>{/if}
        </div>
        <label class="check">
          <input type="checkbox" bind:checked={config.opencode.autostart} />
          Start opencode's background service when it is not running
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={config.opencode.stop_on_exit} />
          Stop the service on exit, if Jar-vis started it
        </label>
        <label class="field">
          <span>opencode binary (empty = find it automatically)</span>
          <div class="inline">
            <input type="text" bind:value={config.opencode.binary} />
            <button onclick={() => browse((p) => (config!.opencode.binary = p), false)}>Browse…</button>
          </div>
        </label>
        <label class="field">
          <span>Workspace folder for Jar-vis sessions</span>
          <div class="inline">
            <input type="text" readonly value={config.opencode.workspace_dir} placeholder="sessions/ in the home folder" />
            <button onclick={() => browse((p) => (config!.opencode.workspace_dir = p))}>Change…</button>
            <button disabled={!config.opencode.workspace_dir} onclick={() => (config!.opencode.workspace_dir = "")}>Use default</button>
          </div>
        </label>
        <h3>Use a different server</h3>
        <p class="muted">
          Leave the URL empty to share opencode's background service with the TUI and the web UI. Set it only to talk to
          an <code>opencode serve</code> instance you run yourself.
        </p>
        <label class="field">
          <span>Server URL</span>
          <input type="text" placeholder="http://127.0.0.1:4096" bind:value={config.opencode.url} />
        </label>
        <div class="inline">
          <label class="field">
            <span>Username</span>
            <input type="text" bind:value={config.opencode.username} />
          </label>
          <label class="field">
            <span>Password</span>
            <input type="password" bind:value={config.opencode.password} />
          </label>
        </div>
        <h3>openchamber</h3>
        <label class="field">
          <span>URL (web or PWA)</span>
          <input type="text" placeholder="http://localhost:3000" bind:value={config.openchamber.url} />
        </label>
        <label class="field">
          <span>Or desktop app path</span>
          <input type="text" bind:value={config.openchamber.app_path} />
        </label>
        <h3>Attach mode</h3>
        <label class="field">
          <span>Agent to switch attached sessions to (empty = keep the session's own agent)</span>
          <input type="text" class="short" bind:value={config.attach.override_agent} />
        </label>
      {:else if tab === "persona"}
        <h2>Personality & pet</h2>
        <p class="muted">
          Personalities are opencode agents in <code>assistant/agents</code>. Add a markdown file there to make your own.
        </p>
        <div class="personas">
          {#each personalities as p (p.name)}
            <label class="persona" class:active={config.persona.active === p.name}>
              <input type="radio" name="persona" value={p.name} checked={config.persona.active === p.name} onchange={() => api.setPersonality(p.name)} />
              <div>
                <strong>{p.displayName}</strong>
                <div class="muted">{p.description}</div>
                <div class="muted small">voice: {p.voice ?? "default"} · pet: {p.pet ?? "default"}</div>
              </div>
            </label>
          {/each}
          {#if !personalities.length}<p class="muted">No personalities found in the home folder.</p>{/if}
        </div>
        <label class="field">
          <span>Pet skin</span>
          <select class="short" bind:value={config.persona.pet}>
            {#each pets as p (p)}<option value={p}>{p}</option>{/each}
          </select>
          <small>Skins live in <code>assistant/pets/&lt;name&gt;/pet.json</code>.</small>
        </label>
        <div class="inline">
          <button onclick={() => (api.resyncPersonas(), refreshPersonas())}>Re-sync agents & skills</button>
          <span class="muted">Regenerates <code>sessions/.opencode</code> and tells opencode to reload it.</span>
        </div>
      {:else if tab === "sessions"}
        <Sessions {snap} />
      {:else if tab === "advanced"}
        <h2>Advanced</h2>
        <label class="field">
          <span>Log level</span>
          <select class="short" bind:value={config.log.level}>
            {#each ["error", "warn", "info", "debug", "trace"] as l (l)}<option value={l}>{l}</option>{/each}
          </select>
          <small>Applies after a restart.</small>
        </label>
        <label class="field">
          <span>Models folder</span>
          <div class="inline">
            <input type="text" readonly value={config.models.dir} placeholder="models/ in the home folder" />
            <button onclick={() => browse((p) => (config!.models.dir = p))}>Change…</button>
            <button disabled={!config.models.dir} onclick={() => (config!.models.dir = "")}>Use default</button>
          </div>
        </label>
        <div class="inline">
          <button onclick={() => api.openPath("logs")}>Open logs folder</button>
          <button onclick={() => api.openPath("config")}>Open config file</button>
          <button class="danger" onclick={reset}>Reset to defaults</button>
        </div>
        <p class="muted">Reset keeps the home folder and the push-to-talk key. Voice command phrases are edited in the config file.</p>
      {/if}
    </section>

    {#if toasts.length}
      <div class="toasts">
        {#each toasts as t (t)}<div class="toast {t.level}">{t.text}</div>{/each}
      </div>
    {/if}
  </main>
{/if}

<style>
  :global(html),
  :global(body) {
    background: var(--bg);
  }
  main {
    height: 100%;
    display: flex;
  }
  .loading {
    align-items: center;
    justify-content: center;
    color: var(--muted);
  }
  nav {
    width: 170px;
    padding: 12px 8px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    background: var(--bg-raised);
    border-right: 1px solid var(--border);
  }
  nav button {
    text-align: left;
    background: transparent;
    border-color: transparent;
    padding: 7px 10px;
  }
  nav button.active {
    background: var(--bg);
    border-color: var(--border);
    color: var(--accent);
  }
  .spacer {
    flex: 1;
  }
  .status {
    font-size: 11px;
    color: var(--muted);
    padding: 0 10px;
    min-height: 16px;
  }
  section {
    flex: 1;
    padding: 16px 22px 24px;
    overflow-y: auto;
  }
  section :global(h2) {
    margin: 0 0 12px;
    font-size: 16px;
  }
  section :global(h3) {
    margin: 18px 0 6px;
    font-size: 13px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 10px 0;
    flex: 1;
  }
  .field > span {
    font-size: 12px;
    color: var(--muted);
  }
  .field input[type="text"],
  .field input[type="password"],
  .field textarea,
  .field select {
    width: 100%;
  }
  .field .short,
  select.short {
    width: 160px;
  }
  small {
    color: var(--muted);
    font-size: 11px;
  }
  .inline {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .inline input[type="text"] {
    flex: 1;
    min-width: 0;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 8px 0;
    cursor: pointer;
  }
  .muted {
    color: var(--muted);
    margin: 4px 0;
  }
  .small {
    font-size: 11px;
  }
  .warn {
    color: var(--warn);
  }
  .error {
    color: var(--danger);
  }
  .ok {
    color: var(--accent);
  }
  code {
    font-family: "Cascadia Code", Consolas, Menlo, monospace;
    font-size: 12px;
    color: #c8d3e6;
  }
  .test {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 12px;
    margin: 8px 0;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .personas {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 8px 0 14px;
  }
  .persona {
    display: flex;
    gap: 10px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
  }
  .persona.active {
    border-color: var(--accent-dim);
    background: rgba(94, 234, 212, 0.05);
  }
  .toasts {
    position: fixed;
    right: 14px;
    bottom: 14px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-width: 380px;
  }
  .toast {
    padding: 8px 12px;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.5);
  }
  .toast.warn {
    border-color: #8a6a14;
  }
  .toast.error {
    border-color: #7f3030;
    color: var(--danger);
  }
</style>
