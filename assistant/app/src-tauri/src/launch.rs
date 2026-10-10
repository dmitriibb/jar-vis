//! Finding the opencode binary and opening a session in other tools.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};

use crate::backend::SessionRef;
use crate::config::OpenchamberCfg;

/// A `Command` that does not flash a console window on Windows.
pub fn quiet_command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Resolve the opencode executable. GUI apps on macOS do not inherit the shell
/// `PATH`, so well-known install locations are checked as well.
pub fn find_opencode(configured: &str) -> Option<PathBuf> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let path = PathBuf::from(configured);
        return path.is_file().then_some(path);
    }
    if let Ok(path) = which::which("opencode") {
        return Some(path);
    }
    let home = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf());
    let mut candidates: Vec<PathBuf> = Vec::new();
    let exe = if cfg!(windows) { "opencode.exe" } else { "opencode" };
    if let Some(home) = &home {
        candidates.push(home.join(".opencode").join("bin").join(exe));
        candidates.push(home.join(".bun").join("bin").join(exe));
        candidates.push(home.join(".local").join("bin").join(exe));
        candidates.push(home.join(".npm-global").join("bin").join(exe));
    }
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            candidates.push(PathBuf::from(appdata).join("npm").join("opencode.cmd"));
        }
    } else {
        candidates.push(PathBuf::from("/opt/homebrew/bin/opencode"));
        candidates.push(PathBuf::from("/usr/local/bin/opencode"));
    }
    if let Some(found) = candidates.into_iter().find(|p| p.is_file()) {
        return Some(found);
    }
    login_shell_lookup()
}

/// Last resort on macOS: ask the user's login shell where opencode is.
#[cfg(target_os = "macos")]
fn login_shell_lookup() -> Option<PathBuf> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let out = Command::new(shell).args(["-lic", "command -v opencode"]).output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout).lines().last()?.trim().to_string();
    let path = PathBuf::from(path);
    path.is_file().then_some(path)
}

#[cfg(not(target_os = "macos"))]
fn login_shell_lookup() -> Option<PathBuf> {
    None
}

/// Open a terminal in the session directory running the opencode TUI on that session.
pub fn open_in_tui(session: &SessionRef, opencode_binary: &str) -> Result<()> {
    let binary = find_opencode(opencode_binary).ok_or_else(|| anyhow!("opencode not found"))?;
    let dir = &session.directory;
    open_terminal(dir, &binary, &["--session", &session.id])
}

#[cfg(windows)]
fn open_terminal(dir: &Path, program: &Path, args: &[&str]) -> Result<()> {
    // Windows Terminal when installed, else a classic console window.
    if let Ok(wt) = which::which("wt.exe") {
        let ok = Command::new(wt).arg("-d").arg(dir).arg(program).args(args).spawn().is_ok();
        if ok {
            return Ok(());
        }
    }
    Command::new("cmd")
        .args(["/c", "start", "", "cmd", "/k"])
        .arg(program)
        .args(args)
        .current_dir(dir)
        .spawn()
        .context("could not open a terminal")?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_terminal(dir: &Path, program: &Path, args: &[&str]) -> Result<()> {
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let mut line = format!("cd {} && {}", quote(&dir.to_string_lossy()), quote(&program.to_string_lossy()));
    for arg in args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    let script = format!(
        "tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell",
        line.replace('\\', "\\\\").replace('"', "\\\"")
    );
    Command::new("osascript").args(["-e", &script]).spawn().context("could not open Terminal")?;
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn open_terminal(dir: &Path, program: &Path, args: &[&str]) -> Result<()> {
    Command::new("x-terminal-emulator")
        .arg("-e")
        .arg(program)
        .args(args)
        .current_dir(dir)
        .spawn()
        .context("could not open a terminal")?;
    Ok(())
}

/// What `open_in_openchamber` should do, decided from the config.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenchamberTarget {
    Url(String),
    App(PathBuf),
}

pub fn openchamber_target(cfg: &OpenchamberCfg) -> Option<OpenchamberTarget> {
    if !cfg.url.trim().is_empty() {
        Some(OpenchamberTarget::Url(cfg.url.trim().to_string()))
    } else if !cfg.app_path.trim().is_empty() {
        Some(OpenchamberTarget::App(PathBuf::from(cfg.app_path.trim())))
    } else {
        None
    }
}

/// Launch a desktop app by path (`open -a` on macOS).
pub fn launch_app(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-a").arg(path).spawn().context("could not launch the app")?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        Command::new(path).spawn().context("could not launch the app")?;
    }
    Ok(())
}
