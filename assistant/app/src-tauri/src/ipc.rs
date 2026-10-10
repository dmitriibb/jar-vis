//! Commands the windows can call. Event names emitted to the windows:
//! `snapshot`, `mic-level`, `progress`, `download-progress`, `toast`, `settings-tab`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde::Serialize;
use tauri::{Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;

use crate::app::{App, Snapshot, ToastLevel};
use crate::audio::capture;
use crate::backend::{AgentBackend, BackendInfo, ModelInfo, PermissionReply, ProjectRef, SessionRef, SessionSummary};
use crate::config::{self, Config};
use crate::launch::{self, OpenchamberTarget};
use crate::models::ModelStatus;
use crate::orchestrator::Event;
use crate::personas::{self, Personality};
use crate::windows;

type Shared<'a> = State<'a, Arc<App>>;
/// Errors cross the IPC boundary as plain strings.
type CmdResult<T> = std::result::Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

// ---- status and config ------------------------------------------------------

#[tauri::command]
pub fn get_snapshot(app: Shared) -> Snapshot {
    app.snapshot.read().clone()
}

#[tauri::command]
pub fn get_config(app: Shared) -> Config {
    app.config.read().clone()
}

/// Validate, persist and apply a config. Returns what was actually saved.
#[tauri::command]
pub fn save_config(app: Shared, config: Config) -> CmdResult<Config> {
    apply_config(&app, config).map_err(err)
}

pub fn apply_config(app: &Arc<App>, mut new: Config) -> Result<Config> {
    for warning in new.validate() {
        app.toast(ToastLevel::Warn, warning);
    }
    if !new.home.trim().is_empty() {
        personas::ensure_home(&new.home_dir())?;
    }
    config::save(&new)?;
    let old = std::mem::replace(&mut *app.config.write(), new.clone());
    if old != new {
        app.send(Event::ConfigChanged(Box::new(old)));
        let _ = app.handle.emit("config-changed", &new);
    }
    Ok(new)
}

#[tauri::command]
pub fn reset_config(app: Shared) -> CmdResult<Config> {
    let current = app.config.read().clone();
    // Keep what the wizard established, reset everything else.
    let fresh = Config {
        home: current.home,
        wizard_done: current.wizard_done,
        hotkey: config::HotkeyCfg { key: current.hotkey.key, ..Default::default() },
        ..Default::default()
    };
    apply_config(&app, fresh).map_err(err)
}

pub fn apply_personality(app: &Arc<App>, name: &str) {
    let mut cfg = app.config.read().clone();
    if cfg.persona.active == name {
        // A check item toggles itself off when clicked; put the menu back.
        windows::rebuild_tray_menu(app);
        return;
    }
    cfg.persona.active = name.to_string();
    if let Some(pet) = personas::find(&cfg, name).and_then(|p| p.pet) {
        cfg.persona.pet = pet;
    }
    if let Err(e) = apply_config(app, cfg) {
        app.toast(ToastLevel::Error, format!("Could not switch personality: {e:#}"));
    }
}

#[tauri::command]
pub fn set_personality(app: Shared, name: String) {
    apply_personality(&app, &name);
}

/// The models opencode offers for the current session's directory.
#[tauri::command]
pub async fn list_agent_models(app: Shared<'_>) -> CmdResult<Vec<ModelInfo>> {
    let dir = app
        .state
        .lock()
        .current_session
        .as_ref()
        .map(|s| s.directory.clone())
        .unwrap_or_else(|| app.config.read().workspace_dir());
    app.backend.list_models(&dir).await.map_err(err)
}

/// `model` is `provider/id`.
#[tauri::command]
pub fn set_model(app: Shared, model: String) {
    app.send(Event::SetModel(model));
}

#[tauri::command]
pub fn set_muted(app: Shared, muted: bool) {
    app.send(Event::SetMuted(muted));
}

// ---- turns ------------------------------------------------------------------

/// Send typed text through the same path as a spoken transcript.
#[tauri::command]
pub fn send_text(app: Shared, text: String) {
    app.send(Event::Transcript { text, typed: true });
}

#[tauri::command]
pub fn new_session(app: Shared) {
    app.send(Event::NewSession);
}

#[tauri::command]
pub fn abort(app: Shared) {
    app.send(Event::Abort);
}

#[tauri::command]
pub fn reply_permission(app: Shared, id: String, reply: PermissionReply) {
    app.send(Event::ReplyPermission { id, reply });
}

#[tauri::command]
pub fn replay_last(app: Shared) {
    app.send(Event::ReplayLast);
}

// ---- sessions ---------------------------------------------------------------

#[tauri::command]
pub async fn list_projects(app: Shared<'_>) -> CmdResult<Vec<ProjectRef>> {
    app.backend.list_projects().await.map_err(err)
}

/// Sessions of one project, or of the Jar-vis workspace when `project_id` is None.
#[tauri::command]
pub async fn list_sessions(app: Shared<'_>, project_id: Option<String>) -> CmdResult<Vec<SessionSummary>> {
    match project_id {
        Some(id) => app.backend.list_project_sessions(&id).await,
        None => {
            let dir = app.config.read().workspace_dir();
            app.backend.list_sessions(&dir).await
        }
    }
    .map_err(err)
}

#[tauri::command]
pub fn attach(app: Shared, session_id: String, directory: PathBuf) {
    app.send(Event::Attach(SessionRef { id: session_id, directory }));
}

#[tauri::command]
pub fn detach(app: Shared) {
    app.send(Event::Detach);
}

/// `target` is `web`, `tui`, `openchamber` or `copy`.
pub async fn open_session(app: &Arc<App>, target: &str) -> Result<()> {
    let session = app.state.lock().current_session.clone().ok_or_else(|| anyhow!("there is no session yet"))?;
    let cfg = app.config.read().clone();
    let copy_id = || app.handle.clipboard().write_text(session.id.clone()).map_err(|e| anyhow!("clipboard: {e}"));
    match target {
        "copy" => {
            copy_id()?;
            app.toast(ToastLevel::Info, "Session ID copied.");
        }
        "tui" => launch::open_in_tui(&session, &cfg.opencode.binary)?,
        "web" => {
            let url = app.backend.web_url().await?;
            app.handle.opener().open_url(url, None::<&str>).map_err(|e| anyhow!("{e}"))?;
            copy_id()?;
        }
        "openchamber" => {
            // openchamber has no documented deep link, so the ID goes to the clipboard.
            copy_id()?;
            match launch::openchamber_target(&cfg.openchamber) {
                Some(OpenchamberTarget::Url(url)) => {
                    app.handle.opener().open_url(url, None::<&str>).map_err(|e| anyhow!("{e}"))?
                }
                Some(OpenchamberTarget::App(path)) => launch::launch_app(&path)?,
                None => return Err(anyhow!("Set the openchamber URL or app path in Settings → OpenCode first")),
            }
            app.toast(ToastLevel::Info, "Session ID copied — paste it in openchamber.");
        }
        other => return Err(anyhow!("unknown target \"{other}\"")),
    }
    Ok(())
}

#[tauri::command]
pub async fn open_session_in(app: Shared<'_>, target: String) -> CmdResult<()> {
    open_session(&app, &target).await.map_err(err)
}

// ---- models -----------------------------------------------------------------

#[tauri::command]
pub fn list_models(app: Shared) -> Vec<ModelStatus> {
    app.models.list()
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProgress {
    model_id: String,
    bytes: u64,
    total: u64,
    done: bool,
    error: Option<String>,
}

/// Resolves when the download has finished. Progress arrives as `download-progress` events.
#[tauri::command]
pub async fn download_model(app: Shared<'_>, id: String) -> CmdResult<()> {
    let app = app.inner().clone();
    let handle = app.handle.clone();
    let model_id = id.clone();
    let result = app
        .models
        .download(&id, move |bytes, total| {
            let _ = handle.emit(
                "download-progress",
                DownloadProgress { model_id: model_id.clone(), bytes, total, done: false, error: None },
            );
        })
        .await;
    let error = result.as_ref().err().map(|e| format!("{e:#}"));
    let _ = app.handle.emit(
        "download-progress",
        DownloadProgress { model_id: id.clone(), bytes: 0, total: 0, done: true, error: error.clone() },
    );
    if result.is_ok() {
        // A freshly installed model can be loaded right away.
        app.send(Event::ModelsChanged);
    }
    result.map_err(err)
}

#[tauri::command]
pub fn cancel_download(app: Shared, id: String) {
    app.models.cancel_download(&id);
}

#[tauri::command]
pub fn delete_model(app: Shared, id: String) -> CmdResult<()> {
    let cfg = app.config.read().clone();
    if cfg.stt.model == id {
        app.stt.unload();
    }
    if cfg.tts.voice == id {
        app.tts.unload();
    }
    app.models.delete(&id).map_err(err)
}

#[tauri::command]
pub fn preview_voice(app: Shared, id: String) -> CmdResult<()> {
    let voice = app.models.voice(&id).map_err(err)?;
    let speed = app.config.read().tts.speed;
    app.tts.speak("Hello, I am Jar-vis. This is how I sound.", voice, speed);
    Ok(())
}

// ---- hotkey and devices -------------------------------------------------------

/// Resolves with the name of the next key pressed anywhere.
#[tauri::command]
pub async fn start_hotkey_capture(app: Shared<'_>) -> CmdResult<String> {
    if !app.hotkey.is_listening() {
        return Err("The global key listener is not running. On macOS, grant Accessibility access and restart Jar-vis.".into());
    }
    let rx = app.hotkey.record_next();
    match tokio::time::timeout(std::time::Duration::from_secs(20), rx).await {
        Ok(Ok(name)) => Ok(name),
        _ => {
            app.hotkey.cancel_recording();
            Err("No key was pressed.".into())
        }
    }
}

#[tauri::command]
pub fn cancel_hotkey_capture(app: Shared) {
    app.hotkey.cancel_recording();
}

#[tauri::command]
pub fn list_input_devices() -> Vec<String> {
    capture::list_input_devices()
}

// ---- personalities and pets ---------------------------------------------------

#[tauri::command]
pub fn list_personalities(app: Shared) -> Vec<Personality> {
    personas::list(&app.config.read())
}

#[tauri::command]
pub fn list_pets(app: Shared) -> Vec<String> {
    let dir = app.config.read().pets_dir();
    let mut pets: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().join("pet.json").is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    if !pets.iter().any(|p| p == "default") {
        pets.push("default".into());
    }
    pets.sort();
    pets
}

/// The images of a custom pet skin, or None for the built-in pet.
#[tauri::command]
pub fn get_pet(app: Shared, name: String) -> CmdResult<Option<crate::pets::Skin>> {
    let dir = app.config.read().pets_dir();
    crate::pets::load(&dir, &name).map_err(err)
}

#[tauri::command]
pub fn resync_personas(app: Shared) {
    app.send(Event::ResyncPersonas);
}

// ---- windows ------------------------------------------------------------------

#[tauri::command]
pub fn toggle_panel(app: Shared) {
    windows::toggle_panel(&app);
}

#[tauri::command]
pub fn hide_panel(app: Shared) {
    windows::hide_panel(&app);
}

/// The panel never takes focus on its own. It becomes focusable only while the
/// user types into it.
#[tauri::command]
pub fn panel_focus(app: Shared, focus: bool) {
    if let Some(panel) = app.handle.get_webview_window(windows::PANEL) {
        let _ = panel.set_focusable(focus);
        if focus {
            let _ = panel.set_focus();
        }
    }
}

#[tauri::command]
pub fn pet_menu(app: Shared) {
    windows::popup_menu(&app);
}

#[tauri::command]
pub fn open_settings(app: Shared, tab: Option<String>) {
    windows::open_settings(&app.handle, tab.as_deref().unwrap_or("general"));
}

/// `what` is `logs`, `config` or `home`.
#[tauri::command]
pub fn open_path(app: Shared, what: String) -> CmdResult<()> {
    let path = match what.as_str() {
        "logs" => config::log_dir(),
        "config" => config::config_path(),
        "home" => app.config.read().home_dir(),
        other => return Err(format!("unknown path \"{other}\"")),
    };
    app.handle.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

// ---- opencode and the wizard ----------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    connected: bool,
    info: Option<BackendInfo>,
    binary: Option<String>,
    error: Option<String>,
}

/// Try to reach opencode now, starting its service if allowed.
#[tauri::command]
pub async fn connection_test(app: Shared<'_>) -> CmdResult<ConnectionInfo> {
    let binary = launch::find_opencode(&app.config.read().opencode.binary).map(|p| p.to_string_lossy().into_owned());
    let result = app.backend.connect().await;
    app.backend.retry_now();
    Ok(match result {
        Ok(info) => ConnectionInfo { connected: true, info: Some(info), binary, error: None },
        Err(e) => ConnectionInfo { connected: false, info: None, binary, error: Some(format!("{e:#}")) },
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WizardInfo {
    suggested_home: String,
    default_hotkey: String,
    platform: &'static str,
    /// macOS only: whether Jar-vis may observe global key events.
    accessibility: bool,
    dev_build: bool,
}

#[tauri::command]
pub fn wizard_info(app: Shared) -> WizardInfo {
    let home = app.config.read().home.clone();
    WizardInfo {
        suggested_home: if home.trim().is_empty() {
            config::suggested_home().to_string_lossy().into_owned()
        } else {
            home
        },
        default_hotkey: config::default_hotkey().to_string(),
        platform: std::env::consts::OS,
        accessibility: accessibility_granted(),
        dev_build: cfg!(debug_assertions),
    }
}

#[cfg(target_os = "macos")]
pub fn accessibility_granted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
    }
    unsafe { AXIsProcessTrusted() != 0 }
}

#[cfg(not(target_os = "macos"))]
pub fn accessibility_granted() -> bool {
    true
}

/// Open the macOS privacy pane where Accessibility access is granted.
#[tauri::command]
pub fn open_accessibility_settings(app: Shared) -> CmdResult<()> {
    app.handle
        .opener()
        .open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility", None::<&str>)
        .map_err(|e| e.to_string())
}
