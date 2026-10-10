<script lang="ts">
  import { onMount, tick } from "svelte";
  import DOMPurify from "dompurify";
  import { marked } from "marked";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { api, on } from "../lib/ipc";
  import type { AgentModel, Exchange, OpenTarget, Snapshot } from "../lib/types";

  let snap = $state<Snapshot | null>(null);
  let stream = $state("");
  let draft = $state("");
  let typing = $state(false);
  let menuOpen = $state(false);
  let models = $state<AgentModel[]>([]);
  let modelsOpen = $state(false);
  let modelsError = $state("");
  let expanded = $state<Record<number, boolean>>({});
  let scroller: HTMLElement | undefined = $state();
  let input: HTMLInputElement | undefined = $state();
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  let autoHideSeconds = 0;

  const busy = $derived(snap?.state === "working" || snap?.state === "sending" || snap?.state === "permission");
  const title = $derived(snap?.session?.title ?? (snap?.session ? "New session" : "No session yet"));

  const modelName = $derived(models.find((m) => m.key === snap?.model)?.name ?? snap?.model?.split("/").slice(1).join("/") ?? "default model");
  // The same list opencode shows, grouped by provider.
  const modelGroups = $derived.by(() => {
    const groups = new Map<string, AgentModel[]>();
    for (const m of models) groups.set(m.provider, [...(groups.get(m.provider) ?? []), m]);
    return [...groups.entries()];
  });

  async function loadModels() {
    try {
      models = await api.listAgentModels();
      modelsError = "";
    } catch (e) {
      modelsError = String(e);
    }
  }

  function toggleModels() {
    menuOpen = false;
    modelsOpen = !modelsOpen;
    if (modelsOpen) loadModels();
  }

  function pickModel(key: string) {
    modelsOpen = false;
    if (key !== snap?.model) api.setModel(key);
  }

  function md(text: string): string {
    return DOMPurify.sanitize(marked.parse(text, { async: false, breaks: true }) as string);
  }

  function time(ms: number): string {
    return new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }

  async function apply(next: Snapshot) {
    const before = snap?.history.at(-1);
    const after = next.history.at(-1);
    const answered = !!after && !after.pending && (before?.pending || before?.time !== after.time);
    if (!next.history.some((e) => e.pending)) stream = "";
    if (next.session?.id !== snap?.session?.id) expanded = {};
    // The list depends on the server and on the session's directory.
    const reload = !!next.backend && (!snap?.backend || next.session?.directory !== snap?.session?.directory || !models.length);
    snap = next;
    if (reload) loadModels();
    await tick();
    scroller?.scrollTo({ top: scroller.scrollHeight });
    if (answered) scheduleHide();
  }

  // `ui.panel_auto_hide_seconds`: hide a while after an answer, unless the user is typing.
  function scheduleHide() {
    clearTimeout(hideTimer);
    if (autoHideSeconds > 0) {
      hideTimer = setTimeout(() => {
        if (!typing) api.hidePanel();
      }, autoHideSeconds * 1000);
    }
  }

  onMount(() => {
    const unlisten = [
      on.snapshot(apply),
      on.progress((text) => (stream += text)),
      on.configChanged((c) => (autoHideSeconds = c.ui.panel_auto_hide_seconds)),
    ];
    api.getSnapshot().then(apply);
    api.getConfig().then((c) => (autoHideSeconds = c.ui.panel_auto_hide_seconds));
    return () => unlisten.forEach((u) => u.then((f) => f()));
  });

  // The panel is non-focusable so it never steals focus. Typing needs focus,
  // so it is granted while the text box is in use and given back afterwards.
  async function startTyping() {
    typing = true;
    await api.panelFocus(true);
    await tick();
    input?.focus();
  }
  function stopTyping() {
    typing = false;
    api.panelFocus(false);
  }
  function submit(e: Event) {
    e.preventDefault();
    const text = draft.trim();
    if (text) api.sendText(text);
    draft = "";
  }
  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      (e.target as HTMLElement).blur();
    }
  }

  function openIn(target: OpenTarget) {
    menuOpen = false;
    api.openSessionIn(target).catch((err) => console.error(err));
  }

  function onHeaderDown(e: MouseEvent) {
    if (e.button === 0 && !(e.target as HTMLElement).closest("button, input, label, .menu")) {
      getCurrentWindow().startDragging();
    }
  }

  function isOpen(i: number, e: Exchange, last: boolean): boolean {
    return expanded[i] ?? (last && e.details.length > 0 && e.details.length < 600);
  }
</script>

