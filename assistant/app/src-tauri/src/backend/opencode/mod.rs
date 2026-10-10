//! opencode backend: REST calls plus one long-lived event stream.

pub mod client;
pub mod events;
pub mod server;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use parking_lot::RwLock;
use tokio::sync::{broadcast, Notify};

use self::client::Client;
use self::server::Conn;
use super::{
    AgentBackend, BackendEvent, BackendInfo, ModelInfo, ModelRef, PermissionReply, PermissionRequest, ProjectRef, SessionOpts, SessionRef,
    SessionSummary, TranscriptItem,
};
use crate::config::OpencodeCfg;

const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// The event stream has no heartbeat we can rely on, so a dead connection
/// (for example after the machine slept) is found by probing.
const HEALTH_INTERVAL: Duration = Duration::from_secs(45);

pub struct OpenCodeBackend {
    http: reqwest::Client,
    cfg: RwLock<OpencodeCfg>,
    conn: RwLock<Option<Conn>>,
    tx: broadcast::Sender<BackendEvent>,
    connected: AtomicBool,
    /// True when Jar-vis started the background service itself.
    started_service: AtomicBool,
    /// Wakes the event loop early: on a config change or a manual retry.
    wake: Notify,
}

impl OpenCodeBackend {
    pub fn new(cfg: OpencodeCfg) -> Arc<Self> {
        let (tx, _) = broadcast::channel(256);
        Arc::new(Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .build()
                .expect("http client"),
            cfg: RwLock::new(cfg),
            conn: RwLock::new(None),
            tx,
            connected: AtomicBool::new(false),
            started_service: AtomicBool::new(false),
            wake: Notify::new(),
        })
    }

    /// Run the connect / stream / reconnect loop until the process exits.
    pub fn start(self: &Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move { this.event_loop().await });
    }

    /// Apply new settings and reconnect with them.
    pub fn reconfigure(&self, cfg: OpencodeCfg) {
        let changed = *self.cfg.read() != cfg;
        if changed {
            *self.cfg.write() = cfg;
            *self.conn.write() = None;
        }
        self.wake.notify_waiters();
    }

    /// Reconnect now instead of waiting for the backoff timer.
    pub fn retry_now(&self) {
        self.wake.notify_waiters();
    }

    pub fn stop_service_if_started(&self) {
        let cfg = self.cfg.read().clone();
        if cfg.stop_on_exit && self.started_service.load(Ordering::SeqCst) {
            server::stop_service(&cfg);
        }
    }

    /// Permanently delete a session. Not part of the backend trait: Jar-vis never
    /// deletes sessions itself. This exists for tests.
    pub async fn delete_session(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        Client { http: &self.http, conn: &conn }.delete_session(id).await
    }

    fn conn(&self) -> Result<Conn> {
        self.conn.read().clone().ok_or_else(|| anyhow!("not connected to opencode"))
    }

    fn set_connected(&self, value: bool) {
        if self.connected.swap(value, Ordering::SeqCst) != value {
            let _ = self.tx.send(if value { BackendEvent::Connected } else { BackendEvent::Disconnected });
        }
    }

    async fn event_loop(self: Arc<Self>) {
        let mut backoff = Duration::from_secs(1);
        loop {
            match self.connect().await {
                Ok(info) => {
                    tracing::info!("connected to opencode {} at {}", info.version, info.url);
                    match self.stream_events().await {
                        Ok(()) => backoff = Duration::from_secs(1),
                        Err(e) => tracing::warn!("opencode event stream ended: {e:#}"),
                    }
                }
                Err(e) => tracing::debug!("opencode is not available: {e:#}"),
            }
            self.set_connected(false);
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = self.wake.notified() => backoff = Duration::from_secs(1),
            }
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    /// Read events until the stream breaks, the config changes, or a health probe fails.
    async fn stream_events(&self) -> Result<()> {
        let conn = self.conn()?;
        let mut req = self.http.get(format!("{}/api/event", conn.url)).header("Accept", "text/event-stream");
        if let Some(pw) = &conn.password {
            req = req.basic_auth(&conn.username, Some(pw));
        }
        let res = req.send().await?;
        if !res.status().is_success() {
            return Err(anyhow!("event stream answered {}", res.status()));
        }
        let mut stream = res.bytes_stream().eventsource();
        let mut normalizer = events::Normalizer::default();
        let mut health = tokio::time::interval_at(tokio::time::Instant::now() + HEALTH_INTERVAL, HEALTH_INTERVAL);
        self.set_connected(true);

        loop {
            tokio::select! {
                item = stream.next() => {
                    let Some(item) = item else { return Ok(()) };
                    let event = item.map_err(|e| anyhow!("{e}"))?;
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&event.data) else { continue };
                    match normalizer.normalize(&value) {
                        // `Connected` is sent once by `set_connected`.
                        Some(BackendEvent::Connected) => {}
                        Some(ev) => { let _ = self.tx.send(ev); }
                        None => tracing::trace!("ignored opencode event {:?}", value.get("type")),
                    }
                }
                _ = health.tick() => {
                    server::probe(&self.http, &conn.url, &conn.username, conn.password.as_deref()).await?;
                }
                _ = self.wake.notified() => {
                    if self.conn.read().is_none() {
                        return Ok(());
                    }
                }
            }
        }
    }
}

