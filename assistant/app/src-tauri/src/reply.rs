//! Split an agent reply into the part that is spoken and the part that is only shown.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub summary: String,
    pub details: String,
}

pub const SEPARATOR: &str = "-----------------------------------------";
pub const MAX_SUMMARY_WORDS: usize = 50;

/// Split on the first standalone separator outside fenced code blocks.
/// Accept `---` for replies saved before this protocol was introduced.
pub fn split(text: &str, max_spoken_chars: usize) -> Reply {
    let text = text.trim();
    let mut offset = 0;
    let mut fence: Option<(char, usize)> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker = trimmed.chars().next().unwrap();
            let len = trimmed.chars().take_while(|c| *c == marker).count();
            match fence {
                None => fence = Some((marker, len)),
                Some((open, count)) if open == marker && len >= count
                    && trimmed[len..].trim().is_empty() => fence = None,
                _ => {},
            }
        } else if fence.is_none() && (trimmed == SEPARATOR || trimmed == "---") && offset > 0 {
            let intro = text[..offset].trim();
            if !intro.is_empty() {
                return Reply {
                    summary: limit_summary(intro, max_spoken_chars),
                    details: text[offset + line.len()..].trim().to_string(),
                };
            }
        }
        offset += line.len();
    }
    let summary = limit_summary(&first_sentences(text, 2, max_spoken_chars), max_spoken_chars);
    let details = if summary == text { String::new() } else { text.to_string() };
    Reply { summary, details }
}

pub fn limit_summary(text: &str, max_chars: usize) -> String {
    let mut words = text.split_whitespace();
    let limited = words.by_ref().take(MAX_SUMMARY_WORDS).collect::<Vec<_>>().join(" ");
    let limited = if words.next().is_some() { format!("{limited}…") } else { limited };
    truncate_at_sentence(&limited, max_chars)
}

/// The first `count` sentences of `text`, cut to at most `max_chars`.
pub fn first_sentences(text: &str, count: usize, max_chars: usize) -> String {
    let first_block = text.split("\n\n").next().unwrap_or(text).trim();
    let mut end = 0;
    let mut found = 0;
    for (start, len) in sentence_spans(first_block) {
        end = start + len;
        found += 1;
        if found == count {
            break;
        }
    }
    let picked = if end == 0 { first_block } else { first_block[..end].trim() };
    truncate_at_sentence(picked, max_chars)
}

/// Cut `text` to at most `max_chars`, preferring a sentence boundary, then a word boundary.
pub fn truncate_at_sentence(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let limit = text.char_indices().nth(max_chars).map(|(i, _)| i).unwrap_or(text.len());
    let mut best = 0;
    for (start, len) in sentence_spans(text) {
        if start + len <= limit {
            best = start + len;
        } else {
            break;
        }
    }
    if best > 0 {
        return text[..best].trim().to_string();
    }
    let head = &text[..limit];
    let cut = head.rfind(char::is_whitespace).unwrap_or(limit);
    format!("{}…", head[..cut].trim_end())
}

/// Byte spans `(start, len)` of the sentences in `text`. A sentence ends at
/// `.`, `!` or `?` followed by whitespace or the end of the text.
pub fn sentence_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(c, '.' | '!' | '?') {
            let at_end = match chars.peek() {
                None => true,
                Some((_, next)) => next.is_whitespace(),
            };
            // "3.5" and "e.g. x" style dots inside tokens are not sentence ends.
            if at_end {
                let end = i + c.len_utf8();
                if !text[start..end].trim().is_empty() {
                    spans.push((start, end - start));
                }
                start = end;
            }
        }
    }
    if !text[start..].trim().is_empty() {
        spans.push((start, text.len() - start));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_separator_and_limits_apply_to_formatted_replies() {
        let text = format!("{}\n{SEPARATOR}\nFull answer", "word ".repeat(80));
        let r = split(&text, 10000);
        assert_eq!(r.summary.split_whitespace().count(), 50);
        assert_eq!(r.details, "Full answer");
        assert!(split(&text, 40).summary.chars().count() <= 41);
    }

    #[test]
    fn does_not_split_code_fences_or_later_separators() {
        let text = format!("Intro.\n```text\n{SEPARATOR}\n```\n{SEPARATOR}\nDetails\n{SEPARATOR}\nMore.");
        let r = split(&text, 1000);
        assert!(r.summary.contains("```"));
        assert_eq!(r.details, format!("Details\n{SEPARATOR}\nMore."));
    }

    #[test]
    fn splits_on_separator() {
        let r = split("All tests pass.\n---\n## Details\n- 12 passed", 400);
        assert_eq!(r.summary, "All tests pass.");
        assert_eq!(r.details, "## Details\n- 12 passed");
    }

    #[test]
    fn separator_with_crlf_and_spaces() {
        let r = split("Done.\r\n ---  \r\nMore here.", 400);
        assert_eq!(r.summary, "Done.");
        assert_eq!(r.details, "More here.");
    }

    #[test]
    fn leading_separator_is_not_a_split() {
        let r = split("---\nJust text. And more. And a third.", 400);
        assert_eq!(r.summary, "--- Just text. And more.");
    }

    #[test]
    fn no_separator_takes_two_sentences() {
        let r = split("One. Two! Three? Four.", 400);
        assert_eq!(r.summary, "One. Two!");
        assert_eq!(r.details, "One. Two! Three? Four.");
    }

    #[test]
    fn short_reply_has_no_details() {
        let r = split("Four.", 400);
        assert_eq!(r.summary, "Four.");
        assert_eq!(r.details, "");
    }

    #[test]
    fn decimals_do_not_end_sentences() {
        let r = split("Version 3.5 is out. It is fast. Third.", 400);
        assert_eq!(r.summary, "Version 3.5 is out. It is fast.");
    }

    #[test]
    fn summary_is_capped() {
        let long = "word ".repeat(200);
        let r = split(&long, 50);
        assert!(r.summary.chars().count() <= 51);
        assert!(r.summary.ends_with('…'));
    }

    #[test]
    fn truncates_at_sentence_boundary() {
        let t = truncate_at_sentence("First sentence here. Second sentence is longer than the cap.", 30);
        assert_eq!(t, "First sentence here.");
    }
}
