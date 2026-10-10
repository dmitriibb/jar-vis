//! The state machine. One task owns all turn state and is fed by a channel of
//! [`Event`]s (hotkey, UI commands, transcripts) plus the backend event stream.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use tauri::Emitter;
use tokio::sync::{broadcast, mpsc};

use crate::app::{now_ms, App, Exchange, SessionView, Snapshot, ToastLevel, MAX_HISTORY};
use crate::audio::capture::Recorder;
use crate::audio::level::{rms, SILENCE_RMS};
use crate::audio::resample;
use crate::backend::opencode::client::is_not_found;
use crate::backend::{
    AgentBackend, BackendEvent, PermissionReply, PermissionRequest, SessionOpts, SessionRef, SessionSummary,
};
use crate::commands::{self, Command};
use crate::config::Config;
use crate::hotkey::HotkeyEvent;
use crate::state::LastAnswer;
use crate::tts::{sanitize, SpeechEnd};
use crate::{personas, reply, stt, windows};

const ERROR_SECONDS: u64 = 6;

pub enum Event {
    Hotkey(HotkeyEvent),
    /// Recognized speech, or text typed into the panel (`typed`).
    Transcript { text: String, typed: bool },
    SttFailed(String),
    SpeechEnded { generation: u64, end: SpeechEnd },
    ClearError(u64),
    NewSession,
    Abort,
    ReplyPermission { id: String, reply: PermissionReply },
    SetMuted(bool),
    Attach(SessionRef),
    Detach,
    ReplayLast,
    /// The config was saved. Carries the previous one to find what changed.
    ConfigChanged(Box<Config>),
    ResyncPersonas,
    /// A model was installed or removed.
    ModelsChanged,
    /// Something outside the orchestrator changed what the windows show.
    Refresh,
}

/// A short-lived state shown on top of the turn state.
#[derive(Debug, Clone, PartialEq)]
enum Overlay {
    None,
    Listening,
    Transcribing,
    Sending,
    Error(String),
}

pub struct Orchestrator {
    app: Arc<App>,
    rx: mpsc::UnboundedReceiver<Event>,
    connected: bool,
    overlay: Overlay,
    error_generation: u64,
    recorder: Option<Recorder>,
    /// A turn is running in the current session.
    turn_active: bool,
    /// The running turn was started by Jar-vis, so its answer is shown and spoken.
    own_turn: bool,
    tool_line: Option<String>,
    permission: Option<PermissionRequest>,
    /// Generation of the utterance being spoken.
    speaking: Option<u64>,
    queue: VecDeque<String>,
    history: Vec<Exchange>,
    session: Option<SessionSummary>,
    /// The persisted session was confirmed to still exist during this run.
    session_verified: bool,
}

impl Orchestrator {
    pub fn new(app: Arc<App>, rx: mpsc::UnboundedReceiver<Event>) -> Self {
        Self {
            app,
            rx,
            connected: false,
            overlay: Overlay::None,
            error_generation: 0,
            recorder: None,
            turn_active: false,
            own_turn: false,
            tool_line: None,
            permission: None,
            speaking: None,
            queue: VecDeque::new(),
            history: Vec::new(),
            session: None,
            session_verified: false,
        }
    }

