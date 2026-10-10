//! The agent backend abstraction. opencode is the only implementation today;
//! Codex can be added later behind the same trait.

pub mod opencode;

use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRef {
    pub id: String,
    pub directory: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub title: Option<String>,
    pub directory: PathBuf,
    pub agent: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub updated: i64,
}

impl SessionSummary {
    pub fn as_ref(&self) -> SessionRef {
        SessionRef { id: self.id.clone(), directory: self.directory.clone() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRef {
    pub id: String,
    pub name: String,
    pub directory: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendInfo {
    pub version: String,
    pub url: String,
}

#[derive(Debug, Clone, Default)]
pub struct SessionOpts {
    /// Agent the new session starts with. None keeps the backend's default.
    pub agent: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionReply {
    Once,
    Always,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    pub id: String,
    pub session: String,
    /// What the agent wants to do, e.g. `bash` or `edit`.
    pub kind: String,
    pub title: String,
    pub patterns: Vec<String>,
}

/// Backend events, normalized so the orchestrator does not depend on any one
/// backend's wire format.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEvent {
    Connected,
    Disconnected,
    SessionBusy { session: String },
    ToolRunning { session: String, tool: String, title: Option<String> },
    TextDelta { session: String, text: String },
    PermissionAsked(PermissionRequest),
    PermissionReplied { session: String, id: String },
    /// The turn finished. `interrupted` is true when it was aborted.
    SessionIdle { session: String, interrupted: bool },
    SessionError { session: String, message: String },
}

#[async_trait]
pub trait AgentBackend: Send + Sync {
    /// Check that the backend is reachable. Starts it if configured to.
    async fn connect(&self) -> Result<BackendInfo>;
    /// The last successful `connect`, if the event stream is currently up.
    fn info(&self) -> Option<BackendInfo>;
    async fn list_projects(&self) -> Result<Vec<ProjectRef>>;
    /// Top-level sessions whose working directory is `dir`, newest first.
    async fn list_sessions(&self, dir: &Path) -> Result<Vec<SessionSummary>>;
    /// Top-level sessions anywhere in a project, newest first.
    async fn list_project_sessions(&self, project_id: &str) -> Result<Vec<SessionSummary>>;
    async fn create_session(&self, dir: &Path, opts: &SessionOpts) -> Result<SessionSummary>;
    /// None when the session no longer exists.
    async fn get_session(&self, id: &str) -> Result<Option<SessionSummary>>;
    async fn set_agent(&self, s: &SessionRef, agent: &str) -> Result<()>;
    async fn send_prompt(&self, s: &SessionRef, text: &str) -> Result<()>;
    async fn abort(&self, s: &SessionRef) -> Result<()>;
    async fn is_busy(&self, s: &SessionRef) -> Result<bool>;
    async fn last_assistant_text(&self, s: &SessionRef) -> Result<String>;
    async fn pending_permissions(&self, s: &SessionRef) -> Result<Vec<PermissionRequest>>;
    async fn reply_permission(&self, s: &SessionRef, req_id: &str, reply: PermissionReply) -> Result<()>;
    /// Names of the agents the backend has registered for a directory.
    async fn list_agents(&self, dir: &Path) -> Result<Vec<String>>;
    /// Make the backend re-read agent and skill files for a directory.
    async fn reload(&self, dir: &Path) -> Result<()>;
    /// A URL that opens the backend's own web UI, already authenticated.
    async fn web_url(&self) -> Result<String>;
    fn subscribe(&self) -> broadcast::Receiver<BackendEvent>;
}
