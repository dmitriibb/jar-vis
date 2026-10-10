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
    /// The session's model, when one was chosen explicitly.
    pub model: Option<ModelRef>,
    /// Milliseconds since the Unix epoch.
    pub updated: i64,
}

/// A model, written `provider/id` in the config and the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub id: String,
    #[serde(rename = "providerID")]
    pub provider_id: String,
}

impl ModelRef {
    /// Parse `provider/id`. Model ids may themselves contain slashes.
    pub fn parse(key: &str) -> Option<ModelRef> {
        let (provider, id) = key.trim().split_once('/')?;
        if provider.is_empty() || id.is_empty() {
            return None;
        }
        Some(ModelRef { id: id.to_string(), provider_id: provider.to_string() })
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.provider_id, self.id)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// `provider/id`.
    pub key: String,
    pub name: String,
    pub provider: String,
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
    /// Model the new session starts with. None keeps the backend's default.
    pub model: Option<ModelRef>,
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
    /// Models that can drive an agent (text output, tool use), newest first.
    async fn list_models(&self, dir: &Path) -> Result<Vec<ModelInfo>>;
    /// The model used when a session has none chosen.
    async fn default_model(&self, dir: &Path) -> Result<Option<ModelRef>>;
    async fn set_model(&self, s: &SessionRef, model: &ModelRef) -> Result<()>;
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
