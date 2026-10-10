//! State shared between the orchestrator, the IPC commands and the windows.

use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use tauri::menu::MenuItem;
use tauri::{AppHandle, Emitter, Wry};
use tokio::sync::mpsc;

use crate::backend::opencode::OpenCodeBackend;
use crate::backend::{BackendInfo, PermissionRequest};
use crate::config::Config;
use crate::hotkey::Hotkey;
use crate::models::ModelManager;
use crate::orchestrator::Event;
use crate::state::PersistedState;
use crate::stt::SttService;
use crate::tts::TtsHandle;

pub const MAX_HISTORY: usize = 50;

pub struct App {
    pub handle: AppHandle,
    pub config: RwLock<Config>,
    pub state: Mutex<PersistedState>,
    pub backend: Arc<OpenCodeBackend>,
    /// Everything that changes orchestrator state goes through this channel.
    pub tx: mpsc::UnboundedSender<Event>,
    pub stt: Arc<SttService>,
    pub tts: TtsHandle,
    pub hotkey: Hotkey,
    pub models: Arc<ModelManager>,
    /// What the windows render. Replaced as a whole on every change.
    pub snapshot: RwLock<Snapshot>,
    /// The disabled status line at the top of the tray menu.
    pub tray_status: Mutex<Option<MenuItem<Wry>>>,
}

impl App {
    pub fn send(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    pub fn toast(&self, level: ToastLevel, text: impl Into<String>) {
        let text = text.into();
        match level {
            ToastLevel::Error => tracing::error!("{text}"),
            ToastLevel::Warn => tracing::warn!("{text}"),
            ToastLevel::Info => tracing::info!("{text}"),
        }
        let _ = self.handle.emit("toast", Toast { level, text });
    }

    /// Update the persisted state and write it to disk.
    pub fn update_state(&self, change: impl FnOnce(&mut PersistedState)) {
        let mut state = self.state.lock();
        change(&mut state);
        crate::state::save(&state);
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToastLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct Toast {
    pub level: ToastLevel,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// `offline | idle | listening | transcribing | sending | working |
    /// permission | speaking | error`.
    pub state: String,
    /// The tool being run, the error message, or why opencode is offline.
    pub detail: Option<String>,
    pub muted: bool,
    pub backend: Option<BackendInfo>,
    pub session: Option<SessionView>,
    pub history: Vec<Exchange>,
    pub permission: Option<PermissionRequest>,
    /// Prompts recorded while a turn was running, waiting to be sent.
    pub queued: usize,
    pub personality: String,
    /// The model the next turn runs on, as `provider/id`, when known.
    pub model: Option<String>,
    pub pet: String,
    pub hotkey: String,
    /// False when the OS refused the global key hook.
    pub hotkey_listening: bool,
    pub sleep_after_minutes: u32,
    pub stream_text: bool,
    pub wizard_done: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub id: String,
    pub title: Option<String>,
    pub directory: String,
    /// `workspace` or `attached`.
    pub mode: String,
    /// Folder name of the session's project, shown on the "attached" badge.
    pub project: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exchange {
    pub transcript: String,
    pub summary: String,
    pub details: String,
    /// True until the answer arrives.
    pub pending: bool,
    pub error: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub time: i64,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}
