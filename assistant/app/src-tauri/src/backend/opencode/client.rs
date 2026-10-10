//! Thin REST client for the opencode v2 HTTP API (`/api/...`).
//!
//! Verified against opencode 2.0.26 (`GET /openapi.json`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Result};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};

use super::events::permission_request;
use super::server::Conn;
use crate::backend::{PermissionReply, PermissionRequest, ProjectRef, SessionSummary};

const TIMEOUT: Duration = Duration::from_secs(20);

/// A non-2xx answer from opencode.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "opencode: {} ({})", self.message, self.status.as_u16())
    }
}

impl std::error::Error for ApiError {}

pub fn is_not_found(err: &anyhow::Error) -> bool {
    err.downcast_ref::<ApiError>().is_some_and(|e| e.status == StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
struct Data<T> {
    data: T,
}

#[derive(Deserialize)]
struct SessionInfo {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    location: Location,
    time: SessionTime,
}

#[derive(Deserialize)]
struct Location {
    directory: PathBuf,
}

#[derive(Deserialize)]
struct SessionTime {
    #[serde(default)]
    updated: f64,
}

impl From<SessionInfo> for SessionSummary {
    fn from(s: SessionInfo) -> Self {
        SessionSummary {
            id: s.id,
            title: s.title.filter(|t| !t.trim().is_empty()),
            directory: s.location.directory,
            agent: s.agent,
            updated: s.time.updated as i64,
        }
    }
}

#[derive(Deserialize)]
struct Project {
    id: String,
    canonical: PathBuf,
    #[serde(default)]
    name: Option<String>,
}

pub struct Client<'a> {
    pub http: &'a reqwest::Client,
    pub conn: &'a Conn,
}

impl Client<'_> {
    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let mut req = self.http.request(method, format!("{}{}", self.conn.url, path)).timeout(TIMEOUT);
        if let Some(pw) = &self.conn.password {
            req = req.basic_auth(&self.conn.username, Some(pw));
        }
        req
    }

    /// Requests without a session are scoped with `location[directory]`.
    fn at(req: RequestBuilder, dir: &Path) -> RequestBuilder {
        req.query(&[("location[directory]", dir.to_string_lossy().as_ref())])
    }

    async fn send(req: RequestBuilder) -> Result<Response> {
        let res = req.send().await.map_err(|e| anyhow!("opencode is not reachable: {e}"))?;
        let status = res.status();
        if status.is_success() {
            return Ok(res);
        }
        let body = res.text().await.unwrap_or_default();
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| body.chars().take(200).collect());
        Err(ApiError { status, message }.into())
    }

    pub async fn list_projects(&self) -> Result<Vec<ProjectRef>> {
        let projects: Vec<Project> = Self::send(self.request(Method::GET, "/api/project")).await?.json().await?;
        Ok(projects
            .into_iter()
            .map(|p| {
                let name = p.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
                    p.canonical
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.canonical.to_string_lossy().into_owned())
                });
                ProjectRef { id: p.id, name, directory: p.canonical }
            })
            .collect())
    }

    /// Top-level sessions, newest first. `filter` is `("directory", path)` or `("project", id)`.
    pub async fn list_sessions(&self, filter: (&str, &str)) -> Result<Vec<SessionSummary>> {
        let req = self
            .request(Method::GET, "/api/session")
            .query(&[filter, ("order", "desc"), ("limit", "100"), ("parentID", "null")]);
        let res: Data<Vec<SessionInfo>> = Self::send(req).await?.json().await?;
        Ok(res.data.into_iter().map(Into::into).collect())
    }

    pub async fn create_session(&self, dir: &Path, agent: Option<&str>) -> Result<SessionSummary> {
        let mut body = json!({ "location": { "directory": dir } });
        if let Some(agent) = agent {
            body["agent"] = json!(agent);
        }
        let res: Data<SessionInfo> =
            Self::send(self.request(Method::POST, "/api/session").json(&body)).await?.json().await?;
        Ok(res.data.into())
    }

    pub async fn get_session(&self, id: &str) -> Result<Option<SessionSummary>> {
        match Self::send(self.request(Method::GET, &format!("/api/session/{id}"))).await {
            Ok(res) => {
                let res: Data<SessionInfo> = res.json().await?;
                Ok(Some(res.data.into()))
            }
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn delete_session(&self, id: &str) -> Result<()> {
        Self::send(self.request(Method::DELETE, &format!("/api/session/{id}"))).await?;
        Ok(())
    }

    pub async fn set_agent(&self, id: &str, agent: &str) -> Result<()> {
        Self::send(self.request(Method::POST, &format!("/api/session/{id}/agent")).json(&json!({ "agent": agent })))
            .await?;
        Ok(())
    }

    /// Admit one prompt and schedule the agent loop. Returns as soon as it is accepted.
    pub async fn prompt(&self, id: &str, text: &str) -> Result<()> {
        Self::send(self.request(Method::POST, &format!("/api/session/{id}/prompt")).json(&json!({ "text": text })))
            .await?;
        Ok(())
    }

    pub async fn interrupt(&self, id: &str) -> Result<()> {
        Self::send(self.request(Method::POST, &format!("/api/session/{id}/interrupt"))).await?;
        Ok(())
    }

    /// Sessions absent from `/api/session/active` are idle.
    pub async fn is_active(&self, id: &str) -> Result<bool> {
        let res: Data<Value> = Self::send(self.request(Method::GET, "/api/session/active")).await?.json().await?;
        Ok(res.data.get(id).is_some())
    }

    /// The text of the newest assistant message of the latest turn.
    pub async fn last_assistant_text(&self, id: &str) -> Result<String> {
        let req = self
            .request(Method::GET, &format!("/api/session/{id}/message"))
            .query(&[("order", "desc"), ("limit", "30")]);
        let res: Data<Vec<Value>> = Self::send(req).await?.json().await?;
        last_assistant_text(&res.data)
    }

    pub async fn pending_permissions(&self, id: &str) -> Result<Vec<PermissionRequest>> {
        let res: Data<Vec<Value>> =
            Self::send(self.request(Method::GET, &format!("/api/session/{id}/permission"))).await?.json().await?;
        Ok(res.data.iter().filter_map(permission_request).collect())
    }

    pub async fn reply_permission(&self, id: &str, req_id: &str, reply: PermissionReply) -> Result<()> {
        let decision = match reply {
            PermissionReply::Once => "once",
            PermissionReply::Always => "always",
            PermissionReply::Reject => "reject",
        };
        let path = format!("/api/session/{id}/permission/{req_id}/reply");
        Self::send(self.request(Method::POST, &path).json(&json!({ "decision": decision }))).await?;
        Ok(())
    }

    pub async fn list_agents(&self, dir: &Path) -> Result<Vec<String>> {
        // opencode always has built-in agents. While it is still building the
        // instance for a directory the list is empty, so wait for it to fill.
        let mut agents = Vec::new();
        for attempt in 0..20 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            let res: Data<Vec<Value>> =
                Self::send(Self::at(self.request(Method::GET, "/api/agent"), dir)).await?.json().await?;
            agents = res.data.iter().filter_map(|a| a.get("id").and_then(Value::as_str)).map(str::to_string).collect();
            if !agents.is_empty() {
                break;
            }
        }
        Ok(agents)
    }

    /// Drop the cached instance for one directory so its config is re-read.
    pub async fn evict_location(&self, dir: &Path) -> Result<()> {
        Self::send(Self::at(self.request(Method::DELETE, "/api/debug/location"), dir)).await?;
        Ok(())
    }

    /// A single-use link that signs a browser into the web UI.
    pub async fn pair_url(&self) -> Result<String> {
        let res: Value = Self::send(self.request(Method::POST, "/api/pair")).await?.json().await?;
        let code = res.get("code").and_then(Value::as_str).ok_or_else(|| anyhow!("no pairing code returned"))?;
        Ok(format!("{}/auth/connect/{code}", self.conn.url))
    }
}

