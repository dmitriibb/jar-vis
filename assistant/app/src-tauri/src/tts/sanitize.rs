//! Turn markdown-ish text into something a voice can read.

use regex::Regex;

use crate::reply::{sentence_spans, truncate_at_sentence};

const CODE_NOTE: &str = " I put the code in the panel. ";

pub fn for_speech(text: &str, max_chars: usize) -> String {
    let re = |p: &str| Regex::new(p).expect("valid regex");

    // Fenced code blocks, including an unterminated last one.
    let mut out = re(r"(?s)```.*?(```|$)").replace_all(text, CODE_NOTE).into_owned();
    // Images and links keep their label.
    out = re(r"!\[([^\]]*)\]\([^)]*\)").replace_all(&out, "$1").into_owned();
    out = re(r"\[([^\]]+)\]\([^)]*\)").replace_all(&out, "$1").into_owned();
    out = re(r"<?https?://[^\s>)]+>?").replace_all(&out, "a link").into_owned();
    // Line-level markdown: headings, quotes, bullets, numbered items, rules.
    out = re(r"(?m)^\s{0,3}#{1,6}\s*").replace_all(&out, "").into_owned();
    out = re(r"(?m)^\s*>\s?").replace_all(&out, "").into_owned();
    out = re(r"(?m)^\s*[-*+]\s+").replace_all(&out, "").into_owned();
    out = re(r"(?m)^\s*\d+[.)]\s+").replace_all(&out, "").into_owned();
    out = re(r"(?m)^\s*([-*_]\s*){3,}$").replace_all(&out, "").into_owned();
    // Inline markers.
    out = re(r"`([^`]*)`").replace_all(&out, "$1").into_owned();
    out = re(r"(\*\*|__|~~)").replace_all(&out, "").into_owned();
    out = re(r"(^|[\s(])[*_]([^*_\n]+)[*_]").replace_all(&out, "$1$2").into_owned();
    out = out.replace('|', " ");
    // Symbols a voice would skip or mangle.
    out = out.replace(" & ", " and ").replace("&&", " and ").replace(" -> ", " to ").replace("=>", " to ");
    out = out.replace(" = ", " equals ").replace('%', " percent");
    out = re(r"[#*_`~<>]").replace_all(&out, "").into_owned();
    // A list item without end punctuation still needs a pause.
    out = re(r"([^\s.!?:;,])\s*\n+").replace_all(&out, "$1. ").into_owned();

    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let collapsed = collapsed.replace(" .", ".").replace(" ,", ",");
    truncate_at_sentence(&collapsed, max_chars).trim_end_matches('…').trim().to_string()
}

/// Sentences to synthesize one at a time, so playback can start early.
pub fn sentences(text: &str) -> Vec<String> {
    sentence_spans(text)
        .into_iter()
        .map(|(start, len)| text[start..start + len].trim().to_string())
        .filter(|s| s.chars().any(char::is_alphanumeric))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_markdown() {
        let s = for_speech("## Result\n**All 12 tests** pass in `cargo test`.", 400);
        assert_eq!(s, "Result. All 12 tests pass in cargo test.");
    }

    #[test]
    fn replaces_code_blocks_and_links() {
        let s = for_speech("Here it is:\n```rust\nfn main() {}\n```\nSee [the docs](https://example.com/x) or https://example.com.", 400);
        assert!(s.contains("I put the code in the panel."), "{s}");
        assert!(!s.contains("fn main"), "{s}");
        assert!(s.contains("See the docs or a link"), "{s}");
        assert!(!s.contains("http"), "{s}");
    }

    #[test]
    fn lists_become_sentences() {
        let s = for_speech("Two things:\n- fix the build\n- run the tests", 400);
        assert_eq!(s, "Two things: fix the build. run the tests");
    }

    #[test]
    fn expands_symbols() {
        assert_eq!(for_speech("Tom & Jerry are 50% done", 400), "Tom and Jerry are 50 percent done");
    }

    #[test]
    fn truncates_at_sentence() {
        let s = for_speech("First one. Second one is quite a bit longer than the limit allows.", 20);
        assert_eq!(s, "First one.");
    }

    #[test]
    fn sentence_list() {
        assert_eq!(sentences("Done. It works! Really?"), vec!["Done.", "It works!", "Really?"]);
        assert_eq!(sentences("..."), Vec::<String>::new());
    }
}
