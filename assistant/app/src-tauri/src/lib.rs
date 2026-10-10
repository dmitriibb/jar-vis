//! Jar-vis: a desktop pet that is a voice front-end for opencode.

pub mod app;
pub mod audio;
pub mod backend;
pub mod commands;
pub mod config;
pub mod hotkey;
pub mod ipc;
pub mod launch;
pub mod models;
pub mod orchestrator;
pub mod personas;
pub mod pets;
pub mod reply;
pub mod schedule;
pub mod state;
pub mod stt;
pub mod tts;
pub mod windows;

use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use tauri::{Manager, RunEvent};
use tokio::sync::mpsc;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::prelude::*;

use crate::app::{App, Snapshot, ToastLevel};
use crate::backend::opencode::OpenCodeBackend;
use crate::hotkey::Hotkey;
use crate::models::ModelManager;
use crate::orchestrator::{Event, Orchestrator};
use crate::stt::SttService;
use crate::tts::TtsHandle;

/// Log to a daily-rotated file, and to the console in debug builds.
fn init_logging(level: &str) -> Option<WorkerGuard> {
    let dir = config::log_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("jarvis")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = tracing_subscriber::EnvFilter::try_new(format!("warn,jar_vis_lib={level}"))
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let file_layer = tracing_subscriber::fmt::layer().with_writer(writer).with_ansi(false);
    let registry = tracing_subscriber::registry().with(filter).with(file_layer);
    // The console layer is added by cfg, not as an `Option` layer: a `None` layer
    // reports "nothing is enabled" and silences the file log as well.
    #[cfg(debug_assertions)]
    registry.with(tracing_subscriber::fmt::layer().with_target(false)).try_init().ok()?;
    #[cfg(not(debug_assertions))]
    registry.try_init().ok()?;
    Some(guard)
}

fn setup(handle: &tauri::AppHandle, loaded: config::Loaded) -> anyhow::Result<()> {
    let cfg = loaded.config;
    if !cfg.home.trim().is_empty() {
        if let Err(e) = personas::ensure_home(&cfg.home_dir()) {
            tracing::warn!("could not prepare the home folder: {e:#}");
        }
    }

    let (tx, rx) = mpsc::unbounded_channel::<Event>();
    let tts = {
        let tx = tx.clone();
        TtsHandle::spawn(move |generation, end| {
            let _ = tx.send(Event::SpeechEnded { generation, end });
        })?
    };
    let hotkey = Hotkey::default();
    hotkey.set_key(&cfg.hotkey.key);

    let app = Arc::new(App {
        handle: handle.clone(),
        backend: OpenCodeBackend::new(cfg.opencode.clone()),
        models: Arc::new(ModelManager::new(cfg.models_dir())),
        snapshot: RwLock::new(Snapshot { state: "offline".into(), wizard_done: cfg.wizard_done, ..Default::default() }),
        config: RwLock::new(cfg.clone()),
        state: Mutex::new(state::load()),
        tx: tx.clone(),
        stt: Arc::new(SttService::default()),
        tts,
        hotkey: hotkey.clone(),
        tray_status: Mutex::new(None),
    });
    handle.manage(app.clone());

    hotkey.start(move |event| {
        let _ = tx.send(Event::Hotkey(event));
    });

    windows::create_tray(&app)?;
    windows::create_pet(&app)?;
    windows::create_panel(&app)?;
    if app.state.lock().panel_visible {
        windows::show_panel(&app);
    }
    if !cfg.wizard_done {
        windows::open_settings(handle, "wizard");
    }
    for warning in loaded.warnings {
        app.toast(ToastLevel::Warn, warning);
    }

    let orchestrator = Orchestrator::new(app.clone(), rx);
    tauri::async_runtime::spawn(async move {
        app.backend.start();
        orchestrator.run().await;
    });
    Ok(())
}

pub fn run() {
    let loaded = config::load();
    let _log_guard = init_logging(&loaded.config.log.level);
    tracing::info!("Jar-vis {} starting, config at {}", env!("CARGO_PKG_VERSION"), config::config_path().display());

    let mut loaded = Some(loaded);
    let app = tauri::Builder::default()
        // A second launch only brings the pet back.
        .plugin(tauri_plugin_single_instance::init(|handle, _args, _cwd| {
            if let Some(pet) = handle.get_webview_window(windows::PET) {
                let _ = pet.show();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            ipc::get_snapshot,
            ipc::get_config,
            ipc::save_config,
            ipc::reset_config,
            ipc::set_personality,
            ipc::set_muted,
            ipc::list_agent_models,
            ipc::set_model,
            ipc::send_text,
            ipc::new_session,
            ipc::abort,
            ipc::reply_permission,
            ipc::replay_last,
            ipc::cancel_task,
            ipc::list_projects,
            ipc::list_sessions,
            ipc::attach,
            ipc::detach,
            ipc::open_session_in,
            ipc::list_models,
            ipc::download_model,
            ipc::cancel_download,
            ipc::delete_model,
            ipc::preview_voice,
            ipc::start_hotkey_capture,
            ipc::cancel_hotkey_capture,
            ipc::list_input_devices,
            ipc::list_personalities,
            ipc::list_pets,
            ipc::get_pet,
            ipc::resync_personas,
            ipc::toggle_panel,
            ipc::hide_panel,
            ipc::panel_focus,
            ipc::copy_text,
            ipc::pet_menu,
            ipc::open_settings,
            ipc::open_path,
            ipc::connection_test,
            ipc::wizard_info,
            ipc::open_accessibility_settings,
        ])
        .on_menu_event(|handle, event| windows::on_menu_event(handle, event.id().as_ref()))
        .setup(move |app| {
            // No Dock icon on macOS: Jar-vis lives in the menu bar.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let loaded = loaded.take().expect("setup runs once");
            setup(app.handle(), loaded)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Jar-vis");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            if let Some(app) = handle.try_state::<Arc<App>>() {
                state::save(&app.state.lock());
            }
        }
    });
}
