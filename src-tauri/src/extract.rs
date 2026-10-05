use std::path::Path;

pub const CHUNK_CHARS: usize = 1800;
pub const CHUNK_OVERLAP: usize = 200;
pub const MAX_TEXT_BYTES: u64 = 2_000_000;
pub const MAX_PDF_BYTES: u64 = 32_000_000;
pub const MAX_CHUNKS: usize = 400;
pub const MAX_CHUNKS_PER_FILE: usize = 80;
pub const EMBED_CHARS: usize = 1800;

pub fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn strip_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn read_text(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let ext = ext_of(path);
    let cap = if ext == "pdf" { MAX_PDF_BYTES } else { MAX_TEXT_BYTES };
    if meta.len() > cap {
        return Err(format!("too large ({} bytes)", meta.len()));
    }
    if ext == "pdf" {
        return pdf_text(path);
    }
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    if ext == "html" || ext == "htm" {
        return Ok(strip_html(&raw));
    }
    Ok(raw)
}

fn pdf_text(path: &Path) -> Result<String, String> {
    pdf_extract::extract_text(path).map_err(|e| format!("pdf: {e}"))
}

/// Split on paragraph boundaries where possible; hard-cut when a block is larger
/// than the window. Same geometry as hive sources (1800 / 200).
///
/// Indices are always snapped to UTF-8 char boundaries. A curly apostrophe is
/// three bytes; slicing at start+1800 without snapping panics inside it.
fn floor_char(s: &str, mut i: usize) -> usize {
    if i > s.len() {
        i = s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char(s: &str, mut i: usize) -> usize {
    if i > s.len() {
        return s.len();
    }
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

pub fn chunk_text(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.len() <= CHUNK_CHARS {
        return vec![trimmed.to_string()];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < trimmed.len() {
        start = ceil_char(trimmed, start);
        if start >= trimmed.len() {
            break;
        }
        let mut end = ceil_char(trimmed, start.saturating_add(CHUNK_CHARS).min(trimmed.len()));
        if end < trimmed.len() {
            let window = &trimmed[start..end];
            if let Some(rel) = window.rfind("\n\n") {
                end = start + rel + 2;
            } else if let Some(rel) = window.rfind('\n') {
                end = start + rel + 1;
            }
            end = floor_char(trimmed, end);
            if end <= start {
                end = ceil_char(trimmed, start.saturating_add(CHUNK_CHARS).min(trimmed.len()));
            }
        }
        let piece = trimmed[start..end].trim();
        if !piece.is_empty() {
            chunks.push(piece.to_string());
        }
        if end >= trimmed.len() {
            break;
        }
        let mut next = floor_char(trimmed, end.saturating_sub(CHUNK_OVERLAP));
        if next <= start {
            next = end;
        }
        start = next;
        if chunks.len() >= MAX_CHUNKS {
            break;
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_one_chunk() {
        assert_eq!(chunk_text("hello"), vec!["hello".to_string()]);
        assert!(chunk_text("   ").is_empty());
    }

    #[test]
    fn long_text_overlaps() {
        let s = "a".repeat(CHUNK_CHARS + 400);
        let c = chunk_text(&s);
        assert!(c.len() >= 2);
        assert!(c[0].len() <= CHUNK_CHARS);
    }

    #[test]
    fn html_drops_tags() {
        assert_eq!(strip_html("<p>Hi <b>there</b></p>"), "Hi there");
    }

    #[test]
    fn em_dash_does_not_panic() {
        let mark = "\u{2014}";
        assert_eq!(mark.len(), 3);
        let text = format!("{}{}{}", "a".repeat(CHUNK_CHARS - 1), mark, "b".repeat(CHUNK_CHARS));
        let chunks = chunk_text(&text);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|c| c.is_char_boundary(0) || c.is_empty()));
    }

    #[test]
    fn curly_apostrophe_does_not_panic() {
        // U+2019 is 3 bytes. Place it so start+CHUNK_CHARS lands inside it —
        // the live ingest panic was this slice.
        let mark = "\u{2019}";
        assert_eq!(mark.len(), 3);
        let text = format!("{}{}{}", "x".repeat(CHUNK_CHARS - 1), mark, "y".repeat(CHUNK_CHARS));
        let chunks = chunk_text(&text);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().any(|c| c.contains('\u{2019}') || c.contains('x')));
    }
}
