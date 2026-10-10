//! Runtime state that survives restarts (`state.json`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::backend::SessionRef;
use crate::config;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PersistedState {
    pub current_session: Option<SessionRef>,
    /// True when `current_session` belongs to another project (attach mode).
    pub attached: bool,
    /// The workspace session to return to after detaching.
    pub workspace_session: Option<SessionRef>,
    /// Physical screen position of the pet window.
    pub pet_position: Option<(i32, i32)>,
    pub panel_visible: bool,
    pub last_answer: Option<LastAnswer>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LastAnswer {
    pub summary: String,
    pub details: String,
}

pub fn state_path() -> PathBuf {
    config::data_dir().join("state.json")
}

pub fn load() -> PersistedState {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(state: &PersistedState) {
    match serde_json::to_vec_pretty(state) {
        Ok(bytes) => {
            if let Err(e) = config::write_atomic(&state_path(), &bytes) {
                tracing::warn!("could not save state.json: {e:#}");
            }
        }
        Err(e) => tracing::warn!("could not serialize state: {e}"),
    }
}