    pub async fn run(mut self) {
        let mut backend_rx = self.app.backend.subscribe();
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        self.preload_stt();
        self.publish();
        loop {
            tokio::select! {
                event = self.rx.recv() => match event {
                    Some(event) => self.handle(event).await,
                    None => break,
                },
                event = backend_rx.recv() => match event {
                    Ok(event) => self.on_backend(event).await,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("missed {n} backend events, resyncing");
                        self.resync().await;
                        self.publish();
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = tick.tick() => self.on_tick(),
            }
        }
    }

    fn cfg(&self) -> Config {
        self.app.config.read().clone()
    }

    fn backend(&self) -> &dyn AgentBackend {
        self.app.backend.as_ref()
    }

    fn current(&self) -> Option<SessionRef> {
        self.app.state.lock().current_session.clone()
    }

    fn is_current(&self, session: &str) -> bool {
        self.current().is_some_and(|c| c.id == session)
    }

    // ---- publishing -----------------------------------------------------

    fn phase(&self) -> (&'static str, Option<String>) {
        match &self.overlay {
            Overlay::Listening => ("listening", None),
            Overlay::Transcribing => ("transcribing", None),
            Overlay::Sending => ("sending", None),
            Overlay::Error(message) => ("error", Some(message.clone())),
            Overlay::None => {
                if !self.connected {
                    ("offline", Some("opencode is not running".to_string()))
                } else if let Some(p) = &self.permission {
                    ("permission", Some(p.title.clone()))
                } else if self.turn_active {
                    ("working", self.tool_line.clone())
                } else if self.speaking.is_some() {
                    ("speaking", None)
                } else {
                    ("idle", None)
                }
            }
        }
    }

    fn session_view(&self) -> Option<SessionView> {
        let state = self.app.state.lock();
        let current = state.current_session.as_ref()?;
        let title = self.session.as_ref().filter(|s| s.id == current.id).and_then(|s| s.title.clone());
        Some(SessionView {
            id: current.id.clone(),
            title,
            directory: current.directory.to_string_lossy().into_owned(),
            mode: if state.attached { "attached" } else { "workspace" }.to_string(),
            project: current
                .directory
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        })
    }

    /// Recompute what the windows show and push it to them.
    fn publish(&self) {
        let cfg = self.app.config.read();
        let (state, detail) = self.phase();
        let snapshot = Snapshot {
            state: state.to_string(),
            detail,
            muted: !cfg.audio.tts_enabled,
            backend: self.backend().info(),
            session: self.session_view(),
            history: self.history.clone(),
            permission: self.permission.clone(),
            queued: self.queue.len(),
            personality: cfg.persona.active.clone(),
            pet: cfg.persona.pet.clone(),
            hotkey: cfg.hotkey.key.clone(),
            hotkey_listening: self.app.hotkey.is_listening(),
            sleep_after_minutes: cfg.ui.sleep_after_minutes,
            stream_text: cfg.ui.stream_text,
            wizard_done: cfg.wizard_done,
        };
        drop(cfg);
        windows::set_tray_status(&self.app, &snapshot);
        let _ = self.app.handle.emit("snapshot", &snapshot);
        *self.app.snapshot.write() = snapshot;
    }

    fn fail(&mut self, message: impl Into<String>) {
        let message = message.into();
        tracing::error!("{message}");
        self.error_generation += 1;
        self.overlay = Overlay::Error(message);
        let generation = self.error_generation;
        let tx = self.app.tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(ERROR_SECONDS)).await;
            let _ = tx.send(Event::ClearError(generation));
        });
    }

    // ---- events ---------------------------------------------------------

    async fn handle(&mut self, event: Event) {
        match event {
            Event::Hotkey(HotkeyEvent::Down) => self.ptt_down(),
            Event::Hotkey(HotkeyEvent::Up) => self.ptt_up(),
            Event::Hotkey(HotkeyEvent::Chord) => self.ptt_chord(),
            Event::Transcript { text, typed } => self.on_transcript(text, typed).await,
            Event::SttFailed(message) => {
                self.overlay = Overlay::None;
                self.fail(message);
            }
            Event::SpeechEnded { generation, end } => {
                if let SpeechEnd::Failed(message) = &end {
                    self.app.toast(ToastLevel::Warn, format!("Could not speak: {message}"));
                }
                if self.speaking == Some(generation) {
                    self.speaking = None;
                }
            }
            Event::ClearError(generation) => {
                if generation == self.error_generation && matches!(self.overlay, Overlay::Error(_)) {
                    self.overlay = Overlay::None;
                }
            }
            Event::NewSession => {
                if let Err(e) = self.new_session().await {
                    self.fail(format!("Could not start a session: {e:#}"));
                }
            }
            Event::Abort => self.abort().await,
            Event::ReplyPermission { id, reply } => self.reply_permission(Some(id), reply).await,
            Event::SetMuted(muted) => self.set_muted(muted),
            Event::Attach(session) => self.attach(session).await,
            Event::Detach => self.detach().await,
            Event::ReplayLast => self.replay_last(),
            Event::ConfigChanged(old) => self.on_config_changed(*old).await,
            Event::ResyncPersonas => self.sync_personas(true).await,
            Event::ModelsChanged => self.preload_stt(),
            Event::Refresh => {}
        }
        self.publish();
    }

    fn on_tick(&mut self) {
        let cfg = self.app.config.read();
        self.app.stt.unload_if_idle(cfg.stt.unload_after_idle_minutes);
        self.app.tts.unload_if_idle(cfg.tts.unload_after_idle_minutes);
    }

    // ---- push-to-talk ---------------------------------------------------

    fn ptt_down(&mut self) {
        // Offline there is nobody to talk to; while a clip is in flight, wait for it.
        if !self.connected || matches!(self.overlay, Overlay::Listening | Overlay::Transcribing | Overlay::Sending) {
            return;
        }
        // Barge-in: stop talking the moment the user wants to speak.
        self.app.tts.stop();
        self.speaking = None;
        let cfg = self.cfg();
        let handle = self.app.handle.clone();
        let started = Recorder::start(&cfg.audio.input_device, cfg.stt.max_seconds, move |level| {
            let _ = handle.emit("mic-level", level);
        });
        match started {
            Ok(recorder) => {
                self.recorder = Some(recorder);
                self.overlay = Overlay::Listening;
            }
            Err(e) => self.fail(format!("Microphone: {e:#}")),
        }
    }

    fn ptt_chord(&mut self) {
        if self.overlay == Overlay::Listening && self.app.config.read().hotkey.cancel_on_chord {
            if let Some(recorder) = self.recorder.take() {
                recorder.cancel();
            }
            self.overlay = Overlay::None;
        }
    }

    fn ptt_up(&mut self) {
        if self.overlay != Overlay::Listening {
            return;
        }
        self.overlay = Overlay::None;
        let Some(recorder) = self.recorder.take() else { return };
        let captured = match recorder.finish() {
            Ok(c) => c,
            Err(e) => return self.fail(format!("Microphone: {e:#}")),
        };
        let cfg = self.cfg();
        if captured.seconds() < cfg.stt.min_seconds || rms(&captured.samples) < SILENCE_RMS {
            tracing::debug!("discarded a {:.2}s clip", captured.seconds());
            return;
        }
        let model = match self.app.models.stt_path(&cfg.stt.model) {
            Ok(path) => path,
            Err(e) => return self.fail(format!("{e:#}")),
        };
        self.overlay = Overlay::Transcribing;
        let stt = self.app.stt.clone();
        let tx = self.app.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = resample::to_16k(&captured.samples, captured.sample_rate)
                .and_then(|pcm| stt.transcribe(&model, &pcm, &cfg.stt.vocabulary));
            let _ = tx.send(match result {
                Ok(text) => Event::Transcript { text, typed: false },
                Err(e) => Event::SttFailed(format!("Transcription failed: {e:#}")),
            });
        });
    }

    fn preload_stt(&self) {
        let cfg = self.cfg();
        if !cfg.stt.preload {
            return;
        }
        if let Ok(model) = self.app.models.stt_path(&cfg.stt.model) {
            let stt = self.app.stt.clone();
            let app = self.app.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(e) = stt.ensure_loaded(&model) {
                    app.toast(ToastLevel::Warn, format!("Could not load the speech model: {e:#}"));
                }
            });
        }
    }

    // ---- transcripts and commands -----------------------------------------

    async fn on_transcript(&mut self, text: String, typed: bool) {
        if self.overlay == Overlay::Transcribing {
            self.overlay = Overlay::None;
        }
        let text = if typed { Some(text.trim().to_string()).filter(|t| !t.is_empty()) } else { stt::filter::clean(&text) };
        let Some(text) = text else {
            tracing::debug!("dropped an empty or hallucinated transcript");
            return;
        };
        tracing::info!("heard: {text}");
        let command = commands::parse(&text, &self.app.config.read().commands, self.permission.is_some());
        match command {
            Command::NewSession { rest } => match self.new_session().await {
                Ok(()) if rest.is_empty() => self.speak("New session started."),
                Ok(()) => self.submit(rest).await,
                Err(e) => self.fail(format!("Could not start a session: {e:#}")),
            },
            Command::Stop => self.abort().await,
            Command::Mute => self.set_muted(true),
            Command::Unmute => {
                self.set_muted(false);
                self.speak("Unmuted.");
            }
            Command::Repeat => self.replay_last(),
            Command::Permission(reply) => self.reply_permission(None, reply).await,
            Command::Prompt(prompt) => self.submit(prompt).await,
        }
    }

    /// Send a prompt now, or queue it behind the running turn.
    async fn submit(&mut self, text: String) {
        if !self.connected {
            return self.fail("opencode is not running");
        }
        if self.turn_active {
            self.queue.push_back(text);
            self.app.toast(ToastLevel::Info, "Queued — I will send it when the current task is done.");
            return;
        }
        self.send_now(text).await;
    }

    async fn send_now(&mut self, text: String) {
        self.overlay = Overlay::Sending;
        self.publish();
        match self.deliver(&text).await {
            Ok(()) => {
                self.overlay = Overlay::None;
                self.turn_active = true;
                self.own_turn = true;
                self.tool_line = None;
                self.history.push(Exchange { transcript: text, pending: true, time: now_ms(), ..Default::default() });
                if self.history.len() > MAX_HISTORY {
                    let extra = self.history.len() - MAX_HISTORY;
                    self.history.drain(..extra);
                }
            }
            Err(e) => {
                self.overlay = Overlay::None;
                self.fail(format!("Could not send: {e:#}"));
            }
        }
    }

    async fn deliver(&mut self, text: &str) -> Result<()> {
        let session = self.ensure_session().await?;
        let attached = self.app.state.lock().attached;
        // Workspace agents carry the voice rules in their prompt. An attached
        // session keeps its own agent, so the rules travel with the message.
        let wire = if attached {
            format!("[voice mode: {}]\n\n{text}", personas::voice_rules_inline(&self.cfg()))
        } else {
            text.to_string()
        };
        match self.backend().send_prompt(&session, &wire).await {
            Err(e) if is_not_found(&e) && !attached => {
                // The session was deleted elsewhere. Start over in a new one.
                let session = self.create_workspace_session().await?;
                self.backend().send_prompt(&session, text).await
            }
            other => other,
        }
    }

    // ---- sessions ---------------------------------------------------------

    async fn ensure_session(&mut self) -> Result<SessionRef> {
        if let Some(current) = self.current() {
            if self.session_verified {
                return Ok(current);
            }
            if let Some(info) = self.backend().get_session(&current.id).await? {
                self.session = Some(info);
                self.session_verified = true;
                return Ok(current);
            }
            tracing::info!("session {} no longer exists", current.id);
        }
        self.create_workspace_session().await
    }

    async fn create_workspace_session(&mut self) -> Result<SessionRef> {
        let cfg = self.cfg();
        let dir = cfg.workspace_dir();
        let agents = self.backend().list_agents(&dir).await.unwrap_or_default();
        let agent = Some(cfg.persona.active.clone()).filter(|a| agents.contains(a));
        if agent.is_none() {
            self.app.toast(
                ToastLevel::Warn,
                format!("opencode does not know the \"{}\" personality yet, using its default agent", cfg.persona.active),
            );
        }
        let info = self.backend().create_session(&dir, &SessionOpts { agent }).await?;
        let session = info.as_ref();
        tracing::info!("created session {} in {}", session.id, session.directory.display());
        self.app.update_state(|s| {
            s.current_session = Some(session.clone());
            s.workspace_session = Some(session.clone());
            s.attached = false;
        });
        self.session = Some(info);
        self.session_verified = true;
        self.reset_turn_state();
        self.history.clear();
        Ok(session)
    }

    fn reset_turn_state(&mut self) {
        self.turn_active = false;
        self.own_turn = false;
        self.tool_line = None;
        self.permission = None;
        self.queue.clear();
    }

    async fn new_session(&mut self) -> Result<()> {
        if !self.connected {
            return Err(anyhow!("opencode is not running"));
        }
        self.create_workspace_session().await.map(|_| ())
    }

    async fn attach(&mut self, session: SessionRef) {
        let workspace = self.cfg().workspace_dir();
        let attached = !same_dir(&session.directory, &workspace);
        self.app.update_state(|s| {
            if !s.attached {
                s.workspace_session = s.current_session.clone();
            }
            s.current_session = Some(session.clone());
            s.attached = attached;
        });
        self.session = None;
        self.session_verified = false;
        self.reset_turn_state();
        self.history.clear();
        self.resync().await;
        if attached {
            let override_agent = self.cfg().attach.override_agent;
            if !override_agent.trim().is_empty() {
                if let Err(e) = self.backend().set_agent(&session, override_agent.trim()).await {
                    self.app.toast(ToastLevel::Warn, format!("Could not set the agent: {e:#}"));
                }
            }
        }
    }

    async fn detach(&mut self) {
        self.app.update_state(|s| {
            s.current_session = s.workspace_session.clone();
            s.attached = false;
        });
        self.session = None;
        self.session_verified = false;
        self.reset_turn_state();
        self.history.clear();
        self.resync().await;
    }

    /// Bring turn state in line with the backend, after connecting or switching session.
    async fn resync(&mut self) {
        let Some(current) = self.current() else { return };
        if !self.connected {
            return;
        }
        match self.backend().get_session(&current.id).await {
            Ok(Some(info)) => {
                self.session = Some(info);
                self.session_verified = true;
            }
            Ok(None) => {
                // Gone. The next prompt creates a fresh workspace session.
                self.app.update_state(|s| {
                    if s.workspace_session.as_ref() == Some(&current) {
                        s.workspace_session = None;
                    }
                    s.current_session = if s.attached { s.workspace_session.clone() } else { None };
                    s.attached = false;
                });
                self.session = None;
                self.session_verified = false;
                self.reset_turn_state();
                return;
            }
            Err(e) => {
                tracing::warn!("could not read the session: {e:#}");
                return;
            }
        }
        let busy = self.backend().is_busy(&current).await.unwrap_or(false);
        self.permission = self.backend().pending_permissions(&current).await.ok().and_then(|mut p| p.pop());
        if self.turn_active && !busy {
            // The turn ended while we were not listening.
            self.finish_turn(false).await;
        } else {
            self.turn_active = busy;
        }
    }

    // ---- backend events -----------------------------------------------------

    async fn on_backend(&mut self, event: BackendEvent) {
        match event {
            BackendEvent::Connected => {
                self.connected = true;
                self.sync_personas(false).await;
                self.resync().await;
            }
            BackendEvent::Disconnected => {
                self.connected = false;
            }
            BackendEvent::SessionBusy { session } if self.is_current(&session) => {
                self.turn_active = true;
            }
            BackendEvent::ToolRunning { session, tool, title } if self.is_current(&session) => {
                self.tool_line = Some(match title {
                    Some(title) => format!("{tool}: {title}"),
                    None => tool,
                });
            }
            BackendEvent::TextDelta { session, text } if self.is_current(&session) => {
                if self.app.config.read().ui.stream_text {
                    let _ = self.app.handle.emit("progress", text);
                }
                return;
            }
            BackendEvent::PermissionAsked(request) if self.is_current(&request.session) => {
                self.speak(&format!("I need your permission. {}", request.title));
                self.permission = Some(request);
                windows::show_panel(&self.app);
            }
            BackendEvent::PermissionReplied { session, id } if self.is_current(&session) => {
                if self.permission.as_ref().is_some_and(|p| p.id == id) {
                    self.permission = None;
                }
            }
            BackendEvent::SessionIdle { session, interrupted } if self.is_current(&session) => {
                self.finish_turn(interrupted).await;
            }
            BackendEvent::SessionError { session, message } if self.is_current(&session) => {
                self.turn_active = false;
                self.tool_line = None;
                self.permission = None;
                if self.own_turn {
                    self.own_turn = false;
                    if let Some(last) = self.history.last_mut().filter(|e| e.pending) {
                        last.pending = false;
                        last.error = Some(message.clone());
                    }
                    self.fail(message);
                    if self.app.config.read().ui.panel_auto_show {
                        windows::show_panel(&self.app);
                    }
                }
                self.flush_queue().await;
            }
            _ => return,
        }
        self.publish();
    }

    async fn finish_turn(&mut self, interrupted: bool) {
        let own = self.own_turn;
        self.turn_active = false;
        self.own_turn = false;
        self.tool_line = None;
        self.permission = None;
        let Some(session) = self.current() else { return };

        if own {
            if interrupted {
                if let Some(last) = self.history.last_mut().filter(|e| e.pending) {
                    last.pending = false;
                    last.summary = "Stopped.".to_string();
                }
            } else {
                match self.backend().last_assistant_text(&session).await {
                    Ok(text) => self.show_answer(&text),
                    Err(e) => {
                        if let Some(last) = self.history.last_mut().filter(|e| e.pending) {
                            last.pending = false;
                            last.error = Some(format!("{e:#}"));
                        }
                        self.fail(format!("{e:#}"));
                    }
                }
            }
            // opencode names the session after the first turn.
            if let Ok(Some(info)) = self.backend().get_session(&session.id).await {
                self.session = Some(info);
            }
        }
        self.flush_queue().await;
    }

    fn show_answer(&mut self, text: &str) {
        let cfg = self.cfg();
        let answer = reply::split(text, cfg.tts.max_spoken_chars);
        match self.history.last_mut().filter(|e| e.pending) {
            Some(last) => {
                last.pending = false;
                last.summary = answer.summary.clone();
                last.details = answer.details.clone();
            }
            None => self.history.push(Exchange {
                summary: answer.summary.clone(),
                details: answer.details.clone(),
                time: now_ms(),
                ..Default::default()
            }),
        }
        self.app.update_state(|s| {
            s.last_answer = Some(LastAnswer { summary: answer.summary.clone(), details: answer.details.clone() })
        });
        if cfg.ui.panel_auto_show {
            windows::show_panel(&self.app);
        }
        self.speak(&answer.summary);
    }

    async fn flush_queue(&mut self) {
        if self.turn_active || !self.connected {
            return;
        }
        if let Some(next) = self.queue.pop_front() {
            self.send_now(next).await;
        }
    }

    async fn abort(&mut self) {
        self.app.tts.stop();
        self.speaking = None;
        self.queue.clear();
        if !self.turn_active {
            return;
        }
        if let Some(session) = self.current() {
            if let Err(e) = self.backend().abort(&session).await {
                self.fail(format!("Could not stop: {e:#}"));
            }
        }
    }

    async fn reply_permission(&mut self, id: Option<String>, reply: PermissionReply) {
        let Some(request) = self.permission.clone() else { return };
        if id.is_some_and(|id| id != request.id) {
            return;
        }
        let Some(session) = self.current() else { return };
        self.app.tts.stop();
        self.speaking = None;
        match self.backend().reply_permission(&session, &request.id, reply).await {
            Ok(()) => self.permission = None,
            Err(e) if is_not_found(&e) => self.permission = None,
            Err(e) => self.fail(format!("Could not answer the permission request: {e:#}")),
        }
    }

    // ---- speech -------------------------------------------------------------

    fn speak(&mut self, text: &str) {
        let cfg = self.cfg();
        if !cfg.audio.tts_enabled {
            return;
        }
        let spoken = sanitize::for_speech(text, cfg.tts.max_spoken_chars);
        if spoken.is_empty() {
            return;
        }
        let personality = personas::find(&cfg, &cfg.persona.active);
        let preferred = personality.as_ref().and_then(|p| p.voice.clone());
        // A personality's voice is used when installed, else the configured one.
        let voice = preferred
            .and_then(|id| self.app.models.voice(&id).ok())
            .map(Ok)
            .unwrap_or_else(|| self.app.models.voice(&cfg.tts.voice));
        let voice = match voice {
            Ok(voice) => voice,
            Err(e) => {
                tracing::warn!("not speaking: {e:#}");
                return;
            }
        };
        let speed = cfg.tts.speed * personality.and_then(|p| p.speed).unwrap_or(1.0);
        self.speaking = Some(self.app.tts.speak(&spoken, voice, speed.clamp(0.5, 2.0)));
    }

    fn replay_last(&mut self) {
        let last = self.app.state.lock().last_answer.clone();
        match last {
            Some(answer) if !answer.summary.is_empty() => self.speak(&answer.summary),
            _ => self.speak("There is nothing to repeat yet."),
        }
    }

    fn set_muted(&mut self, muted: bool) {
        {
            let mut cfg = self.app.config.write();
            if cfg.audio.tts_enabled == !muted {
                return;
            }
            cfg.audio.tts_enabled = !muted;
            if let Err(e) = crate::config::save(&cfg) {
                tracing::warn!("could not save config: {e:#}");
            }
            // An open Settings window must not write the old value back.
            let _ = self.app.handle.emit("config-changed", &*cfg);
        }
        if muted {
            // Muting frees the voice model as well as silencing it.
            self.app.tts.unload();
            self.speaking = None;
        }
        windows::rebuild_tray_menu(&self.app);
    }

    // ---- config and personas ------------------------------------------------

    async fn sync_personas(&mut self, force_reload: bool) {
        let cfg = self.cfg();
        if cfg.home.trim().is_empty() {
            return;
        }
        let report = match tokio::task::spawn_blocking({
            let cfg = cfg.clone();
            move || personas::ensure_home(&cfg.home_dir()).and_then(|_| personas::sync(&cfg))
        })
        .await
        {
            Ok(Ok(report)) => report,
            Ok(Err(e)) => return self.app.toast(ToastLevel::Warn, format!("Could not sync personalities: {e:#}")),
            Err(e) => return tracing::error!("persona sync task failed: {e}"),
        };
        for warning in &report.warnings {
            self.app.toast(ToastLevel::Warn, warning.clone());
        }
        if !self.connected {
            return;
        }
        let dir = cfg.workspace_dir();
        // opencode caches agents per directory. Reload when files changed, or when
        // the active personality is not registered yet.
        let known = self.backend().list_agents(&dir).await.unwrap_or_default();
        let missing = report.agents.iter().any(|a| !known.contains(a));
        if report.changed || missing || force_reload {
            if self.turn_active && !self.app.state.lock().attached {
                tracing::info!("skipping the opencode reload while a turn is running");
                return;
            }
            match self.backend().reload(&dir).await {
                Ok(()) => tracing::info!("asked opencode to reload {}", dir.display()),
                Err(e) => tracing::warn!("could not reload opencode config: {e:#}"),
            }
        }
        windows::rebuild_tray_menu(&self.app);
    }

    async fn on_config_changed(&mut self, old: Config) {
        let cfg = self.cfg();
        if cfg.hotkey.key != old.hotkey.key && !self.app.hotkey.set_key(&cfg.hotkey.key) {
            self.app.toast(ToastLevel::Warn, format!("\"{}\" is not a known key", cfg.hotkey.key));
        }
        self.app.models.set_dir(cfg.models_dir());
        self.app.backend.reconfigure(cfg.opencode.clone());

        if !cfg.audio.tts_enabled && old.audio.tts_enabled {
            self.app.tts.unload();
            self.speaking = None;
        }
        if cfg.stt.model != old.stt.model || cfg.models.dir != old.models.dir {
            self.app.stt.unload();
            self.preload_stt();
        }
        if cfg.home != old.home || cfg.opencode.workspace_dir != old.opencode.workspace_dir {
            self.sync_personas(true).await;
        }
        if cfg.persona.active != old.persona.active {
            // The running session switches agent too, unless it belongs to another project.
            let attached = self.app.state.lock().attached;
            if let (Some(session), false, true) = (self.current(), attached, self.connected) {
                if let Err(e) = self.backend().set_agent(&session, &cfg.persona.active).await {
                    self.app.toast(ToastLevel::Warn, format!("Could not switch personality: {e:#}"));
                }
            }
        }
        windows::rebuild_tray_menu(&self.app);
    }
}

/// Compare two directories ignoring separators, case (on Windows) and trailing slashes.
pub fn same_dir(a: &std::path::Path, b: &std::path::Path) -> bool {
    let norm = |p: &std::path::Path| {
        let s = p.to_string_lossy().replace('\\', "/");
        let s = s.trim_end_matches('/').to_string();
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s
        }
    };
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::same_dir;
    use std::path::Path;

    #[test]
    fn same_dir_ignores_separators() {
        assert!(same_dir(Path::new("C:/projects/jar-vis/sessions"), Path::new("C:\\projects\\jar-vis\\sessions\\")));
        assert!(!same_dir(Path::new("/a/b"), Path::new("/a/c")));
    }
}
