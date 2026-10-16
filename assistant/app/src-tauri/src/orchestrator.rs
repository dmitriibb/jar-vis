//! The state machine. One task owns all turn state and is fed by a channel of
//! [`Event`]s (hotkey, UI commands, transcripts) plus the backend event stream.

use std::collections::{HashMap, HashSet, VecDeque};
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
    AgentBackend, BackendEvent, ModelRef, PermissionReply, PermissionRequest, SessionOpts, SessionRef, SessionSummary,
    TranscriptItem,
};
use crate::commands::{self, Command};
use crate::config::Config;
use crate::hotkey::HotkeyEvent;
use crate::schedule::{self, Schedule, Task};
use crate::state::LastAnswer;
use crate::tts::{sanitize, SpeechEnd};
use crate::{personas, reply, stt, windows};

const ERROR_SECONDS: u64 = 6;

pub enum Event {
    Hotkey(HotkeyEvent),
    PetPttDown(PetPttSource),
    PetPttUp(PetPttSource),
    /// Recognized speech, or text typed into the panel (`typed`).
    Transcript { text: String, typed: bool },
    SttFailed(String),
    SpeechEnded { generation: u64, end: SpeechEnd },
    ClearError(u64),
    NewSession,
    Abort,
    ReplyPermission { id: String, reply: PermissionReply },
    SetMuted(bool),
    /// Choose the model, as `provider/id`.
    SetModel(String),
    SetAgent(String),
    Attach(SessionRef),
    Detach,
    ReplayLast,
    CancelTask(String),
    /// The config was saved. Carries the previous one to find what changed.
    ConfigChanged(Box<Config>),
    ResyncPersonas,
    /// A model was installed or removed.
    ModelsChanged,
    /// Something outside the orchestrator changed what the windows show.
    Refresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PetPttSource {
    Secondary,
    MicButton,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PttSource {
    Hotkey,
    Pet(PetPttSource),
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

/// A prompt on its way to the current session.
struct Outgoing {
    /// What the panel shows.
    text: String,
    /// What is sent, when it differs from `text`.
    wire: Option<String>,
    scheduled: bool,
}

impl Outgoing {
    fn user(text: String) -> Self {
        Self { text, wire: None, scheduled: false }
    }
}

pub struct Orchestrator {
    app: Arc<App>,
    rx: mpsc::UnboundedReceiver<Event>,
    connected: bool,
    overlay: Overlay,
    error_generation: u64,
    recorder: Option<Recorder>,
    ptt_source: Option<PttSource>,
    /// A turn is running in the current session.
    turn_active: bool,
    /// The running turn was started by Jar-vis, so its answer is shown and spoken.
    own_turn: bool,
    /// The running turn answers a scheduled prompt, so it may not schedule more.
    own_scheduled: bool,
    tool_line: Option<String>,
    permission: Option<PermissionRequest>,
    /// Generation of the utterance being spoken.
    speaking: Option<u64>,
    queue: VecDeque<Outgoing>,
    schedule: Schedule,
    /// Scheduled prompts running in sessions other than the current one: id -> title.
    background: HashMap<String, String>,
    history: Vec<Exchange>,
    session: Option<SessionSummary>,
    /// The persisted session was confirmed to still exist during this run.
    session_verified: bool,
    /// Sessions where the Jarvis skill has been requested during this run.
    voice_sessions: HashSet<String>,
    /// opencode's own default model, shown when nothing else was chosen.
    default_model: Option<String>,
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
            ptt_source: None,
            turn_active: false,
            own_turn: false,
            own_scheduled: false,
            tool_line: None,
            permission: None,
            speaking: None,
            queue: VecDeque::new(),
            schedule: Schedule::load(),
            background: HashMap::new(),
            history: Vec::new(),
            session: None,
            session_verified: false,
            voice_sessions: HashSet::new(),
            default_model: None,
        }
    }

    pub async fn run(mut self) {
        let mut backend_rx = self.app.backend.subscribe();
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        let mut schedule_tick = tokio::time::interval(Duration::from_secs(10));
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
                _ = schedule_tick.tick() => self.run_due().await,
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
            agent: self.session.as_ref().filter(|s| s.id == current.id).and_then(|s| s.agent.clone()),
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

    /// The model the next turn runs on: the session's own, else the configured
    /// one (workspace sessions only), else opencode's default.
    fn model_key(&self) -> Option<String> {
        let state = self.app.state.lock();
        let current = state.current_session.as_ref();
        let of_session = self
            .session
            .as_ref()
            .filter(|s| current.is_some_and(|c| c.id == s.id))
            .and_then(|s| s.model.as_ref())
            .map(ModelRef::key);
        let configured = Some(self.app.config.read().opencode.model.clone()).filter(|m| !m.is_empty() && !state.attached);
        of_session.or(configured).or_else(|| self.default_model.clone())
    }

    /// Recompute what the windows show and push it to them.
    fn publish(&self) {
        let model = self.model_key();
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
            tasks: self.schedule.tasks(),
            personality: cfg.persona.active.clone(),
            model,
            pet: cfg.persona.pet.clone(),
            hotkey: cfg.hotkey.key.clone(),
            hotkey_listening: self.app.hotkey.is_listening(),
            sleep_after_minutes: cfg.ui.sleep_after_minutes,
            pet_mic_button: cfg.ui.pet_mic_button,
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
            Event::Hotkey(HotkeyEvent::Down) => {
                tracing::info!("push-to-talk key down");
                self.ptt_down(PttSource::Hotkey)
            }
            Event::Hotkey(HotkeyEvent::Up) => {
                tracing::info!("push-to-talk key up");
                self.ptt_up(PttSource::Hotkey)
            }
            Event::Hotkey(HotkeyEvent::Chord) => self.ptt_chord(),
            Event::PetPttDown(source) => self.ptt_down(PttSource::Pet(source)),
            Event::PetPttUp(source) => self.ptt_up(PttSource::Pet(source)),
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
            Event::SetModel(key) => self.set_model(&key).await,
            Event::SetAgent(agent) => {
                if let Err(e) = self.set_agent(&agent).await {
                    self.app.toast(ToastLevel::Error, format!("Could not switch agent: {e:#}"));
                }
            },
            Event::Attach(session) => self.attach(session).await,
            Event::Detach => self.detach().await,
            Event::ReplayLast => self.replay_last(),
            Event::CancelTask(id) => {
                if self.schedule.cancel(&id) {
                    tracing::info!("cancelled scheduled task {id}");
                }
            }
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

    fn ptt_down(&mut self, source: PttSource) {
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
                self.ptt_source = Some(source);
                self.overlay = Overlay::Listening;
            }
            Err(e) => self.fail(format!("Microphone: {e:#}")),
        }
    }

    fn ptt_chord(&mut self) {
        if self.overlay == Overlay::Listening
            && self.ptt_source == Some(PttSource::Hotkey)
            && self.app.config.read().hotkey.cancel_on_chord
        {
            if let Some(recorder) = self.recorder.take() {
                recorder.cancel();
            }
            self.ptt_source = None;
            self.overlay = Overlay::None;
        }
    }

    fn ptt_up(&mut self, source: PttSource) {
        if self.overlay != Overlay::Listening || self.ptt_source != Some(source) {
            return;
        }
        self.ptt_source = None;
        self.overlay = Overlay::None;
        let Some(recorder) = self.recorder.take() else { return };
        let captured = match recorder.finish() {
            Ok(c) => c,
            Err(e) => return self.fail(format!("Microphone: {e:#}")),
        };
        let cfg = self.cfg();
        // A tap is ignored quietly. A real hold that recorded nothing is worth a hint.
        if captured.seconds() < cfg.stt.min_seconds {
            tracing::info!("ignored a {:.2}s tap", captured.seconds());
            return;
        }
        let level = rms(&captured.samples);
        if level < SILENCE_RMS {
            tracing::info!("discarded {:.1}s of silence (level {level:.5})", captured.seconds());
            self.app.toast(ToastLevel::Warn, "I heard only silence. Check the microphone in Settings → Voice input.");
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
            tracing::info!("dropped an empty or hallucinated transcript");
            self.app.toast(ToastLevel::Info, "I didn't catch that.");
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
            self.queue.push_back(Outgoing::user(text));
            self.app.toast(ToastLevel::Info, "Queued — I will send it when the current task is done.");
            return;
        }
        self.send_now(Outgoing::user(text)).await;
    }

    async fn send_now(&mut self, item: Outgoing) {
        self.overlay = Overlay::Sending;
        self.publish();
        match self.deliver(&item).await {
            Ok(()) => {
                self.overlay = Overlay::None;
                self.turn_active = true;
                self.own_turn = true;
                self.own_scheduled = item.scheduled;
                self.tool_line = None;
                self.history.push(Exchange {
                    transcript: item.text,
                    pending: true,
                    scheduled: item.scheduled,
                    time: now_ms(),
                    ..Default::default()
                });
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

    async fn deliver(&mut self, item: &Outgoing) -> Result<()> {
        let session = self.ensure_session().await?;
        let attached = self.app.state.lock().attached;
        let body = item.wire.as_deref().unwrap_or(&item.text);
        let wire = self.voice_prompt(&session, body);
        match self.backend().send_prompt(&session, &wire).await {
            // The session was deleted elsewhere. A prompt from the user starts
            // over in a new one; a scheduled prompt belongs to the old one.
            Err(e) if is_not_found(&e) && !attached && !item.scheduled => {
                let session = self.create_workspace_session().await?;
                let wire = self.voice_prompt(&session, body);
                let result = self.backend().send_prompt(&session, &wire).await;
                if result.is_ok() { self.voice_sessions.insert(session.id.clone()); }
                result
            }
            other => {
                if other.is_ok() { self.voice_sessions.insert(session.id.clone()); }
                other
            },
        }
    }

    fn voice_prompt(&self, session: &SessionRef, body: &str) -> String {
        let first = !self.voice_sessions.contains(&session.id);
        format!("{}{body}", personas::session_prefix(&self.cfg(), first))
    }

    async fn set_agent(&mut self, agent: &str) -> Result<()> {
        if self.turn_active { return Err(anyhow!("Wait for the current turn to finish before switching agents")); }
        let dir = self.current().map(|s| s.directory).unwrap_or_else(|| self.cfg().workspace_dir());
        if !self.backend().list_agents(&dir).await?.iter().any(|a| a == agent) {
            return Err(anyhow!("This agent is not available as a primary agent in this project"));
        }
        let session = self.ensure_session().await?;
        self.backend().set_agent(&session, agent).await?;
        if let Some(info) = &mut self.session { info.agent = Some(agent.to_string()); }
        self.voice_sessions.remove(&session.id);
        if !self.app.state.lock().attached {
            let mut cfg = self.cfg();
            cfg.opencode.agent = agent.to_string();
            crate::ipc::apply_config(&self.app, cfg)?;
        }
        Ok(())
    }

    // ---- scheduled prompts --------------------------------------------------

    /// Send the prompts that have come due to their sessions.
    async fn run_due(&mut self) {
        if !self.connected {
            // They stay due and fire once opencode is back.
            return;
        }
        let due = self.schedule.take_due(now_ms());
        if due.is_empty() {
            return;
        }
        for task in due {
            self.fire(task).await;
        }
        self.publish();
    }

    async fn fire(&mut self, task: Task) {
        tracing::info!("scheduled task {} is due in session {}", task.id, task.session.id);
        let wire = schedule::wire_prompt(&task, now_ms());
        if self.is_current(&task.session.id) {
            let item = Outgoing { text: task.prompt.clone(), wire: Some(wire), scheduled: true };
            if self.turn_active {
                self.queue.push_back(item);
            } else {
                self.send_now(item).await;
            }
            return;
        }
        // Another session: run it in the background and report when it is done.
        let title = task.session_title.clone().unwrap_or_else(|| "another session".to_string());
        let wire = self.voice_prompt(&task.session, &wire);
        match self.backend().send_prompt(&task.session, &wire).await {
            Ok(()) => {
                self.voice_sessions.insert(task.session.id.clone());
                self.app.toast(ToastLevel::Info, format!("Running a scheduled task in \"{title}\"."));
                self.background.insert(task.session.id.clone(), title);
            }
            Err(e) if is_not_found(&e) => {
                let dropped = self.schedule.remove_session(&task.session.id);
                self.app.toast(
                    ToastLevel::Warn,
                    format!("The session \"{title}\" no longer exists. Dropped its scheduled task{}.", if dropped > 0 { "s" } else { "" }),
                );
            }
            Err(e) => self.app.toast(ToastLevel::Error, format!("A scheduled task in \"{title}\" could not be sent: {e:#}")),
        }
    }

    /// Store the tasks an answer asked for. Returns a spoken note about failures.
    fn schedule_requests(&mut self, requests: Vec<Result<schedule::Directive, String>>) {
        let Some(session) = self.current() else { return };
        let title = self.session.as_ref().filter(|s| s.id == session.id).and_then(|s| s.title.clone());
        for request in requests {
            let added = request
                .map_err(|e| anyhow!("the request was not valid JSON ({e})"))
                .and_then(|d| schedule::task_from(&d, &session, title.clone(), now_ms()))
                .and_then(|task| {
                    let note = format!("Scheduled {}: {}", describe_when(&task), task.prompt);
                    self.schedule.add(task).map(|_| note)
                });
            match added {
                Ok(note) => self.app.toast(ToastLevel::Info, note),
                Err(e) => self.app.toast(ToastLevel::Error, format!("Could not schedule that: {e:#}")),
            }
        }
    }

    /// Rebuild the panel history of the current session from opencode.
    async fn reload_history(&mut self) {
        let Some(session) = self.current() else { return };
        match self.backend().transcript(&session).await {
            Ok(items) => {
                self.history = exchanges_from(&items, self.cfg().tts.max_spoken_chars);
                // A turn that is still running has no answer yet.
                if self.turn_active {
                    if let Some(last) = self.history.last_mut().filter(|e| e.summary.is_empty()) {
                        last.pending = true;
                    }
                }
            }
            Err(e) => tracing::warn!("could not load the session history: {e:#}"),
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
        let preferred = if cfg.opencode.agent.is_empty() { &cfg.persona.active } else { &cfg.opencode.agent };
        let agent = Some(preferred.clone()).filter(|a| agents.contains(a));
        if agent.is_none() {
            self.app.toast(
                ToastLevel::Warn,
                format!("opencode does not know the \"{}\" personality yet, using its default agent", preferred),
            );
        }
        let model = ModelRef::parse(&cfg.opencode.model);
        let info = self.backend().create_session(&dir, &SessionOpts { agent, model }).await?;
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
        self.own_scheduled = false;
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
        // From here on its events are handled as the current session's.
        self.background.remove(&session.id);
        self.resync().await;
        self.reload_history().await;
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
        if let Some(current) = self.current() {
            self.background.remove(&current.id);
        }
        self.resync().await;
        self.reload_history().await;
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
                let workspace = self.cfg().workspace_dir();
                self.default_model = self.backend().default_model(&workspace).await.ok().flatten().map(|m| m.key());
                self.sync_personas(false).await;
                self.resync().await;
                if self.history.is_empty() {
                    self.reload_history().await;
                }
            }
            // Scheduled prompts running in a session that is not on screen.
            BackendEvent::SessionIdle { session, interrupted } if !self.is_current(&session) && self.background.contains_key(&session) => {
                let title = self.background.remove(&session).unwrap_or_default();
                if !interrupted {
                    self.report_background(&session, &title).await;
                }
            }
            BackendEvent::SessionError { session, message } if !self.is_current(&session) && self.background.contains_key(&session) => {
                let title = self.background.remove(&session).unwrap_or_default();
                self.app.toast(ToastLevel::Error, format!("The scheduled task in \"{title}\" failed: {message}"));
                self.speak(&format!("A scheduled task in {title} failed."));
            }
            BackendEvent::PermissionAsked(request) if !self.is_current(&request.session) && self.background.contains_key(&request.session) => {
                let title = self.background.get(&request.session).cloned().unwrap_or_default();
                self.app.toast(
                    ToastLevel::Warn,
                    format!("The scheduled task in \"{title}\" needs your permission ({}). Switch to that session to answer.", request.title),
                );
                self.speak(&format!("A scheduled task in {title} needs your permission."));
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
                    self.own_scheduled = false;
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

    /// Speak and show the result of a scheduled prompt that ran in another session.
    async fn report_background(&mut self, session_id: &str, title: &str) {
        let Ok(Some(info)) = self.backend().get_session(session_id).await else { return };
        match self.backend().last_assistant_text(&info.as_ref()).await {
            Ok(text) => {
                // Answers to scheduled prompts may not schedule more.
                let (text, _) = schedule::extract(&text);
                let answer = reply::split(&text, self.cfg().tts.max_spoken_chars);
                self.app.toast(ToastLevel::Info, format!("Scheduled task in \"{title}\": {}", answer.summary));
                self.speak(&format!("Scheduled task in {title}. {}", answer.summary));
            }
            Err(e) => self.app.toast(ToastLevel::Error, format!("The scheduled task in \"{title}\" gave no answer: {e:#}")),
        }
    }

    async fn finish_turn(&mut self, interrupted: bool) {
        let own = self.own_turn;
        let scheduled = self.own_scheduled;
        self.turn_active = false;
        self.own_turn = false;
        self.own_scheduled = false;
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
                    Ok(text) => self.show_answer(&text, scheduled),
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
                if let Some(title) = &info.title {
                    self.schedule.set_title(&info.id, title);
                }
                self.session = Some(info);
            }
        } else {
            // A turn driven from another client: show it, but do not speak it.
            self.reload_history().await;
        }
        self.flush_queue().await;
    }

    /// `scheduled` answers may not schedule further prompts, which would let a
    /// task multiply itself.
    fn show_answer(&mut self, text: &str, scheduled: bool) {
        let cfg = self.cfg();
        let (text, requests) = schedule::extract(text);
        if !scheduled {
            self.schedule_requests(requests);
        }
        let text = if text.is_empty() { "Scheduled.".to_string() } else { text };
        let answer = reply::split(&text, cfg.tts.max_spoken_chars);
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

    /// Switch the current session to another model. In the workspace the choice
    /// is also remembered for new sessions; an attached session is only switched.
    async fn set_model(&mut self, key: &str) {
        let Some(model) = ModelRef::parse(key) else {
            return self.app.toast(ToastLevel::Warn, format!("\"{key}\" is not a model"));
        };
        let attached = self.app.state.lock().attached;
        if !attached {
            let mut cfg = self.app.config.write();
            cfg.opencode.model = model.key();
            if let Err(e) = crate::config::save(&cfg) {
                tracing::warn!("could not save config: {e:#}");
            }
            let _ = self.app.handle.emit("config-changed", &*cfg);
        }
        if let (Some(session), true) = (self.current(), self.connected) {
            match self.backend().set_model(&session, &model).await {
                Ok(()) => {
                    if let Ok(Some(info)) = self.backend().get_session(&session.id).await {
                        self.session = Some(info);
                    }
                }
                Err(e) => self.app.toast(ToastLevel::Error, format!("Could not switch model: {e:#}")),
            }
        }
        tracing::info!("model set to {}", model.key());
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
                match self.backend().set_agent(&session, &cfg.persona.active).await {
                    Err(e) => self.app.toast(ToastLevel::Warn, format!("Could not switch personality: {e:#}")),
                    Ok(()) => {
                        if let Some(info) = &mut self.session { info.agent = Some(cfg.persona.active.clone()); }
                        self.voice_sessions.remove(&session.id);
                    }
                }
            }
        }
        windows::rebuild_tray_menu(&self.app);
    }
}

/// "in 60 minutes", "every 15 minutes", for the confirmation toast.
fn describe_when(task: &Task) -> String {
    let minutes = ((task.next_run - task.created) as f64 / 60_000.0).round().max(1.0) as i64;
    let delay = if minutes == 1 { "1 minute".to_string() } else { format!("{minutes} minutes") };
    match task.every_minutes {
        Some(every) => format!("every {} min, first in {delay}", every.round() as i64),
        None => format!("in {delay}"),
    }
}

/// Pair user messages with the answer that followed them. A turn can produce
/// several agent messages; the last one before the next user message wins.
pub fn exchanges_from(items: &[TranscriptItem], max_spoken_chars: usize) -> Vec<Exchange> {
    let mut out: Vec<Exchange> = Vec::new();
    for item in items {
        if item.user {
            let (text, scheduled) = schedule::display_text(&item.text);
            out.push(Exchange { transcript: text, scheduled, time: item.time, ..Default::default() });
        } else {
            let (text, _) = schedule::extract(&item.text);
            if text.is_empty() {
                continue;
            }
            let answer = reply::split(&text, max_spoken_chars);
            if out.is_empty() {
                out.push(Exchange { time: item.time, ..Default::default() });
            }
            let last = out.last_mut().expect("just ensured");
            last.summary = answer.summary;
            last.details = answer.details;
        }
    }
    if out.len() > MAX_HISTORY {
        out.drain(..out.len() - MAX_HISTORY);
    }
    out
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
    use super::{exchanges_from, same_dir};
    use crate::backend::TranscriptItem;
    use std::path::Path;

    #[test]
    fn history_is_rebuilt_from_a_transcript() {
        let item = |user: bool, text: &str, time: i64| TranscriptItem { user, text: text.to_string(), time };
        let items = vec![
            item(true, "what is two plus two", 1),
            item(false, "Let me think.", 2),
            item(false, "Four.\n---\nBecause 2 + 2 = 4.", 3),
            item(true, "[scheduled task: the user is not typing this now.]\n\nCheck the service", 4),
            item(false, "It is running.\n\n```jarvis-schedule\n{\"prompt\":\"x\",\"in_minutes\":5}\n```", 5),
            item(true, "still there?", 6),
        ];
        let history = exchanges_from(&items, 400);
        assert_eq!(history.len(), 3);
        assert_eq!((history[0].transcript.as_str(), history[0].summary.as_str()), ("what is two plus two", "Four."));
        assert_eq!(history[0].details, "Because 2 + 2 = 4.");
        assert!(!history[0].scheduled);
        assert_eq!((history[1].transcript.as_str(), history[1].summary.as_str()), ("Check the service", "It is running."));
        assert!(history[1].scheduled);
        assert_eq!((history[2].transcript.as_str(), history[2].summary.as_str()), ("still there?", ""));
    }

    #[test]
    fn same_dir_ignores_separators() {
        assert!(same_dir(Path::new("C:/projects/jar-vis/sessions"), Path::new("C:\\projects\\jar-vis\\sessions\\")));
        assert!(!same_dir(Path::new("/a/b"), Path::new("/a/c")));
    }
}