macro_rules! client {
    ($self:ident, $conn:ident) => {
        Client { http: &$self.http, conn: &$conn }
    };
}

#[async_trait]
impl AgentBackend for OpenCodeBackend {
    async fn connect(&self) -> Result<BackendInfo> {
        let cfg = self.cfg.read().clone();
        let found = server::discover(&self.http, &cfg).await?;
        if found.started {
            self.started_service.store(true, Ordering::SeqCst);
        }
        let info = BackendInfo { version: found.conn.version.clone(), url: found.conn.url.clone() };
        *self.conn.write() = Some(found.conn);
        Ok(info)
    }

    fn info(&self) -> Option<BackendInfo> {
        if !self.connected.load(Ordering::SeqCst) {
            return None;
        }
        self.conn.read().as_ref().map(|c| BackendInfo { version: c.version.clone(), url: c.url.clone() })
    }

    async fn list_projects(&self) -> Result<Vec<ProjectRef>> {
        let conn = self.conn()?;
        client!(self, conn).list_projects().await
    }

    async fn list_sessions(&self, dir: &Path) -> Result<Vec<SessionSummary>> {
        let conn = self.conn()?;
        client!(self, conn).list_sessions(("directory", dir.to_string_lossy().as_ref())).await
    }

    async fn list_project_sessions(&self, project_id: &str) -> Result<Vec<SessionSummary>> {
        let conn = self.conn()?;
        client!(self, conn).list_sessions(("project", project_id)).await
    }

    async fn create_session(&self, dir: &Path, opts: &SessionOpts) -> Result<SessionSummary> {
        let conn = self.conn()?;
        std::fs::create_dir_all(dir).ok();
        client!(self, conn).create_session(dir, opts.agent.as_deref(), opts.model.as_ref()).await
    }

    async fn get_session(&self, id: &str) -> Result<Option<SessionSummary>> {
        let conn = self.conn()?;
        client!(self, conn).get_session(id).await
    }

    async fn set_agent(&self, s: &SessionRef, agent: &str) -> Result<()> {
        let conn = self.conn()?;
        client!(self, conn).set_agent(&s.id, agent).await
    }

    async fn list_models(&self, dir: &Path) -> Result<Vec<ModelInfo>> {
        let conn = self.conn()?;
        client!(self, conn).list_models(dir).await
    }

    async fn default_model(&self, dir: &Path) -> Result<Option<ModelRef>> {
        let conn = self.conn()?;
        client!(self, conn).default_model(dir).await
    }

    async fn set_model(&self, s: &SessionRef, model: &ModelRef) -> Result<()> {
        let conn = self.conn()?;
        client!(self, conn).set_model(&s.id, model).await
    }

    async fn send_prompt(&self, s: &SessionRef, text: &str) -> Result<()> {
        let conn = self.conn()?;
        client!(self, conn).prompt(&s.id, text).await
    }

    async fn abort(&self, s: &SessionRef) -> Result<()> {
        let conn = self.conn()?;
        client!(self, conn).interrupt(&s.id).await
    }

    async fn is_busy(&self, s: &SessionRef) -> Result<bool> {
        let conn = self.conn()?;
        client!(self, conn).is_active(&s.id).await
    }

    async fn last_assistant_text(&self, s: &SessionRef) -> Result<String> {
        let conn = self.conn()?;
        client!(self, conn).last_assistant_text(&s.id).await
    }

    async fn transcript(&self, s: &SessionRef) -> Result<Vec<TranscriptItem>> {
        let conn = self.conn()?;
        client!(self, conn).transcript(&s.id).await
    }

    async fn pending_permissions(&self, s: &SessionRef) -> Result<Vec<PermissionRequest>> {
        let conn = self.conn()?;
        client!(self, conn).pending_permissions(&s.id).await
    }

    async fn reply_permission(&self, s: &SessionRef, req_id: &str, reply: PermissionReply) -> Result<()> {
        let conn = self.conn()?;
        client!(self, conn).reply_permission(&s.id, req_id, reply).await
    }

    async fn list_agents(&self, dir: &Path) -> Result<Vec<String>> {
        let conn = self.conn()?;
        client!(self, conn).list_agents(dir).await
    }

    async fn reload(&self, dir: &Path) -> Result<()> {
        let conn = self.conn()?;
        client!(self, conn).evict_location(dir).await
    }

    async fn web_url(&self) -> Result<String> {
        let conn = self.conn()?;
        client!(self, conn).pair_url().await
    }

    fn subscribe(&self) -> broadcast::Receiver<BackendEvent> {
        self.tx.subscribe()
    }
}
