use crate::census::{kind_of, walk_files};
use crate::extract::{ext_of, read_text};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn one() -> u32 {
    1
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GraphNode {
    pub id: String,
    pub kind: String,
    pub label: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub rel: String,
    #[serde(default = "one")]
    pub weight: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FolderGraph {
    pub path: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub truncated: bool,
}

fn rel_id(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn resolve(from: &Path, spec: &str, index: &HashMap<String, PathBuf>) -> Option<PathBuf> {
    if spec.starts_with("http:") || spec.starts_with("https:") || spec.starts_with("mailto:") {
        return None;
    }
    if spec.starts_with("crate:") || spec.starts_with('@') {
        return None;
    }
    let spec = spec.trim_start_matches("./");
    if spec.is_empty() || spec.starts_with("node_modules/") {
        return None;
    }
    let base = from.parent()?;
    let candidate = base.join(spec);
    let lookup = |p: &Path| index.get(&p.to_string_lossy().replace('\\', "/")).cloned();
    if let Some(p) = lookup(&candidate) {
        return Some(p);
    }
    for extra in [".ts", ".tsx", ".js", ".jsx", ".rs", ".md", ".py"] {
        let c = PathBuf::from(format!("{}{extra}", candidate.to_string_lossy()));
        if let Some(p) = lookup(&c) {
            return Some(p);
        }
    }
    None
}

fn scan_quoted(text: &str, needles: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for needle in needles {
        let mut from = 0;
        while let Some(i) = text[from..].find(needle) {
            let abs = from + i + needle.len();
            let rest = text[abs..].trim_start();
            let quote = rest.chars().next();
            if quote == Some('\'') || quote == Some('"') {
                let q = quote.unwrap();
                if let Some(end) = rest[1..].find(q) {
                    out.push(rest[1..1 + end].to_string());
                }
            }
            from = abs + 1;
        }
    }
    out
}

fn scan_mod(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find("mod ") {
        let abs = from + i + 4;
        let rest = text[abs..].trim_start();
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() && rest[name.len()..].trim_start().starts_with(';') {
            out.push(format!("{name}.rs"));
        }
        from = abs + 1;
    }
    out
}

fn scan_md_links(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find("](") {
        let abs = from + i + 2;
        if let Some(end) = text[abs..].find(')') {
            let spec = text[abs..abs + end]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            if !spec.is_empty() {
                out.push(spec);
            }
            from = abs + end + 1;
        } else {
            break;
        }
    }
    out
}

pub const MAX_GRAPH_NODES: usize = 400;

pub fn folder_graph(root: &Path) -> FolderGraph {
    let (files, walk_truncated) = walk_files(root);
    let mut index: HashMap<String, PathBuf> = HashMap::new();
    for f in &files {
        index.insert(f.to_string_lossy().replace('\\', "/"), f.clone());
    }
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut truncated = walk_truncated;
    for path in &files {
        let ext = ext_of(path);
        let Some(kind) = kind_of(&ext) else { continue };
        if nodes.len() >= MAX_GRAPH_NODES {
            truncated = true;
            break;
        }
        let id = rel_id(root, path);
        nodes.push(GraphNode {
            id: id.clone(),
            kind: kind.to_string(),
            label: path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| id.clone()),
        });
        if kind == "image" || kind == "office" || kind == "pdf" {
            continue;
        }
        let text = match read_text(path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let mut specs = scan_quoted(&text, &["from ", "require("]);
        specs.extend(scan_quoted(&text, &["import "]));
        if ext == "rs" {
            specs.extend(scan_mod(&text));
        }
        if ext == "md" || ext == "html" || ext == "htm" {
            specs.extend(scan_md_links(&text));
        }
        for spec in specs {
            if let Some(target) = resolve(path, &spec, &index) {
                let to = rel_id(root, &target);
                if to != id {
                    edges.push(GraphEdge {
                        from: id.clone(),
                        to,
                        rel: "imports".into(),
                        weight: 1,
                    });
                }
            }
        }
    }
    let mut g = FolderGraph {
        path: root.to_string_lossy().into_owned(),
        nodes,
        edges,
        truncated,
    };
    add_sibling_edges(&mut g);
    g
}

fn add_sibling_edges(graph: &mut FolderGraph) {
    use std::collections::BTreeMap;
    let mut by_dir: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for n in &graph.nodes {
        if n.kind == "chunk" {
            continue;
        }
        let dir = n.id.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        by_dir.entry(dir.to_string()).or_default().push(n.id.clone());
    }
    for ids in by_dir.values_mut() {
        ids.sort();
        if ids.len() < 2 {
            continue;
        }
        for w in ids.windows(2) {
            graph.edges.push(GraphEdge {
                from: w[0].clone(),
                to: w[1].clone(),
                rel: "sibling".into(),
                weight: 1,
            });
        }
        if ids.len() > 2 {
            graph.edges.push(GraphEdge {
                from: ids[ids.len() - 1].clone(),
                to: ids[0].clone(),
                rel: "sibling".into(),
                weight: 1,
            });
        }
    }
}

#[derive(Clone, Debug)]
pub struct IngestedChunk {
    pub rel_path: String,
    pub chunk_index: u32,
}

/// File nodes stay; each ingested chunk becomes a gem hanging off its file,
/// chained NEXT along the document. This is what makes ingest visible in the verse.
pub fn graft_chunks(graph: &mut FolderGraph, chunks: &[IngestedChunk]) {
    use std::collections::{BTreeMap, HashSet};
    let files: HashSet<String> = graph
        .nodes
        .iter()
        .filter(|n| n.kind != "chunk")
        .map(|n| n.id.clone())
        .collect();
    let existing: HashSet<String> = graph.nodes.iter().map(|n| n.id.clone()).collect();
    let mut by_file: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for c in chunks {
        by_file.entry(c.rel_path.clone()).or_default().push(c.chunk_index);
        let id = format!("{}#{}", c.rel_path, c.chunk_index);
        if existing.contains(&id) {
            continue;
        }
        let stem = c.rel_path.rsplit('/').next().unwrap_or(&c.rel_path);
        graph.nodes.push(GraphNode {
            id: id.clone(),
            kind: "chunk".into(),
            label: format!("{stem}#{}", c.chunk_index),
        });
        if files.contains(&c.rel_path) {
            graph.edges.push(GraphEdge {
                from: c.rel_path.clone(),
                to: id,
                rel: "contains".into(),
                weight: 1,
            });
        }
    }
    for (rel, mut idxs) in by_file {
        idxs.sort_unstable();
        idxs.dedup();
        for w in idxs.windows(2) {
            graph.edges.push(GraphEdge {
                from: format!("{}#{}", rel, w[0]),
                to: format!("{}#{}", rel, w[1]),
                rel: "next".into(),
                weight: 1,
            });
        }
    }
}

pub fn graft_entities(
    graph: &mut FolderGraph,
    ents: &[crate::entities::Entity],
    edges: &[crate::entities::EntityEdge],
) {
    let existing: std::collections::HashSet<String> =
        graph.nodes.iter().map(|n| n.id.clone()).collect();
    let hidden: std::collections::HashSet<String> = ents
        .iter()
        .filter(|e| e.withheld)
        .map(|e| e.id.clone())
        .collect();
    for e in ents {
        if e.withheld || existing.contains(&e.id) {
            continue;
        }
        graph.nodes.push(GraphNode {
            id: e.id.clone(),
            kind: e.kind.clone(),
            label: e.name.clone(),
        });
    }
    for e in edges {
        if hidden.contains(&e.from) || hidden.contains(&e.to) {
            continue;
        }
        graph.edges.push(GraphEdge {
            from: e.from.clone(),
            to: e.to.clone(),
            rel: e.rel.clone(),
            weight: e.weight,
        });
    }
}

fn label_of(graph: &FolderGraph, id: &str) -> String {
    graph
        .nodes
        .iter()
        .find(|n| n.id == id)
        .map(|n| n.label.clone())
        .unwrap_or_else(|| id.to_string())
}

/// hop and blast as the chat endpoint sees them. `focus` is a node id.
pub fn run_graph_tool(name: &str, args: &serde_json::Value, graph: &FolderGraph) -> Result<serde_json::Value, String> {
    let focus = args.get("focus").and_then(|v| v.as_str()).unwrap_or("").trim();
    if focus.is_empty() {
        return Err("focus is required".into());
    }
    if !graph.nodes.iter().any(|n| n.id == focus) {
        return Err(format!("unknown node {focus}"));
    }
    match name {
        "hop" => {
            let ids = hop(graph, focus);
            let neighbors: Vec<_> = ids
                .iter()
                .map(|id| serde_json::json!({ "id": id, "label": label_of(graph, id) }))
                .collect();
            Ok(serde_json::json!({ "focus": focus, "neighbors": neighbors }))
        }
        "blast" => {
            let depth = args
                .get("depth")
                .and_then(|v| v.as_u64())
                .unwrap_or(2)
                .clamp(1, 3) as usize;
            let ids = blast(graph, focus, depth);
            let nodes: Vec<_> = ids
                .iter()
                .map(|id| serde_json::json!({ "id": id, "label": label_of(graph, id) }))
                .collect();
            Ok(serde_json::json!({ "focus": focus, "depth": depth, "nodes": nodes }))
        }
        _ => Err(format!("unknown graph tool {name}")),
    }
}

pub fn hop(graph: &FolderGraph, focus: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for e in &graph.edges {
        if e.from == focus {
            ids.push(e.to.clone());
        }
        if e.to == focus {
            ids.push(e.from.clone());
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

pub fn blast(graph: &FolderGraph, focus: &str, depth: usize) -> Vec<String> {
    use std::collections::{HashSet, VecDeque};
    let mut seen = HashSet::new();
    let mut q = VecDeque::new();
    q.push_back((focus.to_string(), 0));
    seen.insert(focus.to_string());
    while let Some((id, d)) = q.pop_front() {
        if d >= depth {
            continue;
        }
        for n in hop(graph, &id) {
            if seen.insert(n.clone()) {
                q.push_back((n, d + 1));
            }
        }
    }
    seen.remove(focus);
    let mut v: Vec<_> = seen.into_iter().collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn import_edge_and_blast() {
        let dir = std::env::temp_dir().join(format!("crystal-graph-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.ts"), "import { x } from './b'\n").unwrap();
        fs::write(dir.join("b.ts"), "export const x = 1\n").unwrap();
        let g = folder_graph(&dir);
        assert_eq!(g.nodes.len(), 2);
        assert!(!g.truncated);
        assert!(g.edges.iter().any(|e| e.from == "a.ts" && e.to == "b.ts"));
        assert_eq!(hop(&g, "a.ts"), vec!["b.ts".to_string()]);
        assert_eq!(blast(&g, "a.ts", 2), vec!["b.ts".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pdf_siblings_and_grafted_chunks() {
        let dir = std::env::temp_dir().join(format!("crystal-graft-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.pdf"), "%PDF").unwrap();
        fs::write(dir.join("b.pdf"), "%PDF").unwrap();
        let mut g = folder_graph(&dir);
        assert!(g.edges.iter().any(|e| e.rel == "sibling"));
        graft_chunks(
            &mut g,
            &[
                IngestedChunk { rel_path: "a.pdf".into(), chunk_index: 0 },
                IngestedChunk { rel_path: "a.pdf".into(), chunk_index: 1 },
            ],
        );
        assert!(g.nodes.iter().any(|n| n.id == "a.pdf#0" && n.kind == "chunk"));
        assert!(g.edges.iter().any(|e| e.rel == "contains" && e.from == "a.pdf"));
        assert!(g.edges.iter().any(|e| e.rel == "next"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn blast_reaches_the_node_two_steps_away() {
        let g = FolderGraph {
            path: "t".into(),
            truncated: false,
            nodes: vec![
                GraphNode { id: "a".into(), kind: "text".into(), label: "a".into() },
                GraphNode { id: "b".into(), kind: "text".into(), label: "bee".into() },
                GraphNode { id: "c".into(), kind: "concept".into(), label: "cee".into() },
            ],
            edges: vec![
                GraphEdge { from: "a".into(), to: "b".into(), rel: "relates_to".into(), weight: 2 },
                GraphEdge { from: "b".into(), to: "c".into(), rel: "relates_to".into(), weight: 4 },
            ],
        };
        assert_eq!(hop(&g, "a"), vec!["b".to_string()]);
        let far = blast(&g, "a", 2);
        assert!(far.iter().any(|id| id == "b") && far.iter().any(|id| id == "c"), "{far:?}");
        assert!(!far.iter().any(|id| id == "a"));
        let hopped = run_graph_tool("hop", &serde_json::json!({"focus": "a"}), &g).unwrap();
        assert_eq!(hopped["neighbors"][0]["label"], "bee");
        let blown = run_graph_tool("blast", &serde_json::json!({"focus": "a", "depth": 2}), &g).unwrap();
        assert_eq!(blown["nodes"].as_array().unwrap().len(), 2);
        assert!(run_graph_tool("hop", &serde_json::json!({"focus": "missing"}), &g).is_err());
    }
}
