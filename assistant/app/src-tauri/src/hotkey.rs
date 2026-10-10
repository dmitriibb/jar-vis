//! The global push-to-talk key.
//!
//! One listener thread sees raw key-down / key-up for every key. It reports the
//! configured key to the orchestrator, and it doubles as the "press a key"
//! recorder for Settings.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use rdev::{EventType, Key};
use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Down,
    Up,
    /// Another key was pressed while the push-to-talk key was held.
    Chord,
}

/// The stable name stored in `config.toml`, e.g. `ControlRight` or `Unknown(124)`.
pub fn key_name(key: Key) -> String {
    format!("{key:?}")
}

/// Keys that can be chosen as the push-to-talk key.
const KEYS: &[Key] = {
    use Key::*;
    &[
        Alt, AltGr, Backspace, CapsLock, ControlLeft, ControlRight, Delete, DownArrow, End, Escape, F1, F2, F3, F4, F5,
        F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24, Home, LeftArrow,
        MetaLeft, MetaRight, PageDown, PageUp, Return, RightArrow, ShiftLeft, ShiftRight, Space, Tab, UpArrow,
        PrintScreen, ScrollLock, Pause, NumLock, BackQuote, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9, Num0,
        Minus, Equal, KeyQ, KeyW, KeyE, KeyR, KeyT, KeyY, KeyU, KeyI, KeyO, KeyP, LeftBracket, RightBracket, KeyA, KeyS,
        KeyD, KeyF, KeyG, KeyH, KeyJ, KeyK, KeyL, SemiColon, Quote, BackSlash, IntlBackslash, KeyZ, KeyX, KeyC, KeyV,
        KeyB, KeyN, KeyM, Comma, Dot, Slash, Insert, KpReturn, KpMinus, KpPlus, KpMultiply, KpDivide, KpDecimal, Kp0,
        Kp1, Kp2, Kp3, Kp4, Kp5, Kp6, Kp7, Kp8, Kp9, Function, Apps,
    ]
};

pub fn parse_key(name: &str) -> Option<Key> {
    let name = name.trim();
    if let Some(code) = name.strip_prefix("Unknown(").and_then(|r| r.strip_suffix(')')) {
        return code.trim().parse().ok().map(Key::Unknown);
    }
    KEYS.iter().copied().find(|k| key_name(*k) == name)
}

/// Modifiers must never be swallowed, and AltGr types characters on German layouts.
pub fn is_modifier(key: Key) -> bool {
    matches!(
        key,
        Key::Alt
            | Key::AltGr
            | Key::ControlLeft
            | Key::ControlRight
            | Key::ShiftLeft
            | Key::ShiftRight
            | Key::MetaLeft
            | Key::MetaRight
            | Key::Function
            | Key::CapsLock
    )
}

#[derive(Default)]
struct State {
    key: Mutex<Option<Key>>,
    down: AtomicBool,
    /// Set while Settings waits for the next key press.
    recording: Mutex<Option<oneshot::Sender<String>>>,
    /// False when the OS refused the global hook (macOS without Accessibility).
    listening: AtomicBool,
}

#[derive(Clone, Default)]
pub struct Hotkey {
    state: Arc<State>,
}

impl Hotkey {
    /// Start the listener thread. `on_event` is called from that thread.
    pub fn start(&self, on_event: impl Fn(HotkeyEvent) + Send + 'static) {
        let state = self.state.clone();
        let spawned = std::thread::Builder::new().name("jarvis-hotkey".into()).spawn(move || {
            state.listening.store(true, Ordering::SeqCst);
            let cb_state = state.clone();
            let result = rdev::listen(move |event| match event.event_type {
                EventType::KeyPress(key) => {
                    if let Some(tx) = cb_state.recording.lock().take() {
                        let _ = tx.send(key_name(key));
                        return;
                    }
                    let target = *cb_state.key.lock();
                    if Some(key) == target {
                        // Holding a key repeats key-down; only the first one counts.
                        if !cb_state.down.swap(true, Ordering::SeqCst) {
                            on_event(HotkeyEvent::Down);
                        }
                    } else if cb_state.down.load(Ordering::SeqCst) {
                        on_event(HotkeyEvent::Chord);
                    }
                }
                EventType::KeyRelease(key) => {
                    if Some(key) == *cb_state.key.lock() && cb_state.down.swap(false, Ordering::SeqCst) {
                        on_event(HotkeyEvent::Up);
                    }
                }
                _ => {}
            });
            state.listening.store(false, Ordering::SeqCst);
            if let Err(e) = result {
                tracing::error!("the global key listener stopped: {e:?}");
            }
        });
        if let Err(e) = spawned {
            tracing::error!("could not start the hotkey thread: {e}");
        }
    }

    /// Change the push-to-talk key. Returns false when the name is not a known key.
    pub fn set_key(&self, name: &str) -> bool {
        let key = parse_key(name);
        *self.state.key.lock() = key;
        self.state.down.store(false, Ordering::SeqCst);
        key.is_some()
    }

    /// Resolve with the name of the next key the user presses.
    pub fn record_next(&self) -> oneshot::Receiver<String> {
        let (tx, rx) = oneshot::channel();
        *self.state.recording.lock() = Some(tx);
        rx
    }

    pub fn cancel_recording(&self) {
        self.state.recording.lock().take();
    }

    pub fn is_listening(&self) -> bool {
        self.state.listening.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_round_trip() {
        for key in [Key::ControlRight, Key::MetaRight, Key::F13, Key::Pause, Key::KeyA, Key::Unknown(124)] {
            assert_eq!(parse_key(&key_name(key)), Some(key), "{key:?}");
        }
    }

    #[test]
    fn unknown_names_are_rejected() {
        assert_eq!(parse_key(""), None);
        assert_eq!(parse_key("NotAKey"), None);
        assert_eq!(parse_key("Unknown(x)"), None);
    }

    #[test]
    fn default_key_parses() {
        assert!(parse_key(crate::config::default_hotkey()).is_some());
    }

    #[test]
    fn modifiers() {
        assert!(is_modifier(Key::ControlRight));
        assert!(is_modifier(Key::AltGr));
        assert!(!is_modifier(Key::F13));
    }
}
