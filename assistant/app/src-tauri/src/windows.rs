//! The pet, panel and settings windows, and the tray menu.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent, Wry};

use crate::app::{App, Snapshot, ToastLevel};
use crate::orchestrator::Event;
use crate::{ipc, personas};

pub const PET: &str = "pet";
pub const PANEL: &str = "panel";
pub const SETTINGS: &str = "settings";
const TRAY: &str = "main";

const PET_SIZE: (f64, f64) = (190.0, 210.0);
const PANEL_SIZE: (f64, f64) = (390.0, 440.0);
const MARGIN: i32 = 24;

pub fn create_pet(app: &Arc<App>) -> tauri::Result<WebviewWindow> {
    let handle = &app.handle;
    let window = WebviewWindowBuilder::new(handle, PET, WebviewUrl::App("pet.html".into()))
        .title("Jar-vis")
        .inner_size(PET_SIZE.0, PET_SIZE.1)
        .transparent(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        // The pet must never take keyboard focus from the app the user works in.
        .focused(false)
        .focusable(false)
        .visible_on_all_workspaces(true)
        .visible(false)
        .build()?;

    let saved = app.state.lock().pet_position;
    let position = saved.filter(|p| is_on_screen(&window, *p)).or_else(|| default_pet_position(&window));
    if let Some((x, y)) = position {
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }

    // Persist the position after a drag settles, and keep the panel next to the pet.
    let moves = Arc::new(AtomicU64::new(0));
    let app_for_events = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Moved(position) = event {
            let app = app_for_events.clone();
            let position = (position.x, position.y);
            let generation = moves.fetch_add(1, Ordering::SeqCst) + 1;
            let moves = moves.clone();
            position_panel(&app);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(600)).await;
                if moves.load(Ordering::SeqCst) == generation {
                    app.update_state(|s| s.pet_position = Some(position));
                }
            });
        }
    });
    window.show()?;
    Ok(window)
}

fn is_on_screen(window: &WebviewWindow, (x, y): (i32, i32)) -> bool {
    window.available_monitors().is_ok_and(|monitors| {
        monitors.iter().any(|m| {
            let (pos, size) = (m.position(), m.size());
            x >= pos.x - 40 && y >= pos.y - 40 && x < pos.x + size.width as i32 - 60 && y < pos.y + size.height as i32 - 60
        })
    })
}

/// Bottom-right corner of the primary monitor's work area.
fn default_pet_position(window: &WebviewWindow) -> Option<(i32, i32)> {
    let monitor = window.primary_monitor().ok().flatten()?;
    let area = monitor.work_area();
    let size = window.outer_size().ok()?;
    Some((
        area.position.x + area.size.width as i32 - size.width as i32 - MARGIN,
        area.position.y + area.size.height as i32 - size.height as i32 - MARGIN,
    ))
}

pub fn create_panel(app: &Arc<App>) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(&app.handle, PANEL, WebviewUrl::App("panel.html".into()))
        .title("Jar-vis")
        .inner_size(PANEL_SIZE.0, PANEL_SIZE.1)
        .min_inner_size(300.0, 200.0)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .focused(false)
        // Focus is granted only while the user types into the panel (see `panel_focus`).
        .focusable(false)
        .visible_on_all_workspaces(true)
        .visible(false)
        .build()
}

