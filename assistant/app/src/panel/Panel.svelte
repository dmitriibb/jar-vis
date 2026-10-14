<script lang="ts">
  import { onMount, tick } from "svelte";
  import DOMPurify from "dompurify";
  import { marked } from "marked";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { api, on } from "../lib/ipc";
  import type { AgentModel, Exchange, OpenTarget, ScheduledTask, SessionSummary, Snapshot } from "../lib/types";

  let snap = $state<Snapshot | null>(null);
  let stream = $state("");
  let draft = $state("");
  let typing = $state(false);
  let menuOpen = $state(false);
  let models = $state<AgentModel[]>([]);
  let modelsOpen = $state(false);
  let modelsError = $state("");
  let sessions = $state<SessionSummary[]>([]);
  let sessionsOpen = $state(false);
  let sessionsError = $state("");
  let tasksOpen = $state(false);
  // Ticks so "in 12 min" stays current while the task list is open.
  let now = $state(Date.now());
  let agents = $state<string[]>([]);
  let agentsOpen = $state(false);
  let agentsError = $state("");
  let scroller: HTMLElement | undefined = $state();
  let input: HTMLInputElement | undefined = $state();
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  // Index of the answer whose Copy button was just used.
  let copied = $state(-1);
  // The panel holds focus because the user selected text in it.
  let selecting = false;
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

  /** Close every dropdown except the one named. */
  function closeMenus(except = "") {
    if (except !== "menu") menuOpen = false;
    if (except !== "agents") agentsOpen = false;
    if (except !== "models") modelsOpen = false;
    if (except !== "sessions") sessionsOpen = false;
    if (except !== "tasks") tasksOpen = false;
  }

  async function toggleSessions() {
    closeMenus("sessions");
    sessionsOpen = !sessionsOpen;
    if (!sessionsOpen) return;
    try {
      // The Jar-vis workspace sessions, newest first.
      sessions = await api.listSessions(null);
      sessionsError = "";
    } catch (e) {
      sessionsError = String(e);
    }
  }

  function switchSession(id: string, directory: string) {
    closeMenus();
    if (id !== snap?.session?.id) api.attach(id, directory);
  }

  function toggleTasks() {
    closeMenus("tasks");
    tasksOpen = !tasksOpen;
    now = Date.now();
  }

  function ago(ms: number): string {
    const minutes = Math.round((Date.now() - ms) / 60_000);
    if (minutes < 1) return "just now";
    if (minutes < 60) return minutes + " min ago";
    if (minutes < 60 * 24) return Math.round(minutes / 60) + " h ago";
    return new Date(ms).toLocaleDateString();
  }

  /** "15:42 · in 12 min · every 15 min" */
  function taskWhen(t: ScheduledTask): string {
    const at = new Date(t.nextRun);
    const sameDay = at.toDateString() === new Date(now).toDateString();
    const clock = at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    const minutes = Math.max(0, Math.round((t.nextRun - now) / 60_000));
    const rel = minutes < 1 ? "due now" : minutes < 90 ? "in " + minutes + " min" : "in " + Math.round(minutes / 60) + " h";
    const parts = [(sameDay ? "" : at.toLocaleDateString([], { month: "short", day: "numeric" }) + " ") + clock, rel];
    if (t.everyMinutes) parts.push("every " + Math.round(t.everyMinutes) + " min");
    return parts.join(" · ");
  }

  function toggleModels() {
    closeMenus("models");
    modelsOpen = !modelsOpen;
    if (modelsOpen) loadModels();
  }

  function pickModel(key: string) {
    modelsOpen = false;
    if (key !== snap?.model) api.setModel(key);
  }

  async function toggleAgents() {
    closeMenus("agents");
    agentsOpen = !agentsOpen;
    if (!agentsOpen) return;
    agents = [];
    try { agents = await api.listSessionAgents(); agentsError = ""; }
    catch (e) { agentsError = String(e); }
  }

  function pickAgent(agent: string) {
    closeMenus();
    api.setSessionAgent(agent);
  }

  const separator = "-----------------------------------------";
  const streamSummary = $derived.by(() => {
    const lines = stream.split(/\r?\n/);
    const boundary = lines.findIndex((line) => line.trim() === separator || line.trim() === "---");
    const intro = (boundary >= 0 ? lines.slice(0, boundary) : lines).join("\n").replace(/\n-+$/, "");
    return intro.trim().split(/\s+/).slice(0, 50).join(" ");
  });

  function detailsSize(text: string): string {
    return Array.from(text).length.toLocaleString("en-US");
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
    if (next.session?.id !== snap?.session?.id) { stream = ""; closeMenus(); }
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
    const clock = setInterval(() => (now = Date.now()), 30_000);
    return () => {
      clearInterval(clock);
      unlisten.forEach((u) => u.then((f) => f()));
    };
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
  // Ctrl+C needs keyboard focus, which the panel normally refuses. Selecting
  // text is a clear sign the user wants to work in the panel, so focus is taken
  // then, and given back when the window loses it.
  function onSelectEnd() {
    if (typing) return;
    const selected = (window.getSelection()?.toString() ?? "").length > 0;
    if (selected && !selecting) {
      selecting = true;
      api.panelFocus(true);
    }
  }
  function onWindowBlur() {
    if (selecting) {
      selecting = false;
      if (!typing) api.panelFocus(false);
    }
  }

  async function copyAnswer(i: number, e: Exchange) {
    const text = e.summary || e.error || "";
    await api.copyText(text).catch((err) => console.error(err));
    copied = i;
    setTimeout(() => (copied === i ? (copied = -1) : null), 1500);
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


</script>

<svelte:window onblur={onWindowBlur} />

<main>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <header onmousedown={onHeaderDown}>
    <div class="title">
      <button class="ghost name" title="Switch session" disabled={!snap?.backend} onclick={toggleSessions}>
        <span class="name-text">{title}</span> ▾
      </button>
      {#if sessionsOpen}
        <div class="dropdown sessions">
          <button onclick={() => (closeMenus(), api.newSession())}>＋ New session</button>
          {#if sessionsError}<p class="error">{sessionsError}</p>{/if}
          {#if !sessions.length && !sessionsError}<p class="hint">No sessions yet.</p>{/if}
          {#each sessions as s (s.id)}
            <button class:current={s.id === snap?.session?.id} onclick={() => switchSession(s.id, s.directory)}>
              <span class="row-title">{s.title ?? "Untitled session"}</span>
              <span class="row-sub">{ago(s.updated)}{snap?.tasks.some((t) => t.session.id === s.id) ? " · ⏱" : ""}</span>
            </button>
          {/each}
        </div>
      {/if}
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
      <button class="ghost" title="Open this session elsewhere" disabled={!snap?.session} onclick={() => (closeMenus("menu"), (menuOpen = !menuOpen))}>↗</button>
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
    <div class="menu">
      <button class="model" title="Select OpenCode agent" disabled={!snap?.backend || busy} onclick={toggleAgents}>
        {snap?.session?.agent ?? "Agent"} ▾
      </button>
      {#if agentsOpen}
        <div class="dropdown models">
          {#if agentsError}<p class="error">{agentsError}</p>{/if}
          {#if !agents.length && !agentsError}<p class="hint">No primary agents available.</p>{/if}
          {#each agents as agent (agent)}
            <button class:current={agent === snap?.session?.agent} onclick={() => pickAgent(agent)}>{agent}</button>
          {/each}
        </div>
      {/if}
    </div>
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
    <div class="spacer"></div>
    <button class="model" title="Scheduled tasks in all sessions" onclick={toggleTasks}>
      ⏱ {snap?.tasks.length ?? 0}
    </button>
    {#if tasksOpen}
      <div class="dropdown tasks">
        {#if !snap?.tasks.length}
          <p class="hint">Nothing is scheduled. Ask for something like “in an hour, check whether my service is running”.</p>
        {/if}
        {#each snap?.tasks ?? [] as t (t.id)}
          <div class="task">
            <div class="task-body">
              <div class="task-prompt" title={t.prompt}>{t.prompt}</div>
              <div class="row-sub">{taskWhen(t)}</div>
              <button
                class="task-session"
                class:current={t.session.id === snap?.session?.id}
                title="Switch to this session"
                onclick={() => switchSession(t.session.id, t.session.directory)}
              >
                {t.session.id === snap?.session?.id ? "this session" : (t.sessionTitle ?? "another session")}
              </button>
            </div>
            <button class="danger" onclick={() => api.cancelTask(t.id)}>Cancel</button>
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <section class="history" bind:this={scroller} onclick={() => closeMenus()} onmouseup={onSelectEnd}>
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
          <p class="you">
            <span class="t">{time(e.time)}</span>
            {#if e.scheduled}<span class="sched" title="Sent by the scheduler">⏱ scheduled</span>{/if}
            {e.transcript}
          </p>
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
          {#if streamSummary}<div class="stream">{streamSummary}</div>{/if}
        {:else if e.error}
          <p class="error">{e.error}</p>
        {:else}
          <p class="summary">{e.summary}</p>
          {#if e.details}
            <details class="full-answer">
              <summary class="answer-size" title="Expand or collapse the full answer">(full answer {detailsSize(e.details)})</summary>
              <div class="details">{@html md(e.details)}</div>
            </details>
          {/if}
          <div class="actions">
            <button class="ghost toggle copy" title="Copy this answer" onclick={() => copyAnswer(i, e)}>
              {copied === i ? "Copied ✓" : "Copy"}
            </button>
          </div>

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
    position: relative;
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 4px;
    max-width: 100%;
    padding: 0;
    font-weight: 600;
  }
  .name-text {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dropdown.sessions {
    left: 0;
    right: auto;
    width: 260px;
    max-height: 300px;
    overflow-y: auto;
  }
  .dropdown.sessions button,
  .task {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
  }
  .row-title {
    max-width: 100%;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row-sub {
    font-size: 10.5px;
    font-weight: 400;
    color: var(--muted);
  }
  .spacer {
    flex: 1;
  }
  .dropdown.tasks {
    right: 10px;
    left: auto;
    width: 330px;
    max-height: 300px;
    overflow-y: auto;
    cursor: default;
  }
  .task {
    flex-direction: row;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
  }
  .task:last-child {
    border-bottom: none;
  }
  .task-body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
  }
  .task-prompt {
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .dropdown .task-session {
    padding: 0;
    font-size: 10.5px;
    color: var(--muted);
    text-decoration: underline;
  }
  .dropdown .task .danger {
    border-color: #7f3030;
    flex: none;
  }
  .sched {
    color: var(--accent);
    font-size: 10.5px;
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
  .answer-size {
    color: var(--muted, #888);
    font-size: 11px;
    margin-top: 6px;
    cursor: pointer;
  }
  .answer-size:hover,
  .answer-size:focus-visible {
    color: var(--accent);
  }
  .full-answer {
    align-self: stretch;
    min-width: 0;
  }
  .details {
    margin-top: 6px;
    padding: 8px 10px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 8px;
    line-height: 1.45;
    overflow-x: auto;
    overflow-wrap: anywhere;
    user-select: text;
    -webkit-user-select: text;
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
  .actions {
    display: flex;
    gap: 10px;
  }
  .toggle.copy {
    color: var(--muted);
  }
  .toggle {
    color: var(--accent);
    font-size: 11.5px;
    padding-left: 0;
  }
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