/// `messages` is newest first. Walk back to the latest user message and return
/// the newest assistant text found on the way.
pub fn last_assistant_text(messages: &[Value]) -> Result<String> {
    let mut error: Option<String> = None;
    for message in messages {
        match message.get("type").and_then(Value::as_str) {
            Some("user") => break,
            Some("assistant") => {
                let text = message
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
                            .filter_map(|p| p.get("text").and_then(Value::as_str))
                            .map(str::trim)
                            .filter(|t| !t.is_empty())
                            .collect::<Vec<_>>()
                            .join("\n\n")
                    })
                    .unwrap_or_default();
                if !text.is_empty() {
                    return Ok(text);
                }
                if error.is_none() {
                    error = message
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
            _ => {}
        }
    }
    match error {
        Some(e) => Err(anyhow!("the agent failed: {e}")),
        None => Err(anyhow!("the agent finished without a text answer")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_newest_assistant_text() {
        let messages = vec![
            json!({"type":"idle","outcome":"succeeded"}),
            json!({"type":"assistant","content":[{"type":"reasoning","text":"hm"},{"type":"text","text":"Done.\n---\nDetails"}]}),
            json!({"type":"assistant","content":[{"type":"text","text":"Let me check."},{"type":"tool","name":"bash"}]}),
            json!({"type":"user","text":"run the tests"}),
            json!({"type":"assistant","content":[{"type":"text","text":"old answer"}]}),
        ];
        assert_eq!(last_assistant_text(&messages).unwrap(), "Done.\n---\nDetails");
    }

    #[test]
    fn skips_tool_only_messages() {
        let messages = vec![
            json!({"type":"assistant","content":[{"type":"tool","name":"bash"}]}),
            json!({"type":"assistant","content":[{"type":"text","text":"Running it now."}]}),
            json!({"type":"user","text":"go"}),
        ];
        assert_eq!(last_assistant_text(&messages).unwrap(), "Running it now.");
    }

    #[test]
    fn never_returns_a_previous_turn() {
        let messages = vec![
            json!({"type":"idle","outcome":"failed"}),
            json!({"type":"user","text":"second"}),
            json!({"type":"assistant","content":[{"type":"text","text":"first answer"}]}),
        ];
        assert!(last_assistant_text(&messages).is_err());
    }

    #[test]
    fn reports_assistant_error() {
        let messages = vec![
            json!({"type":"assistant","content":[],"error":{"type":"provider","message":"rate limited"}}),
            json!({"type":"user","text":"hi"}),
        ];
        let err = last_assistant_text(&messages).unwrap_err().to_string();
        assert!(err.contains("rate limited"), "{err}");
    }
}
