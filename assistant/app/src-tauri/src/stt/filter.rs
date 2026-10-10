//! Whisper invents text for silence and noise. Drop the usual suspects.

use crate::commands::normalize;

const BLOCKLIST: &[&str] = &[
    "you",
    "thank you",
    "thanks",
    "thank you very much",
    "thanks for watching",
    "thank you for watching",
    "bye",
    "bye bye",
    "blank audio",
    "blank_audio",
    "silence",
    "music",
    "applause",
    "inaudible",
    "no speech",
    "so",
    "okay",
    "subtitles by the amara org community",
];

/// Remove bracketed annotations such as `[BLANK_AUDIO]`, `(silence)` or `*music*`.
fn strip_annotations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.replace('*', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The cleaned transcript, or None when it is a known hallucination or empty.
pub fn clean(text: &str) -> Option<String> {
    let stripped = strip_annotations(text);
    let normalized = normalize(&stripped).replace('_', " ");
    if normalized.is_empty() || BLOCKLIST.contains(&normalized.as_str()) {
        return None;
    }
    Some(stripped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_hallucinations() {
        for t in ["", "  ", "you", "You.", "Thank you.", "Thanks for watching!", "[BLANK_AUDIO]", "(silence)", "[Music]", " [BLANK_AUDIO] you"] {
            assert_eq!(clean(t), None, "{t:?}");
        }
    }

    #[test]
    fn keeps_real_speech() {
        assert_eq!(clean(" What's 2+2? ").as_deref(), Some("What's 2+2?"));
        assert_eq!(clean("Thank you for fixing the build, now run the tests").as_deref(), Some("Thank you for fixing the build, now run the tests"));
        assert_eq!(clean("[BLANK_AUDIO] start new session").as_deref(), Some("start new session"));
    }
}
