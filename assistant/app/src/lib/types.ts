// These mirror the Rust serde structs in src-tauri/src (app.rs, backend/mod.rs,
// models/mod.rs, personas.rs). IPC structs are camelCase; `Config` keeps the
// snake_case keys of config.toml.

export type PetState =
  | "offline"
  | "idle"
  | "listening"
  | "transcribing"
  | "sending"
  | "working"
  | "permission"
  | "speaking"
  | "error";

export interface BackendInfo {
  version: string;
  url: string;
}

export interface SessionView {
  agent: string | null;
  id: string;
  title: string | null;
  directory: string;
  mode: "workspace" | "attached";
  project: string;
}

export interface Exchange {
  transcript: string;
  summary: string;
  details: string;
  pending: boolean;
  /** The prompt came from the scheduler. */
  scheduled: boolean;
  error: string | null;
  time: number;
}

export interface PermissionRequest {
  id: string;
  session: string;
  kind: string;
  title: string;
  patterns: string[];
}

export type PermissionReply = "once" | "always" | "reject";

export interface Snapshot {
  state: PetState;
  detail: string | null;
  muted: boolean;
  backend: BackendInfo | null;
  session: SessionView | null;
  history: Exchange[];
  permission: PermissionRequest | null;
  queued: number;
  /** Scheduled prompts of every session, soonest first. */
  tasks: ScheduledTask[];
  personality: string;
  /** `provider/id` of the model the next turn runs on. */
  model: string | null;
  pet: string;
  hotkey: string;
  hotkeyListening: boolean;
  sleepAfterMinutes: number;
  petMicButton: boolean;
  streamText: boolean;
  wizardDone: boolean;
}

export interface ScheduledTask {
  id: string;
  session: { id: string; directory: string };
  sessionTitle: string | null;
  prompt: string;
  /** Milliseconds since the Unix epoch. */
  created: number;
  nextRun: number;
  /** Set for repeating tasks. */
  everyMinutes: number | null;
  runs: number;
  maxRuns: number | null;
  expires: number | null;
}

export interface Toast {
  level: "info" | "warn" | "error";
  text: string;
}

export interface Config {
  home: string;
  wizard_done: boolean;
  hotkey: { key: string; cancel_on_chord: boolean; suppress: boolean };
  audio: { input_device: string; tts_enabled: boolean };
  stt: {
    model: string;
    preload: boolean;
    unload_after_idle_minutes: number;
    min_seconds: number;
    max_seconds: number;
    vocabulary: string;
  };
  tts: { voice: string; speed: number; max_spoken_chars: number; unload_after_idle_minutes: number };
  opencode: {
    url: string;
    autostart: boolean;
    stop_on_exit: boolean;
    binary: string;
    username: string;
    password: string;
    workspace_dir: string;
    agent: string;
    model: string;
  };
  persona: { active: string; pet: string };
  attach: { override_agent: string };
  openchamber: { url: string; app_path: string };
  commands: {
    wake_word: string;
    new_session: string[];
    stop: string[];
    mute: string[];
    unmute: string[];
    repeat: string[];
  };
  ui: {
    panel_auto_show: boolean;
    panel_auto_hide_seconds: number;
    stream_text: boolean;
    sleep_after_minutes: number;
    pet_mic_button: boolean;
  };
  models: { dir: string };
  log: { level: string };
}

export interface ModelStatus {
  id: string;
  kind: "stt" | "tts";
  engine: string;
  display: string;
  sizeMb: number;
  ramMbEst: number;
  license: string;
  installed: boolean;
  downloading: boolean;
}

export interface DownloadProgress {
  modelId: string;
  bytes: number;
  total: number;
  done: boolean;
  error: string | null;
}

export interface Personality {
  name: string;
  displayName: string;
  description: string;
  voice: string | null;
  speed: number | null;
  pet: string | null;
}

export interface PetAnimation {
  /** A data URL. */
  src: string;
  frames?: number;
  fps?: number;
  loop?: boolean;
  micLevel?: boolean;
}

export interface PetSkin {
  name: string;
  size: number;
  states: Record<string, PetAnimation>;
}

export interface AgentModel {
  /** `provider/id`. */
  key: string;
  name: string;
  provider: string;
}

export interface ProjectRef {
  id: string;
  name: string;
  directory: string;
}

export interface SessionSummary {
  id: string;
  title: string | null;
  directory: string;
  agent: string | null;
  updated: number;
}

export interface ConnectionInfo {
  connected: boolean;
  info: BackendInfo | null;
  binary: string | null;
  error: string | null;
}

export interface WizardInfo {
  suggestedHome: string;
  defaultHotkey: string;
  platform: string;
  accessibility: boolean;
  devBuild: boolean;
}

export type OpenTarget = "web" | "tui" | "openchamber" | "copy";
