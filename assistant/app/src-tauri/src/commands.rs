//! Voice commands that Jar-vis handles itself instead of sending to the agent.

use regex::Regex;

use crate::backend::PermissionReply;
use crate::config::CommandsCfg;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start a new session. `rest` is sent as its first prompt when not empty.
    NewSession { rest: String },
    Stop,
    Mute,
    Unmute,
    Repeat,
    Permission(PermissionReply),
    Prompt(String),
}

/// Lowercase, drop punctuation, collapse whitespace.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_alphanumeric() || c == '\'' {
            out.extend(c.to_lowercase());
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out.trim().to_string()
}

/// Strip a leading wake word ("Jarvis, ...") from the original text.
fn strip_wake_word<'a>(text: &'a str, wake_word: &str) -> &'a str {
    let wake = wake_word.trim();
    if wake.is_empty() {
        return text;
    }
    let trimmed = text.trim_start();
    let Some(head) = trimmed.get(..wake.len()) else { return text };
    if !head.eq_ignore_ascii_case(wake) {
        return text;
    }
    let rest = &trimmed[wake.len()..];
    // Only a whole word counts: "jarvisual" is not a wake word.
    if rest.chars().next().is_some_and(|c| c.is_alphanumeric()) {
        return text;
    }
    rest.trim_start_matches(|c: char| !c.is_alphanumeric())
}

fn matches_any(normalized: &str, phrases: &[String]) -> bool {
    phrases.iter().any(|p| normalize(p) == normalized)
}

/// Matches "start a new session", "open new chat", "new conversation" and so on,
/// tolerating the punctuation Whisper puts between words.
fn new_session_regex() -> Regex {
    Regex::new(r"(?i)^\W*(?:(?:start|open|begin|create)\W+)?(?:an?\W+)?new\W+(?:session|chat|conversation)\b\W*")
        .expect("valid regex")
}

/// Classify one utterance. `awaiting_permission` enables the yes/no/always replies.
pub fn parse(text: &str, cfg: &CommandsCfg, awaiting_permission: bool) -> Command {
    let text = strip_wake_word(text.trim(), &cfg.wake_word).trim();
    let normalized = normalize(text);

    if awaiting_permission {
        if let Some(reply) = parse_permission(&normalized) {
            return Command::Permission(reply);
        }
    }

    if let Some(m) = new_session_regex().find(text) {
        return Command::NewSession { rest: text[m.end()..].trim().to_string() };
    }
    // Configured phrases the regex does not cover, as a prefix of the utterance.
    for phrase in &cfg.new_session {
        let p = normalize(phrase);
        if !p.is_empty() && normalized == p {
            return Command::NewSession { rest: String::new() };
        }
    }

    // These must be the whole utterance, so "stop the dev server" stays a prompt.
    if matches_any(&normalized, &cfg.stop) {
        return Command::Stop;
    }
    if matches_any(&normalized, &cfg.unmute) {
        return Command::Unmute;
    }
    if matches_any(&normalized, &cfg.mute) {
        return Command::Mute;
    }
    if matches_any(&normalized, &cfg.repeat) {
        return Command::Repeat;
    }
    Command::Prompt(text.to_string())
}

fn parse_permission(normalized: &str) -> Option<PermissionReply> {
    const ALWAYS: &[&str] = &["always", "always allow", "allow always", "yes always"];
    const ONCE: &[&str] = &[
        "yes", "yeah", "yep", "allow", "ok", "okay", "do it", "go ahead", "sure", "allow once", "yes please",
        "approve",
    ];
    const REJECT: &[&str] = &["no", "nope", "deny", "don't", "do not", "reject", "no thanks", "stop", "cancel"];
    if ALWAYS.contains(&normalized) {
        Some(PermissionReply::Always)
    } else if ONCE.contains(&normalized) {
        Some(PermissionReply::Once)
    } else if REJECT.contains(&normalized) {
        Some(PermissionReply::Reject)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Command {
        parse(text, &CommandsCfg::default(), false)
    }

    #[test]
    fn new_session_with_prompt() {
        assert_eq!(
            p("Start a new session. Check my email"),
            Command::NewSession { rest: "Check my email".into() }
        );
        assert_eq!(p("start new session, what's 2+2?"), Command::NewSession { rest: "what's 2+2?".into() });
        assert_eq!(p("Open a new chat and list the files"), Command::NewSession { rest: "and list the files".into() });
    }

    #[test]
    fn new_session_alone() {
        for t in ["New session.", "new chat", "Start a new conversation!", "Jarvis, start new session", "Begin new session"] {
            assert_eq!(p(t), Command::NewSession { rest: String::new() }, "{t}");
        }
    }

    #[test]
    fn new_session_must_lead() {
        assert!(matches!(p("How do I start a new session in tmux?"), Command::Prompt(_)));
        assert!(matches!(p("newsession thing"), Command::Prompt(_)));
    }

    #[test]
    fn whole_utterance_commands() {
        assert_eq!(p("Stop."), Command::Stop);
        assert_eq!(p("Cancel!"), Command::Stop);
        assert_eq!(p("Mute"), Command::Mute);
        assert_eq!(p("Be quiet."), Command::Mute);
        assert_eq!(p("unmute"), Command::Unmute);
        assert_eq!(p("Say that again?"), Command::Repeat);
        assert_eq!(p("Jarvis, repeat."), Command::Repeat);
    }

    #[test]
    fn commands_inside_sentences_are_prompts() {
        assert_eq!(p("Stop the dev server"), Command::Prompt("Stop the dev server".into()));
        assert_eq!(p("Repeat the last command in the terminal"), Command::Prompt("Repeat the last command in the terminal".into()));
    }

    #[test]
    fn wake_word_is_stripped_from_prompts() {
        assert_eq!(p("Jarvis, what time is it?"), Command::Prompt("what time is it?".into()));
        assert_eq!(p("jarvis what time is it"), Command::Prompt("what time is it".into()));
    }

    #[test]
    fn permission_replies_only_when_asked() {
        let cfg = CommandsCfg::default();
        assert_eq!(parse("Yes.", &cfg, true), Command::Permission(PermissionReply::Once));
        assert_eq!(parse("Do it!", &cfg, true), Command::Permission(PermissionReply::Once));
        assert_eq!(parse("Always", &cfg, true), Command::Permission(PermissionReply::Always));
        assert_eq!(parse("No.", &cfg, true), Command::Permission(PermissionReply::Reject));
        assert_eq!(parse("Don't", &cfg, true), Command::Permission(PermissionReply::Reject));
        assert_eq!(parse("Yes.", &cfg, false), Command::Prompt("Yes.".into()));
        // A real sentence while a permission is pending is still a prompt.
        assert!(matches!(parse("no, use the other branch instead", &cfg, true), Command::Prompt(_)));
    }

    #[test]
    fn normalize_handles_punctuation() {
        assert_eq!(normalize("  Start, a NEW   session!! "), "start a new session");
        assert_eq!(normalize("don't"), "don't");
    }
}
