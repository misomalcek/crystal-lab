use crate::entities::{Entity, EntityEdge};
use crate::rrf::{rrf_fuse_scored, RRF_K};
use crate::store::{SearchHit, Store};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, serde::Serialize)]
pub struct ChunkRec {
    pub path: String,
    pub chunk_index: u32,
    pub content: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct RetrieveReport {
    pub hits: Vec<SearchHit>,
    pub via_vector: u32,
    pub via_lexical: u32,
    pub via_graph: u32,
    pub graph_only_files: Vec<String>,
}

fn hit_id(h: &SearchHit) -> String {
    format!("{}#{}", h.path, h.chunk_index)
}

fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(|w| w.to_string())
        .collect()
}

/// Cheap BM25 over the in-memory chunk list (list B).
pub fn lexical_search(query: &str, chunks: &[ChunkRec], limit: usize) -> Vec<SearchHit> {
    if chunks.is_empty() {
        return vec![];
    }
    let q = tokenize(query);
    if q.is_empty() {
        return vec![];
    }
    let n = chunks.len() as f32;
    let mut df: HashMap<&str, u32> = HashMap::new();
    let docs: Vec<Vec<String>> = chunks.iter().map(|c| tokenize(&c.content)).collect();
    for terms in &docs {
        let uniq: HashSet<&str> = terms.iter().map(|s| s.as_str()).collect();
        for t in uniq {
            *df.entry(t).or_insert(0) += 1;
        }
    }
    let avgdl = docs.iter().map(|d| d.len() as f32).sum::<f32>() / n.max(1.0);
    let k1 = 1.2f32;
    let b = 0.75f32;
    let mut scored: Vec<(usize, f32)> = Vec::new();
    for (i, terms) in docs.iter().enumerate() {
        let dl = terms.len() as f32;
        let mut tf: HashMap<&str, u32> = HashMap::new();
        for t in terms {
            *tf.entry(t.as_str()).or_insert(0) += 1;
        }
        let mut score = 0.0f32;
        for qt in &q {
            let tfv = *tf.get(qt.as_str()).unwrap_or(&0) as f32;
            if tfv == 0.0 {
                continue;
            }
            let dfi = *df.get(qt.as_str()).unwrap_or(&0) as f32;
            let idf = (1.0 + (n - dfi + 0.5) / (dfi + 0.5)).ln();
            let denom = tfv + k1 * (1.0 - b + b * dl / avgdl.max(1.0));
            score += idf * (tfv * (k1 + 1.0)) / denom;
        }
        if score > 0.0 {
            scored.push((i, score));
        }
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(limit)
        .map(|(i, score)| {
            let c = &chunks[i];
            let preview: String = c.content.chars().take(220).collect();
            SearchHit {
                score,
                rrf_score: None,
                lists: vec!["lexical".into()],
                path: c.path.clone(),
                chunk_index: c.chunk_index,
                preview,
                payload: serde_json::json!({
                    "rel_path": c.path,
                    "chunk_index": c.chunk_index,
                    "content": c.content,
                }),
            }
        })
        .collect()
}

/// List C: from top vector entities, walk RELATES_TO by weight, take their chunks.
pub fn graph_expand(
    vector_hits: &[SearchHit],
    entities: &[Entity],
    edges: &[EntityEdge],
    chunks: &[ChunkRec],
    occupied: &HashSet<String>,
    limit: usize,
) -> Vec<SearchHit> {
    if entities.is_empty() || vector_hits.is_empty() {
        return vec![];
    }
    let hit_files: HashSet<String> = vector_hits.iter().map(|h| h.path.clone()).collect();
    let mut seed: Vec<(&Entity, u32)> = entities
        .iter()
        .filter(|e| !e.withheld && e.files.iter().any(|f| hit_files.contains(f)))
        .map(|e| (e, e.mentions))
        .collect();
    seed.sort_by(|a, b| b.1.cmp(&a.1));
    seed.truncate(8);

    let mut by_from: HashMap<String, Vec<&EntityEdge>> = HashMap::new();
    for e in edges {
        if e.rel == "relates_to" {
            by_from.entry(e.from.clone()).or_default().push(e);
            by_from.entry(e.to.clone()).or_default().push(e);
        }
    }
    for v in by_from.values_mut() {
        v.sort_by(|a, b| b.weight.cmp(&a.weight));
    }

    let entity_by_id: HashMap<&str, &Entity> = entities.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut related_files: Vec<(String, u32)> = Vec::new();
    for (ent, _) in &seed {
        if let Some(nbrs) = by_from.get(&ent.id) {
            for edge in nbrs {
                let other = if edge.from == ent.id {
                    &edge.to
                } else {
                    &edge.from
                };
                if let Some(oe) = entity_by_id.get(other.as_str()) {
                    for f in &oe.files {
                        if !hit_files.contains(f) {
                            related_files.push((f.clone(), edge.weight));
                        }
                    }
                }
            }
        }
    }
    related_files.sort_by(|a, b| b.1.cmp(&a.1));

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (file, weight) in related_files {
        for c in chunks.iter().filter(|c| c.path == file) {
            let id = format!("{}#{}", c.path, c.chunk_index);
            if occupied.contains(&id) || !seen.insert(id) {
                continue;
            }
            let preview: String = c.content.chars().take(220).collect();
            out.push(SearchHit {
                score: weight as f32,
                rrf_score: None,
                lists: vec!["graph".into()],
                path: c.path.clone(),
                chunk_index: c.chunk_index,
                preview,
                payload: serde_json::json!({
                    "rel_path": c.path,
                    "chunk_index": c.chunk_index,
                    "content": c.content,
                    "via": "relates_to",
                    "weight": weight,
                }),
            });
            if out.len() >= limit {
                return out;
            }
        }
    }
    out
}

pub fn fuse_retrieve(
    store: &Store,
    query: &str,
    folder: Option<&str>,
    chunks: &[ChunkRec],
    entities: &[Entity],
    edges: &[EntityEdge],
    limit: usize,
) -> Result<RetrieveReport, String> {
    let a = store.search_vector(query, folder, limit.max(8))?;
    let b = lexical_search(query, chunks, limit.max(8));
    let mut occupied: HashSet<String> = a.iter().map(hit_id).chain(b.iter().map(hit_id)).collect();
    let c = graph_expand(&a, entities, edges, chunks, &occupied, limit.max(8));
    for h in &c {
        occupied.insert(hit_id(h));
    }

    let fused = rrf_fuse_scored(&[a.clone(), b.clone(), c.clone()], hit_id, limit, RRF_K);
    let a_ids: HashSet<String> = a.iter().map(hit_id).collect();
    let b_ids: HashSet<String> = b.iter().map(hit_id).collect();
    let c_ids: HashSet<String> = c.iter().map(hit_id).collect();

    let mut via_vector = 0u32;
    let mut via_lexical = 0u32;
    let mut via_graph = 0u32;
    let mut graph_only_files = Vec::new();
    let mut hits = Vec::new();
    for (mut hit, rrf) in fused {
        let id = hit_id(&hit);
        let mut lists = Vec::new();
        if a_ids.contains(&id) {
            lists.push("vector".into());
            via_vector += 1;
        }
        if b_ids.contains(&id) {
            lists.push("lexical".into());
            via_lexical += 1;
        }
        if c_ids.contains(&id) {
            lists.push("graph".into());
            via_graph += 1;
            if !a_ids.contains(&id) && !b_ids.contains(&id) {
                graph_only_files.push(hit.path.clone());
            }
        }
        hit.lists = lists;
        hit.rrf_score = Some(rrf as f32);
        hits.push(hit);
    }
    graph_only_files.sort();
    graph_only_files.dedup();
    Ok(RetrieveReport {
        hits,
        via_vector,
        via_lexical,
        via_graph,
        graph_only_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{Entity, EntityEdge};
    use std::collections::HashSet;

    #[test]
    fn graph_list_c_reaches_a_file_vector_missed() {
        let chunks = vec![
            ChunkRec {
                path: "paper-a.md".into(),
                chunk_index: 0,
                content: "Shannon at Bell Labs".into(),
            },
            ChunkRec {
                path: "paper-b.md".into(),
                chunk_index: 0,
                content: "Landauer limit and reversible computing".into(),
            },
        ];
        let vector = vec![SearchHit {
            score: 0.9,
            rrf_score: None,
            lists: vec!["vector".into()],
            path: "paper-a.md".into(),
            chunk_index: 0,
            preview: "Shannon".into(),
            payload: serde_json::json!({}),
        }];
        let entities = vec![
            Entity {
                id: "entity:shannon".into(),
                name: "Shannon".into(),
                kind: "person".into(),
                aliases: vec![],
                files: vec!["paper-a.md".into()],
                mentions: 4,
                ..Entity::default()
            },
            Entity {
                id: "entity:landauer".into(),
                name: "Landauer".into(),
                kind: "person".into(),
                aliases: vec![],
                files: vec!["paper-b.md".into()],
                mentions: 3,
                ..Entity::default()
            },
        ];
        let edges = vec![EntityEdge {
            from: "entity:shannon".into(),
            to: "entity:landauer".into(),
            rel: "relates_to".into(),
            weight: 12,
        }];
        let occupied: HashSet<String> = vector.iter().map(hit_id).collect();
        let c = graph_expand(&vector, &entities, &edges, &chunks, &occupied, 8);
        assert!(c.iter().any(|h| h.path == "paper-b.md"), "{c:?}");
    }

    #[test]
    fn lexical_finds_the_word() {
        let chunks = vec![
            ChunkRec {
                path: "a.md".into(),
                chunk_index: 0,
                content: "crystals absorb light in the lattice".into(),
            },
            ChunkRec {
                path: "b.md".into(),
                chunk_index: 0,
                content: "potatoes and weather reports".into(),
            },
        ];
        let hits = lexical_search("crystals lattice", &chunks, 4);
        assert_eq!(hits[0].path, "a.md");
    }
}
