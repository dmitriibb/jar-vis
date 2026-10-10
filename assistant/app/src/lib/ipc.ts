// Typed wrappers around the Tauri commands in src-tauri/src/ipc.rs.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AgentModel,
  Config,
  ConnectionInfo,
  DownloadProgress,
  ModelStatus,
  OpenTarget,
  PermissionReply,
  Personality,
  PetSkin,
  ProjectRef,
  SessionSummary,
  Snapshot,
  Toast,
  WizardInfo,
} from "./types";

export const api = {
  getSnapshot: () => invoke<Snapshot>("get_snapshot"),
  getConfig: () => invoke<Config>("get_config"),
  saveConfig: (config: Config) => invoke<Config>("save_config", { config }),
  resetConfig: () => invoke<Config>("reset_config"),
  setPersonality: (name: string) => invoke<void>("set_personality", { name }),
  setMuted: (muted: boolean) => invoke<void>("set_muted", { muted }),
  listAgentModels: () => invoke<AgentModel[]>("list_agent_models"),
  setModel: (model: string) => invoke<void>("set_model", { model }),

  sendText: (text: string) => invoke<void>("send_text", { text }),
  newSession: () => invoke<void>("new_session"),
  abort: () => invoke<void>("abort"),
  replyPermission: (id: string, reply: PermissionReply) => invoke<void>("reply_permission", { id, reply }),
  replayLast: () => invoke<void>("replay_last"),

  listProjects: () => invoke<ProjectRef[]>("list_projects"),
  listSessions: (projectId: string | null) => invoke<SessionSummary[]>("list_sessions", { projectId }),
  attach: (sessionId: string, directory: string) => invoke<void>("attach", { sessionId, directory }),
  detach: () => invoke<void>("detach"),
  openSessionIn: (target: OpenTarget) => invoke<void>("open_session_in", { target }),

  listModels: () => invoke<ModelStatus[]>("list_models"),
  downloadModel: (id: string) => invoke<void>("download_model", { id }),
  cancelDownload: (id: string) => invoke<void>("cancel_download", { id }),
  deleteModel: (id: string) => invoke<void>("delete_model", { id }),
  previewVoice: (id: string) => invoke<void>("preview_voice", { id }),

  startHotkeyCapture: () => invoke<string>("start_hotkey_capture"),
  cancelHotkeyCapture: () => invoke<void>("cancel_hotkey_capture"),
  listInputDevices: () => invoke<string[]>("list_input_devices"),

  listPersonalities: () => invoke<Personality[]>("list_personalities"),
  listPets: () => invoke<string[]>("list_pets"),
  getPet: (name: string) => invoke<PetSkin | null>("get_pet", { name }),
  resyncPersonas: () => invoke<void>("resync_personas"),

  togglePanel: () => invoke<void>("toggle_panel"),
  hidePanel: () => invoke<void>("hide_panel"),
  panelFocus: (focus: boolean) => invoke<void>("panel_focus", { focus }),
  petMenu: () => invoke<void>("pet_menu"),
  openSettings: (tab?: string) => invoke<void>("open_settings", { tab }),
  openPath: (what: "logs" | "config" | "home") => invoke<void>("open_path", { what }),

  connectionTest: () => invoke<ConnectionInfo>("connection_test"),
  wizardInfo: () => invoke<WizardInfo>("wizard_info"),
  openAccessibilitySettings: () => invoke<void>("open_accessibility_settings"),
};

export const on = {
  snapshot: (cb: (s: Snapshot) => void): Promise<UnlistenFn> => listen<Snapshot>("snapshot", (e) => cb(e.payload)),
  micLevel: (cb: (rms: number) => void): Promise<UnlistenFn> => listen<number>("mic-level", (e) => cb(e.payload)),
  progress: (cb: (text: string) => void): Promise<UnlistenFn> => listen<string>("progress", (e) => cb(e.payload)),
  toast: (cb: (t: Toast) => void): Promise<UnlistenFn> => listen<Toast>("toast", (e) => cb(e.payload)),
  download: (cb: (p: DownloadProgress) => void): Promise<UnlistenFn> =>
    listen<DownloadProgress>("download-progress", (e) => cb(e.payload)),
  configChanged: (cb: (c: Config) => void): Promise<UnlistenFn> => listen<Config>("config-changed", (e) => cb(e.payload)),
  settingsTab: (cb: (tab: string) => void): Promise<UnlistenFn> => listen<string>("settings-tab", (e) => cb(e.payload)),
};

const KEY_LABELS: Record<string, string> = {
  ControlRight: "Right Ctrl",
  ControlLeft: "Left Ctrl",
  MetaRight: "Right ⌘ / Win",
  MetaLeft: "Left ⌘ / Win",
  ShiftRight: "Right Shift",
  ShiftLeft: "Left Shift",
  Alt: "Left Alt",
  AltGr: "Right Alt (AltGr)",
  Return: "Enter",
  BackQuote: "Backquote",
};

/** Human label for an rdev key name, e.g. `ControlRight` -> `Right Ctrl`. */
export function keyLabel(name: string): string {
  if (!name) return "not set";
  if (KEY_LABELS[name]) return KEY_LABELS[name];
  return name.replace(/^Key([A-Z])$/, "$1").replace(/^Num(\d)$/, "$1").replace(/^Kp/, "Numpad ");
}

/** Keys that type characters on some layouts and so make poor push-to-talk keys. */
export function isRiskyKey(name: string): boolean {
  return name === "AltGr" || name === "Alt" || /^Key[A-Z]$/.test(name) || /^Num\d$/.test(name) || name === "Space";
}