<main>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <header onmousedown={onHeaderDown}>
    <div class="title">
      <span class="name" title={snap?.session?.id ?? ""}>{title}</span>
      <span class="mode" class:attached={snap?.session?.mode === "attached"}>
        {#if snap?.session?.mode === "attached"}attached: {snap.session.project}{:else}workspace{/if}
        {#if snap?.backend}· opencode {snap.backend.version}{:else}· offline{/if}
      </span>
    </div>
    <label class="mute" title="Mute spoken answers">
      <input type="checkbox" checked={snap?.muted ?? false} onchange={(e) => api.setMuted(e.currentTarget.checked)} />
      mute
    </label>
    <div class="menu">
      <button class="ghost" title="Open this session elsewhere" disabled={!snap?.session} onclick={() => ((modelsOpen = false), (menuOpen = !menuOpen))}>↗</button>
      {#if menuOpen}
        <div class="dropdown">
          <button onclick={() => openIn("web")}>opencode web</button>
          <button onclick={() => openIn("tui")}>opencode TUI</button>
          <button onclick={() => openIn("openchamber")}>openchamber</button>
          <button onclick={() => openIn("copy")}>Copy session ID</button>
        </div>
      {/if}
    </div>
    <button class="ghost" title="New session" onclick={() => api.newSession()}>＋</button>
    <button class="ghost" title="Settings" onclick={() => api.openSettings("general")}>⚙</button>
    <button class="ghost" title="Minimize" onclick={() => api.hidePanel()}>—</button>
  </header>

  <div class="modelbar">
    <span class="label">Model</span>
    <button class="model" title={snap?.model ?? ""} disabled={!snap?.backend} onclick={toggleModels}>
      <span class="model-name">{modelName}</span> ▾
    </button>
    {#if modelsOpen}
      <div class="dropdown models">
        {#if modelsError}<p class="error">{modelsError}</p>{/if}
        {#if !models.length && !modelsError}<p class="hint">Loading…</p>{/if}
        {#each modelGroups as [provider, list] (provider)}
          <div class="group">{provider}</div>
          {#each list as m (m.key)}
            <button class:current={m.key === snap?.model} onclick={() => pickModel(m.key)}>{m.name}</button>
          {/each}
        {/each}
      </div>
    {/if}
  </div>

  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <section class="history" bind:this={scroller} onclick={() => ((menuOpen = false), (modelsOpen = false))}>
    {#if !snap?.history.length}
      <p class="empty">
        {#if snap?.state === "offline"}
          opencode is not running. Jar-vis will connect as soon as it is.
        {:else}
          Hold your push-to-talk key and speak, or type below.
        {/if}
      </p>
    {/if}
    {#each snap?.history ?? [] as e, i (e.time + ":" + i)}
      {@const last = i === (snap?.history.length ?? 0) - 1}
      <article class:old={!last}>
        {#if e.transcript}
          <p class="you"><span class="t">{time(e.time)}</span> {e.transcript}</p>
        {/if}
        {#if e.pending}
          <p class="progress">
            <span class="spinner"></span>
            {#if snap?.state === "permission"}
              waiting for your permission…
            {:else}
              {snap?.detail && snap.state === "working" ? snap.detail : "working…"}
            {/if}
          </p>
          {#if stream}<div class="stream">{stream}</div>{/if}
        {:else if e.error}
          <p class="error">{e.error}</p>
        {:else}
          <p class="summary">{e.summary}</p>
          {#if e.details}
            <button class="ghost toggle" onclick={() => (expanded[i] = !isOpen(i, e, last))}>
              Details {isOpen(i, e, last) ? "▾" : "▸"}
            </button>
            {#if isOpen(i, e, last)}
              <div class="details">{@html md(e.details)}</div>
            {/if}
          {/if}
        {/if}
      </article>
    {/each}
  </section>

  {#if snap?.permission}
    <section class="permission">
      <p><strong>Permission needed</strong> · {snap.permission.kind}</p>
      <p class="what">{snap.permission.title}</p>
      <div class="row">
        <button class="primary" onclick={() => api.replyPermission(snap!.permission!.id, "once")}>Allow once</button>
        <button onclick={() => api.replyPermission(snap!.permission!.id, "always")}>Always</button>
        <button class="danger" onclick={() => api.replyPermission(snap!.permission!.id, "reject")}>Deny</button>
      </div>
    </section>
  {/if}

  <footer>
    <form onsubmit={submit}>
      <input
        bind:this={input}
        type="text"
        placeholder={typing ? "Type a message, Enter to send" : "Click to type…"}
        bind:value={draft}
        onmousedown={startTyping}
        onblur={stopTyping}
        onkeydown={onKey}
      />
    </form>
    {#if busy}
      <button class="danger" title="Stop the current task" onclick={() => api.abort()}>Stop</button>
    {/if}
    {#if snap && snap.queued > 0}<span class="queued">{snap.queued} queued</span>{/if}
  </footer>
</main>

<style>
  main {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 12px;
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 6px 6px 12px;
    background: var(--bg-raised);
    border-bottom: 1px solid var(--border);
    cursor: grab;
  }
  .title {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .name {
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mode {
    font-size: 10.5px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mode.attached {
    color: var(--accent);
  }
  .mute {
    display: flex;
    align-items: center;
    gap: 3px;
    font-size: 11px;
    color: var(--muted);
    cursor: pointer;
  }
  .menu {
    position: relative;
  }
  .dropdown {
    position: absolute;
    right: 0;
    top: 100%;
    z-index: 5;
    display: flex;
    flex-direction: column;
    min-width: 150px;
    padding: 4px;
    gap: 2px;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.5);
  }
  .dropdown button {
    text-align: left;
    border-color: transparent;
    background: transparent;
  }
  .dropdown button:hover {
    background: var(--bg);
  }

  .modelbar {
    position: relative;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border-bottom: 1px solid var(--border);
  }
  .label {
    font-size: 11px;
    color: var(--muted);
  }
  .model {
    display: flex;
    align-items: center;
    gap: 4px;
    max-width: 260px;
    padding: 2px 8px;
    font-size: 12px;
  }
  .model-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dropdown.models {
    left: 10px;
    right: auto;
    width: 250px;
    max-height: 280px;
    overflow-y: auto;
  }
  .group {
    padding: 6px 8px 2px;
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
  }
  .dropdown button.current {
    color: var(--accent);
    font-weight: 600;
  }
  .hint {
    padding: 6px 8px;
    color: var(--muted);
  }

  .history {
    flex: 1;
    overflow-y: auto;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .empty {
    margin: auto;
    color: var(--muted);
    text-align: center;
    padding: 0 20px;
  }
  article {
    display: flex;
    flex-direction: column;
    gap: 4px;
    align-items: flex-start;
  }
  article.old {
    opacity: 0.72;
  }
  p {
    margin: 0;
  }
  .you {
    color: var(--muted);
    font-size: 12px;
    user-select: text;
    -webkit-user-select: text;
  }
  .t {
    font-size: 10px;
    opacity: 0.7;
  }
  .summary {
    font-size: 15px;
    line-height: 1.4;
    user-select: text;
    -webkit-user-select: text;
  }
  article.old .summary {
    font-size: 13px;
  }
  .error {
    color: var(--danger);
    user-select: text;
    -webkit-user-select: text;
  }
  .toggle {
    color: var(--accent);
    font-size: 11.5px;
    padding-left: 0;
  }
  .details,
  .stream {
    align-self: stretch;
    padding: 8px 10px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 8px;
    line-height: 1.45;
    overflow-x: auto;
    user-select: text;
    -webkit-user-select: text;
  }
  .stream {
    color: var(--muted);
    white-space: pre-wrap;
    max-height: 120px;
    overflow-y: auto;
  }
  .details :global(pre) {
    background: #0a0d13;
    padding: 8px;
    border-radius: 6px;
    overflow-x: auto;
  }
  .details :global(code) {
    font-family: "Cascadia Code", Consolas, Menlo, monospace;
    font-size: 12px;
  }
  .details :global(p),
  .details :global(ul),
  .details :global(ol) {
    margin: 0 0 6px;
  }
  .details :global(ul),
  .details :global(ol) {
    padding-left: 20px;
  }
  .details :global(h1),
  .details :global(h2),
  .details :global(h3) {
    font-size: 13.5px;
    margin: 6px 0 4px;
  }
  .details :global(a) {
    color: var(--accent);
  }
  .details :global(table) {
    border-collapse: collapse;
  }
  .details :global(td),
  .details :global(th) {
    border: 1px solid var(--border);
    padding: 2px 6px;
  }
  .progress {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--accent);
    font-size: 12px;
  }
  .spinner {
    width: 10px;
    height: 10px;
    border: 2px solid var(--accent-dim);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    flex: none;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .permission {
    margin: 0 10px 8px;
    padding: 8px 10px;
    border: 1px solid #8a6a14;
    border-radius: 8px;
    background: rgba(251, 191, 36, 0.08);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .what {
    font-family: "Cascadia Code", Consolas, Menlo, monospace;
    font-size: 12px;
    word-break: break-word;
    user-select: text;
    -webkit-user-select: text;
  }
  .row {
    display: flex;
    gap: 6px;
  }

  footer {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
    border-top: 1px solid var(--border);
  }
  form {
    flex: 1;
    display: flex;
  }
  footer input {
    flex: 1;
    min-width: 0;
  }
  .queued {
    font-size: 11px;
    color: var(--muted);
  }
</style>
