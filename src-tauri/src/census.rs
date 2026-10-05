use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", ".git"];
const MAX_FILES: u32 = 10_000;
const SAMPLE_CAP: usize = 12;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct FolderCensus {
    pub path: String,
    pub markdown: u32,
    pub text: u32,
    pub pdf: u32,
    pub image: u32,
    pub html: u32,
    pub office: u32,
    pub code: u32,
    pub skipped: u32,
    pub truncated: bool,
    pub samples: Vec<String>,
}

pub fn kind_of(ext: &str) -> Option<&'static str> {
    match ext {
        "md" => Some("markdown"),
        "txt" => Some("text"),
        "pdf" => Some("pdf"),
        "png" | "jpg" | "jpeg" => Some("image"),
        "html" | "htm" => Some("html"),
        "doc" | "docx" | "ppt" | "pptx" => Some("office"),
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py" | "go" | "java"
        | "c" | "h" | "cpp" | "cc" | "hpp" => Some("code"),
        _ => None,
    }
}

pub fn skip_dir(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    if name.ends_with("_files") {
        return true;
    }
    SKIP_DIRS.iter().any(|s| *s == name)
}

/// True when the hive sources/code layers would try to read this as text.
pub fn is_text_ingest(ext: &str) -> bool {
    matches!(
        kind_of(ext),
        Some("markdown") | Some("text") | Some("html") | Some("code") | Some("pdf")
    )
}

pub fn walk_files(root: &Path) -> (Vec<PathBuf>, bool) {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    let mut seen: u32 = 0;
    let mut truncated = false;
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if seen >= MAX_FILES {
                truncated = true;
                return (out, truncated);
            }
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                if skip_dir(&name) {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !ft.is_file() || name.starts_with("~$") {
                continue;
            }
            seen += 1;
            out.push(path);
        }
    }
    (out, truncated)
}

pub fn census_dir(root: &Path) -> FolderCensus {
    let mut out = FolderCensus {
        path: root.to_string_lossy().into_owned(),
        markdown: 0,
        text: 0,
        pdf: 0,
        image: 0,
        html: 0,
        office: 0,
        code: 0,
        skipped: 0,
        truncated: false,
        samples: Vec::new(),
    };
    let (files, truncated) = walk_files(root);
    out.truncated = truncated;
    for path in files {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match kind_of(&ext) {
            Some("markdown") => out.markdown += 1,
            Some("text") => out.text += 1,
            Some("pdf") => out.pdf += 1,
            Some("image") => out.image += 1,
            Some("html") => out.html += 1,
            Some("office") => out.office += 1,
            Some("code") => out.code += 1,
            _ => {
                out.skipped += 1;
                continue;
            }
        }
        if out.samples.len() < SAMPLE_CAP {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            out.samples.push(rel);
        }
    }
    out
}
