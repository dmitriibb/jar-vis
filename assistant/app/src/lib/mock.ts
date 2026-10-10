// Browser preview for UI work: `npm run dev`, then open /pet.html, /panel.html or
// /settings.html in a normal browser. Loaded only in dev builds outside Tauri.
// Drive it from the console, e.g. `jarvisMock.set({ state: "working", detail: "bash: npm test" })`.

import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { Config, ModelStatus, Snapshot } from "./types";

const now = Date.now();

let snapshot: Snapshot = {
  state: "idle",
  detail: null,
  muted: false,
  backend: { version: "2.0.26", url: "http://127.0.0.1:49374" },
  session: { id: "ses_mock123", title: "Checking the build", directory: "C:/projects/jar-vis/sessions", mode: "workspace", project: "sessions" },
  history: [
    { transcript: "what's two plus two", summary: "Four.", details: "", pending: false, scheduled: false, error: null, time: now - 300_000 },
    {
      transcript: "run the tests and tell me what failed",
      summary: "All 60 tests pass. Nothing failed.",
      details: "## Details\n\n- `cargo test --lib`: **60 passed**\n- Took 0.07 s\n\n```bash\ncargo test --lib\n```",
      pending: false,
      scheduled: true,
      error: null,
      time: now - 60_000,
    },
  ],
  permission: null,
  queued: 0,
  tasks: [
    {
      id: "task_1",
      session: { id: "ses_mock123", directory: "C:/projects/jar-vis/sessions" },
      sessionTitle: "Checking the build",
      prompt: "Check whether the payment service is running and report its status.",
      created: now - 600_000,
      nextRun: now + 12 * 60_000,
      everyMinutes: 15,
      runs: 2,
      maxRuns: null,
      expires: now + 86_400_000,
    },
    {
      id: "task_2",
      session: { id: "ses_mock456", directory: "C:/projects/jar-vis/sessions" },
      sessionTitle: "Deploy watch",
      prompt: "Check if the deploy finished.",
      created: now - 60_000,
      nextRun: now + 58 * 60_000,
      everyMinutes: null,
      runs: 0,
      maxRuns: null,
      expires: null,
    },
  ],
  personality: "jarvis",
  model: "openai/gpt-6.1-sol",
  pet: "default",
  hotkey: "ControlRight",
  hotkeyListening: true,
  sleepAfterMinutes: 10,
  streamText: false,
  wizardDone: true,
};

let config: Config = {
  home: "C:/projects/jar-vis",
  wizard_done: true,
  hotkey: { key: "ControlRight", cancel_on_chord: true, suppress: false },
  audio: { input_device: "", tts_enabled: true },
  stt: { model: "base.en-q5_1", preload: true, unload_after_idle_minutes: 0, min_seconds: 0.35, max_seconds: 120, vocabulary: "opencode, Tauri, Rust" },
  tts: { voice: "piper-en_US-lessac-medium", speed: 1, max_spoken_chars: 400, unload_after_idle_minutes: 30 },
  opencode: { url: "", autostart: true, stop_on_exit: false, binary: "", username: "opencode", password: "", workspace_dir: "", model: "" },
  persona: { active: "jarvis", pet: "default" },
  attach: { override_agent: "" },
  openchamber: { url: "", app_path: "" },
  commands: { wake_word: "jarvis", new_session: [], stop: [], mute: [], unmute: [], repeat: [] },
  ui: { panel_auto_show: true, panel_auto_hide_seconds: 0, stream_text: false, sleep_after_minutes: 10 },
  models: { dir: "" },
  log: { level: "info" },
};

const model = (id: string, kind: "stt" | "tts", display: string, sizeMb: number, installed: boolean): ModelStatus => ({
  id,
  kind,
  engine: kind === "stt" ? "whisper" : "piper",
  display,
  sizeMb,
  ramMbEst: sizeMb * 2,
  license: "MIT",
  installed,
  downloading: false,
});

const models = [
  model("tiny.en-q5_1", "stt", "Whisper tiny.en (fastest, weakest)", 32, false),
  model("base.en-q5_1", "stt", "Whisper base.en (default)", 60, true),
  model("small.en-q5_1", "stt", "Whisper small.en (recommended upgrade)", 190, false),
  model("piper-en_US-lessac-medium", "tts", "Lessac (US, default)", 67, true),
  model("piper-en_GB-alan-medium", "tts", "Alan (British)", 67, false),
];

function set(patch: Partial<Snapshot>) {
  snapshot = { ...snapshot, ...patch };
  emit("snapshot", snapshot);
}

export function install() {
  mockWindows("main");
  mockIPC(
    (cmd, payload) => {
      const args = (payload ?? {}) as Record<string, unknown>;
      switch (cmd) {
        case "get_snapshot":
          return snapshot;
        case "get_config":
          return config;
        case "save_config":
          config = args.config as Config;
          return config;
        case "set_muted":
          return set({ muted: args.muted as boolean });
        case "cancel_task":
          return set({ tasks: snapshot.tasks.filter((t) => t.id !== args.id) });
        case "attach":
          return set({
            session: { ...snapshot.session!, id: String(args.sessionId), title: "Deploy watch" },
            history: [],
          });
        case "copy_text":
          console.debug("[mock] copied", args.text);
          return;
        case "list_agent_models":
          return [
            { key: "openai/gpt-6.1-sol", name: "GPT-6.1 Sol", provider: "openai" },
            { key: "openai/gpt-6-luna", name: "GPT-6 Luna", provider: "openai" },
            { key: "openai/gpt-5.4-mini", name: "GPT-5.4 mini", provider: "openai" },
            { key: "anthropic/claude-sonnet-5-5", name: "Claude Sonnet 5.5", provider: "anthropic" },
          ];
        case "set_model":
          return set({ model: args.model as string });
        case "list_models":
          return models;
        case "list_input_devices":
          return ["Microphone (USB Audio)", "Headset (Bluetooth)"];
        case "list_personalities":
          return [
            { name: "jarvis", displayName: "Jarvis", description: "Calm, concise voice assistant", voice: "piper-en_US-lessac-medium", speed: 1, pet: "default" },
            { name: "pixel", displayName: "Pixel", description: "A cheerful pixel-cat assistant", voice: "piper-en_US-amy-low", speed: 1.05, pet: "default" },
          ];
        case "list_pets":
          return ["default"];
        case "get_pet":
          return null;
        case "list_projects":
          return [{ id: "p1", name: "jar-vis", directory: "C:/projects/jar-vis" }];
        case "list_sessions":
          return [
            { id: "ses_mock123", title: "Checking the build", directory: "C:/projects/jar-vis/sessions", agent: "jarvis", updated: now - 60_000 },
            { id: "ses_mock456", title: "Deploy watch", directory: "C:/projects/jar-vis/sessions", agent: "jarvis", updated: now - 7_200_000 },
          ];
        case "wizard_info":
          return { suggestedHome: "C:/Users/you/jar-vis", defaultHotkey: "ControlRight", platform: "windows", accessibility: true, devBuild: true };
        case "connection_test":
          return { connected: true, info: snapshot.backend, binary: "C:/tools/opencode.cmd", error: null };
        case "send_text":
          set({
            state: "working",
            detail: "bash: npm test",
            history: [...snapshot.history, { transcript: String(args.text), summary: "", details: "", pending: true, scheduled: false, error: null, time: Date.now() }],
          });
          return;
        default:
          console.debug("[mock] unhandled command", cmd, args);
          return null;
      }
    },
    { shouldMockEvents: true },
  );
  Object.assign(window, { jarvisMock: { set, get: () => snapshot } });
}
