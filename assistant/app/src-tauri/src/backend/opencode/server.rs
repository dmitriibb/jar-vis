//! Finding (and if needed starting) the opencode server.
//!
//! opencode v2 runs a per-user background service on a dynamic port. It writes
//! its URL and password to `<state dir>/opencode/service.json`, and the TUI and
//! web UI connect to that same service. Jar-vis does too, so a session it drives
//! updates live everywhere else.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use crate::config::OpencodeCfg;
use crate::launch;

#[derive(Debug, Clone)]
pub struct Conn {
    pub url: String,
    pub username: String,
    pub password: Option<String>,
    pub version: String,
}

#[derive(Debug, Deserialize)]
struct ServiceFile {
    url: String,
    #[serde(default)]
    password: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ServerInfo {
    version: String,
}

/// opencode uses XDG-style paths on every OS.
pub fn opencode_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir).join("opencode");
    }
    directories::BaseDirs::new()
        .map(|b| b.home_dir().join(".local").join("state").join("opencode"))
        .unwrap_or_else(|| PathBuf::from(".local/state/opencode"))
}

fn read_service_file() -> Option<ServiceFile> {
    let text = std::fs::read_to_string(opencode_state_dir().join("service.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// `GET /api/info`. Returns the server version when reachable and authorized.
pub async fn probe(http: &reqwest::Client, url: &str, username: &str, password: Option<&str>) -> Result<String> {
    let mut req = http.get(format!("{}/api/info", url.trim_end_matches('/'))).timeout(Duration::from_secs(5));
    if let Some(pw) = password {
        req = req.basic_auth(username, Some(pw));
    }
    let res = req.send().await.context("opencode is not reachable")?;
    if res.status() == reqwest::StatusCode::UNAUTHORIZED {
        bail!("opencode rejected the credentials");
    }
    if !res.status().is_success() {
        bail!("opencode answered {}", res.status());
    }
    let info: ServerInfo = res.json().await.context("unexpected /api/info response")?;
    Ok(info.version)
}

async fn probe_service_file(http: &reqwest::Client, cfg: &OpencodeCfg) -> Result<Conn> {
    let file = read_service_file().ok_or_else(|| anyhow!("the opencode background service is not running"))?;
    let version = probe(http, &file.url, &cfg.username, file.password.as_deref()).await?;
    Ok(Conn {
        url: file.url.trim_end_matches('/').to_string(),
        username: cfg.username.clone(),
        password: file.password,
        version,
    })
}

/// Outcome of [`discover`]: the connection, and whether Jar-vis started the service.
pub struct Discovered {
    pub conn: Conn,
    pub started: bool,
}

pub async fn discover(http: &reqwest::Client, cfg: &OpencodeCfg) -> Result<Discovered> {
    // An explicit URL bypasses the background service entirely.
    if !cfg.url.trim().is_empty() {
        let url = cfg.url.trim().trim_end_matches('/').to_string();
        let password = Some(cfg.password.clone()).filter(|p| !p.is_empty());
        let version = probe(http, &url, &cfg.username, password.as_deref()).await?;
        return Ok(Discovered { conn: Conn { url, username: cfg.username.clone(), password, version }, started: false });
    }

    let first = match probe_service_file(http, cfg).await {
        Ok(conn) => return Ok(Discovered { conn, started: false }),
        Err(e) => e,
    };
    if !cfg.autostart {
        return Err(first);
    }

    let binary = launch::find_opencode(&cfg.binary)
        .ok_or_else(|| anyhow!("opencode not found — set its path in Settings"))?;
    tracing::info!("starting the opencode background service with {}", binary.display());
    let mut cmd = launch::quiet_command(&binary);
    cmd.args(["service", "start"]);
    let output = tokio::time::timeout(Duration::from_secs(30), tokio::process::Command::from(cmd).output())
        .await
        .map_err(|_| anyhow!("`opencode service start` timed out"))?
        .context("could not run opencode")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("`opencode service start` failed: {}", err.trim());
    }

    // The service writes service.json once it is listening.
    let mut last = first;
    for _ in 0..30 {
        match probe_service_file(http, cfg).await {
            Ok(conn) => return Ok(Discovered { conn, started: true }),
            Err(e) => last = e,
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(last.context("the opencode background service did not come up"))
}

/// `opencode service stop`, used on exit when Jar-vis started the service.
pub fn stop_service(cfg: &OpencodeCfg) {
    if let Some(binary) = launch::find_opencode(&cfg.binary) {
        let mut cmd = launch::quiet_command(&binary);
        cmd.args(["service", "stop"]);
        if let Err(e) = cmd.status() {
            tracing::warn!("could not stop the opencode service: {e}");
        }
    }
}
