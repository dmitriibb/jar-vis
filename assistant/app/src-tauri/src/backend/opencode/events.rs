//! Turn opencode's event stream into [`BackendEvent`]s.
//!
//! Each SSE message carries a JSON object `{id, type, location?, data}`. Unknown
//! types are ignored, never an error: the event set changes between versions.

use std::collections::HashMap;

use serde_json::Value;

use crate::backend::{BackendEvent, PermissionRequest};

const MAX_TITLE_CHARS: usize = 70;

#[derive(Default)]
pub struct Normalizer {
    /// Tool call id -> tool name. `session.tool.called` only carries the id.
    tools: HashMap<String, String>,
}

impl Normalizer {
    pub fn normalize(&mut self, event: &Value) -> Option<BackendEvent> {
        let kind = event.get("type")?.as_str()?;
        let data = event.get("data").unwrap_or(&Value::Null);
        let session = || data.get("sessionID").and_then(Value::as_str).map(str::to_string);

        match kind {
            "server.connected" => Some(BackendEvent::Connected),
            "session.execution.started" => Some(BackendEvent::SessionBusy { session: session()? }),
            "session.execution.succeeded" => Some(BackendEvent::SessionIdle { session: session()?, interrupted: false }),
            "session.execution.interrupted" => Some(BackendEvent::SessionIdle { session: session()?, interrupted: true }),
            "session.execution.failed" => Some(BackendEvent::SessionError {
                session: session()?,
                message: error_message(data.get("error")),
            }),
            "session.tool.input.started" => {
                let id = data.get("id")?.as_str()?;
                let name = data.get("name")?.as_str()?;
                if self.tools.len() > 512 {
                    self.tools.clear();
                }
                self.tools.insert(id.to_string(), name.to_string());
                Some(BackendEvent::ToolRunning { session: session()?, tool: name.to_string(), title: None })
            }
            "session.tool.called" => {
                let id = data.get("id").and_then(Value::as_str).unwrap_or_default();
                let tool = self
                    .tools
                    .get(id)
                    .cloned()
                    .or_else(|| data.get("name").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_else(|| "tool".to_string());
                Some(BackendEvent::ToolRunning { session: session()?, tool, title: tool_title(data.get("input")) })
            }
            "session.tool.success" | "session.tool.failed" => {
                if let Some(id) = data.get("id").and_then(Value::as_str) {
                    self.tools.remove(id);
                }
                None
            }
            "session.text.delta" => Some(BackendEvent::TextDelta {
                session: session()?,
                text: data.get("delta")?.as_str()?.to_string(),
            }),
            "permission.asked" => Some(BackendEvent::PermissionAsked(permission_request(data)?)),
            "permission.replied" => Some(BackendEvent::PermissionReplied {
                session: session()?,
                id: data.get("requestID")?.as_str()?.to_string(),
            }),
            _ => None,
        }
    }
}

/// Parse a `Permission.Request` as found in events and in the REST listing.
pub fn permission_request(data: &Value) -> Option<PermissionRequest> {
    let kind = data.get("action").and_then(Value::as_str).unwrap_or("permission").to_string();
    let patterns: Vec<String> = data
        .get("resources")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let title = match data.get("message").and_then(Value::as_str).filter(|m| !m.trim().is_empty()) {
        Some(message) => message.to_string(),
        None if patterns.is_empty() => kind.clone(),
        None => format!("{kind}: {}", shorten(&patterns.join(", "))),
    };
    Some(PermissionRequest {
        id: data.get("id")?.as_str()?.to_string(),
        session: data.get("sessionID")?.as_str()?.to_string(),
        kind,
        title,
        patterns,
    })
}

fn error_message(error: Option<&Value>) -> String {
    let Some(error) = error else { return "the agent failed".to_string() };
    let message = error.get("message").and_then(Value::as_str).unwrap_or("the agent failed");
    match error.get("type").and_then(Value::as_str) {
        Some("provider.auth") => {
            format!("opencode could not authenticate with the model provider ({message}). Run `opencode auth login`.")
        }
        Some(kind) => format!("{kind}: {message}"),
        None => message.to_string(),
    }
}

/// A short human label for a tool call, taken from its most telling argument.
fn tool_title(input: Option<&Value>) -> Option<String> {
    let input = input?.as_object()?;
    const KEYS: &[&str] = &["command", "description", "filePath", "file_path", "path", "pattern", "query", "url", "prompt"];
    KEYS.iter()
        .find_map(|k| input.get(*k).and_then(Value::as_str))
        .map(|s| shorten(s.lines().next().unwrap_or(s)))
        .filter(|s| !s.is_empty())
}

fn shorten(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= MAX_TITLE_CHARS {
        text.to_string()
    } else {
        let head: String = text.chars().take(MAX_TITLE_CHARS).collect();
        format!("{}…", head.trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture_events() -> Vec<Value> {
        include_str!("../../../tests/fixtures/events.jsonl")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("fixture line is JSON"))
            .collect()
    }

    #[test]
    fn recorded_failed_turn() {
        let mut n = Normalizer::default();
        let out: Vec<BackendEvent> = fixture_events().iter().filter_map(|e| n.normalize(e)).collect();
        let sid = "ses_edac8add1ffek5tSqToVVaCItw".to_string();
        assert_eq!(out[0], BackendEvent::Connected);
        assert_eq!(out[1], BackendEvent::SessionBusy { session: sid.clone() });
        match &out[2] {
            BackendEvent::SessionError { session, message } => {
                assert_eq!(session, &sid);
                assert!(message.contains("opencode auth login"), "{message}");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn tool_call_uses_name_from_input_started() {
        let mut n = Normalizer::default();
        let started = json!({"type":"session.tool.input.started","data":{"sessionID":"s1","assistantMessageID":"m1","id":"call_1","name":"bash"}});
        assert_eq!(
            n.normalize(&started),
            Some(BackendEvent::ToolRunning { session: "s1".into(), tool: "bash".into(), title: None })
        );
        let called = json!({"type":"session.tool.called","data":{"sessionID":"s1","assistantMessageID":"m1","id":"call_1","input":{"command":"npm test\necho done"}}});
        assert_eq!(
            n.normalize(&called),
            Some(BackendEvent::ToolRunning { session: "s1".into(), tool: "bash".into(), title: Some("npm test".into()) })
        );
    }

    #[test]
    fn text_delta_and_completion() {
        let mut n = Normalizer::default();
        let delta = json!({"type":"session.text.delta","data":{"sessionID":"s1","assistantMessageID":"m1","ordinal":0,"delta":"Hel"}});
        assert_eq!(n.normalize(&delta), Some(BackendEvent::TextDelta { session: "s1".into(), text: "Hel".into() }));
        let ok = json!({"type":"session.execution.succeeded","data":{"sessionID":"s1"}});
        assert_eq!(n.normalize(&ok), Some(BackendEvent::SessionIdle { session: "s1".into(), interrupted: false }));
        let stopped = json!({"type":"session.execution.interrupted","data":{"sessionID":"s1","reason":"user"}});
        assert_eq!(n.normalize(&stopped), Some(BackendEvent::SessionIdle { session: "s1".into(), interrupted: true }));
    }

    #[test]
    fn permission_events() {
        let mut n = Normalizer::default();
        let asked = json!({"type":"permission.asked","data":{"id":"per_1","sessionID":"s1","action":"bash","resources":["rm -rf build"]}});
        assert_eq!(
            n.normalize(&asked),
            Some(BackendEvent::PermissionAsked(PermissionRequest {
                id: "per_1".into(),
                session: "s1".into(),
                kind: "bash".into(),
                title: "bash: rm -rf build".into(),
                patterns: vec!["rm -rf build".into()],
            }))
        );
        let replied = json!({"type":"permission.replied","data":{"sessionID":"s1","requestID":"per_1","reply":"once"}});
        assert_eq!(n.normalize(&replied), Some(BackendEvent::PermissionReplied { session: "s1".into(), id: "per_1".into() }));
    }

    #[test]
    fn unknown_and_malformed_events_are_ignored() {
        let mut n = Normalizer::default();
        assert_eq!(n.normalize(&json!({"type":"lsp.updated","data":{}})), None);
        assert_eq!(n.normalize(&json!({"type":"session.execution.started","data":{}})), None);
        assert_eq!(n.normalize(&json!({"nope":1})), None);
        assert_eq!(n.normalize(&json!("text")), None);
    }
}