/// Put the panel beside the pet: to the left when the pet is on the right half
/// of its monitor, else to the right. Bottoms are aligned.
pub fn position_panel(app: &App) {
    let (Some(pet), Some(panel)) = (app.handle.get_webview_window(PET), app.handle.get_webview_window(PANEL)) else {
        return;
    };
    let (Ok(pet_pos), Ok(pet_size), Ok(panel_size)) = (pet.outer_position(), pet.outer_size(), panel.outer_size()) else {
        return;
    };
    let Some(monitor) = pet.current_monitor().ok().flatten().or_else(|| pet.primary_monitor().ok().flatten()) else {
        return;
    };
    let area = monitor.work_area();
    let (left, top) = (area.position.x, area.position.y);
    let (right, bottom) = (left + area.size.width as i32, top + area.size.height as i32);
    let pet_center = pet_pos.x + pet_size.width as i32 / 2;

    let mut x = if pet_center > (left + right) / 2 {
        pet_pos.x - panel_size.width as i32 - 8
    } else {
        pet_pos.x + pet_size.width as i32 + 8
    };
    let mut y = pet_pos.y + pet_size.height as i32 - panel_size.height as i32;
    x = x.clamp(left, (right - panel_size.width as i32).max(left));
    y = y.clamp(top, (bottom - panel_size.height as i32).max(top));
    let _ = panel.set_position(PhysicalPosition::new(x, y));
}

pub fn show_panel(app: &App) {
    if let Some(panel) = app.handle.get_webview_window(PANEL) {
        if !panel.is_visible().unwrap_or(false) {
            position_panel(app);
            let _ = panel.show();
            app.update_state(|s| s.panel_visible = true);
        }
    }
}

pub fn hide_panel(app: &App) {
    if let Some(panel) = app.handle.get_webview_window(PANEL) {
        let _ = panel.set_focusable(false);
        let _ = panel.hide();
        app.update_state(|s| s.panel_visible = false);
    }
}

pub fn toggle_panel(app: &App) {
    let visible = app.handle.get_webview_window(PANEL).is_some_and(|p| p.is_visible().unwrap_or(false));
    if visible {
        hide_panel(app);
    } else {
        show_panel(app);
    }
}

pub fn toggle_pet(app: &App) {
    if let Some(pet) = app.handle.get_webview_window(PET) {
        if pet.is_visible().unwrap_or(false) {
            let _ = pet.hide();
            hide_panel(app);
        } else {
            let _ = pet.show();
        }
    }
}

/// Open Settings on a tab (`general`, `sessions`, `wizard`, ...). The window is
/// created on demand and destroyed on close, to keep the idle footprint small.
pub fn open_settings(handle: &AppHandle, tab: &str) {
    if let Some(window) = handle.get_webview_window(SETTINGS) {
        let _ = handle.emit_to(SETTINGS, "settings-tab", tab);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let url = format!("settings.html#{tab}");
    let built = WebviewWindowBuilder::new(handle, SETTINGS, WebviewUrl::App(url.into()))
        .title("Jar-vis Settings")
        .inner_size(780.0, 600.0)
        .min_inner_size(640.0, 460.0)
        .center()
        .build();
    if let Err(e) = built {
        tracing::error!("could not open settings: {e}");
    }
}

// ---- tray -----------------------------------------------------------------

fn build_menu(app: &App) -> tauri::Result<Menu<Wry>> {
    let h = &app.handle;
    let cfg = app.config.read().clone();
    let (attached, has_session) = {
        let state = app.state.lock();
        (state.attached, state.current_session.is_some())
    };
    let item = |id: &str, text: &str, enabled: bool| MenuItem::with_id(h, id, text, enabled, None::<&str>);
    let sep = || PredefinedMenuItem::separator(h);

    let status = item("status", &status_text(&app.snapshot.read()), false)?;
    *app.tray_status.lock() = Some(status.clone());

    let open_in = Submenu::with_items(
        h,
        "Open current session in",
        has_session,
        &[
            &item("open:web", "opencode web", true)?,
            &item("open:tui", "opencode TUI", true)?,
            &item("open:openchamber", "openchamber", true)?,
            &sep()?,
            &item("open:copy", "Copy session ID", true)?,
        ],
    )?;

    let personalities = personas::list(&cfg);
    let persona_items: Vec<CheckMenuItem<Wry>> = personalities
        .iter()
        .map(|p| {
            CheckMenuItem::with_id(h, format!("persona:{}", p.name), &p.display_name, true, p.name == cfg.persona.active, None::<&str>)
        })
        .collect::<tauri::Result<_>>()?;
    let persona_refs: Vec<&dyn tauri::menu::IsMenuItem<Wry>> = persona_items.iter().map(|i| i as _).collect();
    let persona_menu = Submenu::with_items(h, "Personality", !persona_items.is_empty(), &persona_refs)?;

    Menu::with_items(
        h,
        &[
            &status,
            &sep()?,
            &item("toggle_pet", "Show / hide pet", true)?,
            &item("toggle_panel", "Show / hide panel", true)?,
            &CheckMenuItem::with_id(h, "mute", "Mute", true, !cfg.audio.tts_enabled, None::<&str>)?,
            &sep()?,
            &item("new_session", "New session", true)?,
            &item("stop", "Stop current task", true)?,
            &item("attach", "Attach to session…", true)?,
            &item("detach", "Detach", attached)?,
            &open_in,
            &persona_menu,
            &sep()?,
            &item("settings", "Settings…", true)?,
            &item("quit", "Quit", true)?,
        ],
    )
}

fn status_text(snapshot: &Snapshot) -> String {
    let state = if snapshot.state.is_empty() { "starting" } else { snapshot.state.as_str() };
    match &snapshot.detail {
        Some(detail) if !detail.is_empty() => {
            let short: String = detail.chars().take(48).collect();
            format!("Jar-vis: {state} — {short}")
        }
        _ => format!("Jar-vis: {state}"),
    }
}

pub fn set_tray_status(app: &App, snapshot: &Snapshot) {
    let text = status_text(snapshot);
    if let Some(item) = app.tray_status.lock().as_ref() {
        let _ = item.set_text(&text);
    }
    if let Some(tray) = app.handle.tray_by_id(TRAY) {
        let _ = tray.set_tooltip(Some(&text));
    }
}

pub fn create_tray(app: &Arc<App>) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    let mut builder = TrayIconBuilder::with_id(TRAY).menu(&menu).tooltip("Jar-vis").show_menu_on_left_click(true);
    if let Some(icon) = app.handle.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(&app.handle)?;
    Ok(())
}

