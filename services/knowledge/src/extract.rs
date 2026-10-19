//! Deterministic extraction: no provider calls and no interpretation of source instructions.
use anyhow::{bail, Context, Result};

#[derive(Debug, Clone)]
pub struct Section {
    pub heading: String,
    pub page: Option<i32>,
    pub text: String,
}

pub fn extension(filename: &str) -> Result<&str> {
    let ext = filename.rsplit('.').next().unwrap_or("");
    match ext {
        "txt" | "md" | "pdf" => Ok(ext),
        _ => bail!("Supported files: UTF-8 .txt, .md, and text-based .pdf (lowercase extensions)"),
    }
}

pub fn extract(filename: &str, bytes: &[u8]) -> Result<Vec<Section>> {
    let pages = match extension(filename)? {
        "pdf" => pdf_extract::extract_text_from_mem_by_pages(bytes)
            .context("Cannot extract PDF; encrypted or malformed PDFs are unsupported")?
            .into_iter()
            .enumerate()
            .map(|(n, text)| (Some(n as i32 + 1), text))
            .collect(),
        _ => vec![(
            None,
            std::str::from_utf8(bytes)
                .context("Text must be UTF-8")?
                .to_string(),
        )],
    };
    let mut sections = Vec::new();
    for (page, text) in pages {
        let mut heading = page
            .map(|p| format!("Page {p}"))
            .unwrap_or_else(|| "Document".into());
        let mut buffer = String::new();
        for line in text.lines() {
            if filename.ends_with(".md") && line.starts_with('#') {
                flush(&mut sections, &heading, page, &mut buffer);
                heading = line.trim_start_matches('#').trim().to_string();
            } else {
                buffer.push_str(line);
                buffer.push('\n');
            }
        }
        flush(&mut sections, &heading, page, &mut buffer);
        if sections.len() > crate::MAX_SECTIONS {
            bail!("Document exceeds the {} section limit", crate::MAX_SECTIONS);
        }
    }
    if sections.is_empty() {
        bail!("No searchable text found. Scanned/image-only PDFs need OCR, which is not supported yet");
    }
    Ok(sections)
}

fn flush(sections: &mut Vec<Section>, heading: &str, page: Option<i32>, buffer: &mut String) {
    // Split on Unicode scalar boundaries. Prefer a paragraph/space boundary, while
    // retaining every character. No overlap yet: neighboring sections are readable.
    let mut rest = buffer.trim();
    while !rest.is_empty() {
        let end = rest
            .char_indices()
            .nth(1600)
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        let end = if end < rest.len() {
            rest[..end]
                .rfind(char::is_whitespace)
                .filter(|i| *i >= end / 2)
                .unwrap_or(end)
        } else {
            end
        };
        let part = rest[..end].trim();
        if !part.is_empty() {
            sections.push(Section {
                heading: heading.into(),
                page,
                text: part.into(),
            });
        }
        rest = rest[end..].trim_start();
    }
    buffer.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_unicode_and_long_sections_preserve_text() {
        let text = "棋".repeat(4000);
        let sections = extract(
            "rules.md",
            format!("# Setup\n{text}\n# Scoring\nScore ten points.").as_bytes(),
        )
        .unwrap();
        assert_eq!(sections.len(), 4);
        assert!(sections[..3].iter().all(|s| s.heading == "Setup"));
        assert_eq!(
            sections[..3]
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>(),
            text
        );
        assert_eq!(sections[3].heading, "Scoring");
    }

    #[test]
    fn rejects_empty_invalid_and_unsupported_sources() {
        assert!(extract("blank.txt", b" \n ").is_err());
        assert!(extract("invalid.txt", &[0xff]).is_err());
        assert!(extract("binary.exe", b"anything").is_err());
        assert!(extract("broken.pdf", b"not a PDF").is_err());
    }
}
