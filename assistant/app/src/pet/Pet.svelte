<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { api, keyLabel, on } from "../lib/ipc";
  import type { PetSkin, Snapshot, Toast } from "../lib/types";
  import Blob from "./Blob.svelte";
  import Sprite from "./Sprite.svelte";

  const TOAST_MS = 5000;
  const DRAG_THRESHOLD = 4;

  let snap = $state<Snapshot | null>(null);
  let level = $state(0);
  let toast = $state<Toast | null>(null);
  let skin = $state<PetSkin | null>(null);
  let loadedSkin = "";
  let idleSince = $state(Date.now());
  let now = $state(Date.now());
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let press: { x: number; y: number } | null = null;
  let dragged = false;

  // "sleeping" is a UI-only state: idle for longer than the configured timeout.
  const visual = $derived.by(() => {
    if (!snap) return "offline";
    const limit = snap.sleepAfterMinutes * 60_000;
    if (snap.state === "idle" && limit > 0 && now - idleSince > limit) return "sleeping";
    return snap.state;
  });

  const line = $derived.by(() => {
    if (toast) return { text: toast.text, kind: toast.level };
    if (!snap) return null;
    switch (snap.state) {
      case "error":
        return { text: snap.detail ?? "Something went wrong", kind: "error" };
      case "permission":
        return { text: `Allow? ${snap.detail ?? ""}`, kind: "warn" };
      case "working":
        return { text: snap.detail ?? "working…", kind: "info" };
      case "sending":
        return { text: "sending…", kind: "info" };
      case "transcribing":
        return { text: "transcribing…", kind: "info" };
      case "listening":
        return { text: "listening", kind: "info" };
      case "offline":
        return { text: snap.detail ?? "offline", kind: "warn" };
      default:
        if (!snap.hotkeyListening) return { text: "key listener is off — see Settings", kind: "warn" };
        if (!snap.wizardDone) return { text: `hold ${keyLabel(snap.hotkey)} to talk`, kind: "info" };
        return snap.queued > 0 ? { text: `${snap.queued} queued`, kind: "info" } : null;
    }
  });

  function apply(next: Snapshot) {
    if (next.state !== "idle" || snap?.state !== "idle") idleSince = Date.now();
    snap = next;
    if (next.pet !== loadedSkin) {
      loadedSkin = next.pet;
      api.getPet(next.pet).then(
        (s) => (skin = s),
        () => (skin = null),
      );
    }
  }

  onMount(() => {
    const unlisten = [
      on.snapshot(apply),
      on.micLevel((rms) => (level = rms)),
      on.toast((t) => {
        toast = t;
        clearTimeout(toastTimer);
        toastTimer = setTimeout(() => (toast = null), TOAST_MS);
      }),
    ];
    api.getSnapshot().then(apply);
    const clock = setInterval(() => (now = Date.now()), 15_000);
    return () => {
      clearInterval(clock);
      unlisten.forEach((u) => u.then((f) => f()));
    };
  });

  // A press that moves becomes a window drag; one that does not is a click.
  function onMouseDown(e: MouseEvent) {
    if (e.button !== 0) return;
    press = { x: e.screenX, y: e.screenY };
    dragged = false;
  }
  function onMouseMove(e: MouseEvent) {
    if (!press || !(e.buttons & 1)) return;
    if (Math.hypot(e.screenX - press.x, e.screenY - press.y) > DRAG_THRESHOLD) {
      press = null;
      dragged = true;
      getCurrentWindow().startDragging();
    }
  }
  function onClick() {
    if (dragged) return;
    idleSince = Date.now();
    api.togglePanel();
  }
  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    api.petMenu();
  }
</script>

<main>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="pet"
    title={snap?.session?.title ?? "Jar-vis"}
    onmousedown={onMouseDown}
    onmousemove={onMouseMove}
    onclick={onClick}
    ondblclick={() => api.replayLast()}
    oncontextmenu={onContextMenu}
  >
    {#if skin}
      <Sprite {skin} state={visual} {level} />
    {:else}
      <Blob state={visual} {level} />
    {/if}
    {#if visual === "listening"}
      <!-- Shown for as long as the key is held and the microphone is open. -->
      <span class="mic" title="Listening">
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <rect x="9" y="3" width="6" height="11" rx="3" />
          <path d="M6 11a6 6 0 0 0 12 0M12 17v4M8.5 21h7" />
        </svg>
      </span>
    {/if}
    {#if snap?.muted}
      <span class="badge mute" title="Muted">🔇</span>
    {/if}
  </div>

  {#if snap?.session?.mode === "attached"}
    <div class="attached" title={snap.session.directory}>attached: {snap.session.project}</div>
  {/if}
  {#if line}
    <div class="line {line.kind}">{line.text}</div>
  {/if}
</main>

<style>
  main {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: flex-start;
    padding-top: 6px;
    gap: 3px;
  }
  .pet {
    position: relative;
    width: 128px;
    height: 128px;
    cursor: grab;
    flex: none;
  }
  .pet:active {
    cursor: grabbing;
  }
  .badge {
    position: absolute;
    left: 6px;
    bottom: 4px;
    font-size: 15px;
    filter: drop-shadow(0 1px 2px #000);
  }
  .mic {
    position: absolute;
    right: 0;
    top: 0;
    width: 34px;
    height: 34px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: #dc2626;
    box-shadow: 0 2px 6px rgba(0, 0, 0, 0.5);
    animation: mic-pulse 1s ease-in-out infinite;
  }
  .mic svg {
    width: 20px;
    height: 20px;
    fill: none;
    stroke: #fff;
    stroke-width: 2;
    stroke-linecap: round;
  }
  .mic rect {
    fill: #fff;
    stroke: none;
  }
  @keyframes mic-pulse {
    50% {
      box-shadow:
        0 2px 6px rgba(0, 0, 0, 0.5),
        0 0 0 7px rgba(220, 38, 38, 0.3);
    }
  }
  .line,
  .attached {
    max-width: 180px;
    padding: 2px 8px;
    border-radius: 9px;
    background: rgba(18, 22, 31, 0.88);
    border: 1px solid var(--border);
    font-size: 11px;
    line-height: 1.3;
    text-align: center;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    word-break: break-word;
  }
  .attached {
    color: var(--accent);
    -webkit-line-clamp: 1;
    line-clamp: 1;
    font-size: 10px;
  }
  .line.warn {
    border-color: #8a6a14;
    color: var(--warn);
  }
  .line.error {
    border-color: #7f3030;
    color: var(--danger);
  }
</style>