/// Rebuild the menu after anything it reflects changed (mute, personality, attach).
pub fn rebuild_tray_menu(app: &App) {
    match build_menu(app) {
        Ok(menu) => {
            if let Some(tray) = app.handle.tray_by_id(TRAY) {
                let _ = tray.set_menu(Some(menu));
            }
        }
        Err(e) => tracing::warn!("could not rebuild the tray menu: {e}"),
    }
}

/// The pet's right-click menu has the same items as the tray.
pub fn popup_menu(app: &App) {
    if let (Some(pet), Ok(menu)) = (app.handle.get_webview_window(PET), build_menu(app)) {
        let _ = pet.popup_menu(&menu);
    }
}

pub fn on_menu_event(handle: &AppHandle, id: &str) {
    let Some(app) = handle.try_state::<Arc<App>>().map(|s| s.inner().clone()) else { return };
    match id {
        "toggle_pet" => toggle_pet(&app),
        "toggle_panel" => toggle_panel(&app),
        "mute" => {
            let muted = app.config.read().audio.tts_enabled;
            app.send(Event::SetMuted(muted));
        }
        "new_session" => app.send(Event::NewSession),
        "stop" => app.send(Event::Abort),
        "attach" => open_settings(handle, "sessions"),
        "detach" => app.send(Event::Detach),
        "settings" => open_settings(handle, "general"),
        "quit" => {
            app.backend.stop_service_if_started();
            crate::state::save(&app.state.lock());
            handle.exit(0);
        }
        _ => {
            if let Some(target) = id.strip_prefix("open:") {
                let target = target.to_string();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = ipc::open_session(&app, &target).await {
                        app.toast(ToastLevel::Error, format!("{e:#}"));
                    }
                });
            } else if let Some(name) = id.strip_prefix("persona:") {
                ipc::apply_personality(&app, name);
            }
        }
    }
}
