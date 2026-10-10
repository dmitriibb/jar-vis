<script lang="ts">
  // Current session, and the attach picker: project -> session.
  import { onMount } from "svelte";
  import { api } from "../lib/ipc";
  import type { ProjectRef, SessionSummary, Snapshot } from "../lib/types";

  let { snap }: { snap: Snapshot | null } = $props();

  let projects = $state<ProjectRef[]>([]);
  let projectId = $state("");
  let sessions = $state<SessionSummary[]>([]);
  let search = $state("");
  let loading = $state(false);
  let error = $state("");

  const filtered = $derived(
    sessions.filter((s) => {
      const q = search.trim().toLowerCase();
      return !q || (s.title ?? "").toLowerCase().includes(q) || s.id.toLowerCase().includes(q);
    }),
  );

  async function loadProjects() {
    error = "";
    try {
      projects = await api.listProjects();
      if (!projectId && projects.length) projectId = projects[0].id;
    } catch (e) {
      error = String(e);
    }
  }

  async function loadSessions() {
    if (!projectId) return;
    loading = true;
    error = "";
    try {
      sessions = await api.listSessions(projectId);
    } catch (e) {
      error = String(e);
      sessions = [];
    } finally {
      loading = false;
    }
  }

  onMount(loadProjects);
  $effect(() => {
    projectId;
    loadSessions();
  });

  function when(ms: number): string {
    const minutes = Math.round((Date.now() - ms) / 60_000);
    if (minutes < 1) return "just now";
    if (minutes < 60) return `${minutes} min ago`;
    if (minutes < 60 * 24) return `${Math.round(minutes / 60)} h ago`;
    return new Date(ms).toLocaleDateString();
  }
</script>

<h3>Current session</h3>
{#if snap?.session}
  <div class="current">
    <div>
      <strong>{snap.session.title ?? "New session"}</strong>
      <span class="tag" class:attached={snap.session.mode === "attached"}>{snap.session.mode}</span>
    </div>
    <div class="mono">{snap.session.id}</div>
    <div class="mono">{snap.session.directory}</div>
  </div>
  <div class="row">
    <button onclick={() => api.openSessionIn("web")}>Open in opencode web</button>
    <button onclick={() => api.openSessionIn("tui")}>Open in opencode TUI</button>
    <button onclick={() => api.openSessionIn("openchamber")}>Open in openchamber</button>
    <button onclick={() => api.openSessionIn("copy")}>Copy ID</button>
  </div>
{:else}
  <p class="muted">No session yet. One is created with your first prompt.</p>
{/if}
<div class="row">
  <button onclick={() => api.newSession()}>New session</button>
  <button disabled={snap?.session?.mode !== "attached"} onclick={() => api.detach()}>Detach</button>
</div>

<h3>Attach to an existing session</h3>
<p class="muted">
  Talk to a session from any project. Jar-vis keeps that session's agent and adds a short note to each message so
  answers stay speakable. Detach to return to the Jar-vis workspace.
</p>
<div class="row">
  <select bind:value={projectId}>
    {#each projects as p (p.id)}
      <option value={p.id}>{p.name} — {p.directory}</option>
    {/each}
  </select>
  <input type="text" placeholder="Search sessions" bind:value={search} />
  <button onclick={() => (loadProjects(), loadSessions())}>Refresh</button>
</div>
{#if error}<p class="error">{error}</p>{/if}
<div class="list">
  {#if loading}
    <p class="muted">Loading…</p>
  {:else if !filtered.length}
    <p class="muted">No sessions.</p>
  {/if}
  {#each filtered as s (s.id)}
    <div class="item" class:active={s.id === snap?.session?.id}>
      <div class="info">
        <div>{s.title ?? "Untitled session"}</div>
        <div class="mono">{when(s.updated)} · {s.directory}</div>
      </div>
      <button disabled={s.id === snap?.session?.id} onclick={() => api.attach(s.id, s.directory)}>
        {s.id === snap?.session?.id ? "Current" : "Attach"}
      </button>
    </div>
  {/each}
</div>

<style>
  h3 {
    margin: 14px 0 6px;
    font-size: 13px;
  }
  h3:first-child {
    margin-top: 0;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 8px 0;
  }
  .row select {
    flex: 2;
    min-width: 0;
  }
  .row input {
    flex: 1;
    min-width: 0;
  }
  .current {
    padding: 8px 10px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .tag {
    margin-left: 6px;
    font-size: 10.5px;
    color: var(--muted);
  }
  .tag.attached {
    color: var(--accent);
  }
  .mono {
    font-size: 11px;
    color: var(--muted);
    word-break: break-all;
    user-select: text;
    -webkit-user-select: text;
  }
  .muted {
    color: var(--muted);
    margin: 4px 0;
  }
  .error {
    color: var(--danger);
  }
  .list {
    border: 1px solid var(--border);
    border-radius: 8px;
    max-height: 230px;
    overflow-y: auto;
    padding: 2px 8px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 0;
    border-bottom: 1px solid var(--border);
  }
  .item:last-child {
    border-bottom: none;
  }
  .item.active .info {
    color: var(--accent);
  }
  .info {
    flex: 1;
    min-width: 0;
  }
</style>
