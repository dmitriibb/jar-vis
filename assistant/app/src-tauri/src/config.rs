//! `config.toml`: load, validate, save, and locate the Jar-vis home folder.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Jar-vis home (the folder that holds `assistant/`, `sessions/`, `models/`).
    pub home: String,
    /// Set once the first-run wizard has been completed.
    pub wizard_done: bool,
    pub hotkey: HotkeyCfg,
    pub audio: AudioCfg,
    pub stt: SttCfg,
    pub tts: TtsCfg,
    pub opencode: OpencodeCfg,
    pub persona: PersonaCfg,
    pub attach: AttachCfg,
    pub openchamber: OpenchamberCfg,
    pub commands: CommandsCfg,
    pub ui: UiCfg,
    pub models: ModelsCfg,
    pub log: LogCfg,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HotkeyCfg {
    /// rdev key name, e.g. `ControlRight`. Empty until the wizard records one.
    pub key: String,
    pub cancel_on_chord: bool,
    pub suppress: bool,
}

impl Default for HotkeyCfg {
    fn default() -> Self {
        Self { key: default_hotkey().into(), cancel_on_chord: true, suppress: false }
    }
}

/// Never Right Alt / Right Option: on German layouts those type `@ € { } [ ] \ |`.
pub fn default_hotkey() -> &'static str {
    if cfg!(target_os = "macos") {
        "MetaRight"
    } else {
        "ControlRight"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioCfg {
    /// Empty means the system default input device.
    pub input_device: String,
    /// The mute checkbox. When false the TTS model is never loaded.
    pub tts_enabled: bool,
}

impl Default for AudioCfg {
    fn default() -> Self {
        Self { input_device: String::new(), tts_enabled: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SttCfg {
    pub model: String,
    pub preload: bool,
    /// 0 keeps the model loaded.
    pub unload_after_idle_minutes: u32,
    pub min_seconds: f32,
    pub max_seconds: u32,
    pub vocabulary: String,
}

impl Default for SttCfg {
    fn default() -> Self {
        Self {
            model: "base.en-q5_1".into(),
            preload: true,
            unload_after_idle_minutes: 0,
            min_seconds: 0.35,
            max_seconds: 120,
            vocabulary: "opencode, openchamber, Tauri, Rust, TypeScript".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TtsCfg {
    pub voice: String,
    pub speed: f32,
    pub max_spoken_chars: usize,
    pub unload_after_idle_minutes: u32,
}

impl Default for TtsCfg {
    fn default() -> Self {
        Self {
            voice: "piper-en_US-lessac-medium".into(),
            speed: 1.0,
            max_spoken_chars: 400,
            unload_after_idle_minutes: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OpencodeCfg {
    /// Empty means "use opencode's own background service" (discovered from its
    /// `service.json`). Set it to talk to an explicit `opencode serve` instance.
    pub url: String,
    /// Start the background service when it is not running.
    pub autostart: bool,
    /// Stop the background service on exit, but only if Jar-vis started it.
    pub stop_on_exit: bool,
    /// Empty means auto-detect.
    pub binary: String,
    pub username: String,
    /// Only used together with an explicit `url`.
    pub password: String,
    /// Empty means `<home>/sessions`.
    pub workspace_dir: String,
    /// Model for Jar-vis sessions as `provider/id`. Empty uses opencode's default.
    pub model: String,
    /// Agent for new workspace sessions; empty keeps the chosen personality.
    pub agent: String,
}

impl Default for OpencodeCfg {
    fn default() -> Self {
        Self {
            url: String::new(),
            autostart: true,
            stop_on_exit: false,
            binary: String::new(),
            username: "opencode".into(),
            password: String::new(),
            workspace_dir: String::new(),
            model: String::new(),
            agent: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PersonaCfg {
    /// File stem in `assistant/agents`.
    pub active: String,
    pub pet: String,
}

impl Default for PersonaCfg {
    fn default() -> Self {
        Self { active: "jarvis".into(), pet: "default".into() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AttachCfg {
    /// Empty keeps the attached session's own agent.
    pub override_agent: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OpenchamberCfg {
    pub url: String,
    pub app_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CommandsCfg {
    pub wake_word: String,
    pub new_session: Vec<String>,
    pub stop: Vec<String>,
    pub mute: Vec<String>,
    pub unmute: Vec<String>,
    pub repeat: Vec<String>,
}

impl Default for CommandsCfg {
    fn default() -> Self {
        let v = |items: &[&str]| items.iter().map(|s| s.to_string()).collect();
        Self {
            wake_word: "jarvis".into(),
            new_session: v(&[
                "start new session",
                "start a new session",
                "new session",
                "new chat",
                "new conversation",
            ]),
            stop: v(&["stop", "cancel", "abort"]),
            mute: v(&["mute", "be quiet"]),
            unmute: v(&["unmute"]),
            repeat: v(&["repeat", "say that again"]),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiCfg {
    pub panel_auto_show: bool,
    /// 0 never hides the panel automatically.
    pub panel_auto_hide_seconds: u32,
    pub stream_text: bool,
    pub sleep_after_minutes: u32,
}

impl Default for UiCfg {
    fn default() -> Self {
        Self {
            panel_auto_show: true,
            panel_auto_hide_seconds: 0,
            stream_text: false,
            sleep_after_minutes: 10,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelsCfg {
    /// Empty means `<home>/models`.
    pub dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LogCfg {
    pub level: String,
}

impl Default for LogCfg {
    fn default() -> Self {
        Self { level: "info".into() }
    }
}

impl Config {
    pub fn home_dir(&self) -> PathBuf {
        PathBuf::from(&self.home)
    }

    pub fn workspace_dir(&self) -> PathBuf {
        if self.opencode.workspace_dir.trim().is_empty() {
            self.home_dir().join("sessions")
        } else {
            PathBuf::from(&self.opencode.workspace_dir)
        }
    }

    pub fn models_dir(&self) -> PathBuf {
        if self.models.dir.trim().is_empty() {
            self.home_dir().join("models")
        } else {
            PathBuf::from(&self.models.dir)
        }
    }

    pub fn agents_dir(&self) -> PathBuf {
        self.home_dir().join("assistant").join("agents")
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.home_dir().join("assistant").join("skills")
    }

    pub fn pets_dir(&self) -> PathBuf {
        self.home_dir().join("assistant").join("pets")
    }

    /// Clamp out-of-range values back to their defaults. Returns one message per fix.
    pub fn validate(&mut self) -> Vec<String> {
        let mut fixed = Vec::new();
        let d = Config::default();
        if !(0.5..=2.0).contains(&self.tts.speed) {
            fixed.push(format!("tts.speed {} is out of range, using {}", self.tts.speed, d.tts.speed));
            self.tts.speed = d.tts.speed;
        }
        if self.tts.max_spoken_chars < 40 {
            fixed.push("tts.max_spoken_chars is too small, using the default".into());
            self.tts.max_spoken_chars = d.tts.max_spoken_chars;
        }
        if !(0.0..=5.0).contains(&self.stt.min_seconds) {
            fixed.push("stt.min_seconds is out of range, using the default".into());
            self.stt.min_seconds = d.stt.min_seconds;
        }
        if self.stt.max_seconds == 0 || self.stt.max_seconds > 600 {
            fixed.push("stt.max_seconds is out of range, using the default".into());
            self.stt.max_seconds = d.stt.max_seconds;
        }
        if self.stt.model.trim().is_empty() {
            self.stt.model = d.stt.model;
        }
        if self.tts.voice.trim().is_empty() {
            self.tts.voice = d.tts.voice;
        }
        if self.persona.active.trim().is_empty() {
            self.persona.active = d.persona.active;
        }
        if self.persona.pet.trim().is_empty() {
            self.persona.pet = d.persona.pet;
        }
        if self.opencode.username.trim().is_empty() {
            self.opencode.username = d.opencode.username;
        }
        if !["trace", "debug", "info", "warn", "error"].contains(&self.log.level.as_str()) {
            fixed.push(format!("log.level \"{}\" is unknown, using info", self.log.level));
            self.log.level = d.log.level;
        }
        fixed
    }
}

/// `%APPDATA%\jarvis` on Windows, `~/Library/Application Support/jarvis` on macOS.
pub fn config_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.config_dir().join("jarvis"))
        .unwrap_or_else(|| PathBuf::from(".jarvis"))
}

/// Runtime state and logs. `%LOCALAPPDATA%\jarvis` on Windows.
pub fn data_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.data_local_dir().join("jarvis"))
        .unwrap_or_else(|| PathBuf::from(".jarvis"))
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub struct Loaded {
    pub config: Config,
    /// Problems found while loading, shown to the user as toasts.
    pub warnings: Vec<String>,
    /// True when no config file existed yet.
    pub created: bool,
}

pub fn load() -> Loaded {
    load_from(&config_path())
}

pub fn load_from(path: &Path) -> Loaded {
    let mut warnings = Vec::new();
    let mut created = false;
    let mut config = match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str::<Config>(&text) {
            Ok(c) => c,
            Err(e) => {
                warnings.push(format!("config.toml could not be parsed, using defaults: {e}"));
                Config::default()
            }
        },
        Err(_) => {
            created = true;
            Config::default()
        }
    };
    warnings.extend(config.validate());
    if config.home.trim().is_empty() {
        if let Some(home) = detect_home() {
            config.home = home.to_string_lossy().into_owned();
        }
    }
    if created {
        if let Err(e) = save_to(&config, path) {
            warnings.push(format!("could not write config.toml: {e}"));
        }
    }
    Loaded { config, warnings, created }
}

pub fn save(config: &Config) -> Result<()> {
    save_to(config, &config_path())
}

pub fn save_to(config: &Config, path: &Path) -> Result<()> {
    let text = toml::to_string_pretty(config).context("serialize config")?;
    write_atomic(path, text.as_bytes())
}

/// Write to a temp file next to the target, then rename over it.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

/// In a dev checkout the home is the repo root: walk up from the executable and
/// the working directory until a folder containing `assistant/agents` is found.
pub fn detect_home() -> Option<PathBuf> {
    let mut starts = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            starts.push(dir.to_path_buf());
        }
    }
    for start in starts {
        let mut dir: Option<&Path> = Some(start.as_path());
        while let Some(d) = dir {
            if d.join("assistant").join("agents").is_dir() {
                return Some(d.to_path_buf());
            }
            dir = d.parent();
        }
    }
    None
}

/// Where the wizard suggests creating the home in an installed build.
pub fn suggested_home() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().join("jar-vis"))
        .unwrap_or_else(|| PathBuf::from("jar-vis"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        let c = Config::default();
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn partial_file_keeps_defaults() {
        let c: Config = toml::from_str("[tts]\nspeed = 1.2\n").unwrap();
        assert_eq!(c.tts.speed, 1.2);
        assert_eq!(c.tts.voice, "piper-en_US-lessac-medium");
        assert_eq!(c.stt.model, "base.en-q5_1");
        assert!(c.audio.tts_enabled);
    }

    #[test]
    fn invalid_values_fall_back() {
        let mut c: Config = toml::from_str("[tts]\nspeed = 9.0\n[log]\nlevel = \"loud\"\n").unwrap();
        let fixed = c.validate();
        assert_eq!(fixed.len(), 2);
        assert_eq!(c.tts.speed, 1.0);
        assert_eq!(c.log.level, "info");
    }

    #[test]
    fn default_key_is_never_alt() {
        assert!(!default_hotkey().contains("Alt"));
    }

    #[test]
    fn derived_dirs() {
        let mut c = Config::default();
        c.home = "/x/jar-vis".into();
        assert_eq!(c.workspace_dir(), PathBuf::from("/x/jar-vis").join("sessions"));
        c.opencode.workspace_dir = "/y/ws".into();
        assert_eq!(c.workspace_dir(), PathBuf::from("/y/ws"));
    }
}
