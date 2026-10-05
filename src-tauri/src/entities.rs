use crate::store::Store;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

static CAP_PHRASE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\p{Lu}\p{Ll}{2,}(?:\s+(?:of|the|for|and|in|de|a|v|na|pro)\s+)?(?:\s+\p{Lu}\p{Ll}{2,}){1,3})").unwrap()
});
static ACRONYM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\p{Lu}[\p{Lu}0-9]{1,15})").unwrap()
});
static STRUCTURAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[IVXLCDM]+|PROP|MSS|PMID|PMC|ISSN|ISBN|DOI|PDF|HTML|URL|FIG|TAB|EQ|CHAP|SEC|VOL|NO|PP|ED|VS|ET|AL)$").unwrap()
});

const SENTENCE_STARTERS: &[&str] = &[
    "The", "This", "That", "These", "Those", "And", "But", "For", "However", "Therefore",
    "While", "When", "Where", "What", "Which", "There", "Here", "It", "In", "On", "At",
    "As", "If", "We", "They", "Our", "Their", "His", "Her", "Its", "One", "Two", "First",
    "Second", "Third", "Finally", "Thus", "Hence", "Since", "Because", "Although",
];

const BLOCK_THRESHOLD: f32 = 0.82;
const ADJUDICATION_BATCH: usize = 10;
const MIN_FREQUENCY: u32 = 2;
const MAX_ENTITIES: usize = 200;
const MAX_ADJUDICATED: usize = 80;
const MAX_SIGHTINGS: usize = 16;

static UUID_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$").unwrap()
});

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Sighting {
    pub path: String,
    pub chunk_index: u32,
    pub surface: String,
    pub count: u32,
}

/// A normalize-merge is an event. The surfaces that folded into this id stay here.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct MergeNote {
    pub reason: String,
    pub surfaces: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub files: Vec<String>,
    pub mentions: u32,
    #[serde(default)]
    pub sightings: Vec<Sighting>,
    #[serde(default)]
    pub merges: Vec<MergeNote>,
    #[serde(default)]
    pub flags: Vec<String>,
    #[serde(default)]
    pub withheld: bool,
    #[serde(default)]
    pub betweenness: f32,
    #[serde(default)]
    pub degree: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EntityEdge {
    pub from: String,
    pub to: String,
    pub rel: String,
    pub weight: u32,
}

#[derive(Clone, Debug)]
pub struct DistillDoc {
    pub rel_path: String,
    pub chunk_index: u32,
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct Verdict {
    pub input: String,
    pub keep: bool,
    pub canonical: String,
    pub kind: String,
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').chars().take(60).collect()
}

pub fn extract_candidates(text: &str) -> HashMap<String, u32> {
    let starters: HashSet<&str> = SENTENCE_STARTERS.iter().copied().collect();
    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut bump = |s: &str, by: u32| {
        let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.len() < 3 || t.len() > 60 {
            return;
        }
        if STRUCTURAL.is_match(&t) {
            return;
        }
        if t.split(' ').next().map(|w| starters.contains(w)).unwrap_or(false) {
            return;
        }
        *counts.entry(t).or_insert(0) += by;
    };
    for m in CAP_PHRASE.captures_iter(text) {
        if let Some(g) = m.get(1) {
            bump(g.as_str(), 1);
        }
    }
    for m in ACRONYM.captures_iter(text) {
        if let Some(g) = m.get(1) {
            bump(g.as_str(), 1);
        }
    }
    counts
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0;
    let mut na = 0.0;
    let mut nb = 0.0;
    let n = a.len().min(b.len());
    for i in 0..n {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    dot / (na.sqrt() * nb.sqrt()).max(1e-8)
}

/// Near-duplicate pairs. Callers flag these; they must not merge on cosine alone.
pub fn near_pairs(names: &[String], vecs: &[Vec<f32>], threshold: f32) -> Vec<(usize, usize, f32)> {
    let mut out = Vec::new();
    for i in 0..names.len() {
        for j in (i + 1)..names.len() {
            if names[i] == names[j] {
                continue;
            }
            let sim = cosine(&vecs[i], &vecs[j]);
            if sim >= threshold {
                out.push((i, j, sim));
            }
        }
    }
    out.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn completions_url(base: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    if b.ends_with("/chat/completions") {
        b.to_string()
    } else if b.ends_with("/v1") {
        format!("{b}/chat/completions")
    } else {
        format!("{b}/v1/chat/completions")
    }
}

pub fn probe_endpoint(url: &str, api_key: &str, model: &str) -> Result<(), String> {
    let body = json!({
        "model": model,
        "messages": [
            {"role": "user", "content": "Reply with the single word pong."}
        ],
        "max_tokens": 8
    });
    chat_raw(url, api_key, body)?;
    Ok(())
}

pub fn post_completion(url: &str, api_key: &str, body: Value) -> Result<Value, String> {
    let endpoint = completions_url(url);
    let mut req = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .post(&endpoint);
    if !api_key.trim().is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", api_key.trim()));
    }
    let resp = req.send_json(body).map_err(|e| format!("endpoint: {e}"))?;
    let status = resp.status();
    let text = resp.into_string().map_err(|e| e.to_string())?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "endpoint HTTP {status}: {}",
            text.chars().take(400).collect::<String>()
        ));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// An endpoint that rejects the tools field says so in the error body.
/// A plain 400 about the question does not count.
pub fn tools_refused(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("tool") || e.contains("function_call") || (e.contains("function") && e.contains("unsupported"))
}

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

pub fn tool_calls_from(message: &Value) -> Vec<ToolCall> {
    let Some(arr) = message.get("tool_calls").and_then(|v| v.as_array()) else {
        return vec![];
    };
    arr.iter()
        .filter_map(|call| {
            let id = call.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let fnv = call.get("function")?;
            let name = fnv.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if name.is_empty() {
                return None;
            }
            let arguments = match fnv.get("arguments") {
                Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(json!({})),
                Some(v @ Value::Object(_)) => v.clone(),
                _ => json!({}),
            };
            Some(ToolCall { id, name, arguments })
        })
        .collect()
}

pub fn message_text(message: &Value) -> String {
    message
        .get("content")
        .and_then(|v| v.as_str())
        .or_else(|| message.get("reasoning_content").and_then(|v| v.as_str()))
        .unwrap_or("")
        .trim()
        .to_string()
}

fn chat_raw(url: &str, api_key: &str, body: Value) -> Result<String, String> {
    let j = post_completion(url, api_key, body)?;
    let content = message_text(&j["choices"][0]["message"]);
    if content.is_empty() {
        return Err("endpoint returned an empty answer".into());
    }
    Ok(content)
}

pub fn chat_with_context(
    url: &str,
    api_key: &str,
    model: &str,
    question: &str,
    context: &str,
) -> Result<String, String> {
    let body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": "Answer from the provided source excerpts. If the excerpts do not contain the answer, say so. Do not invent citations."},
            {"role": "user", "content": format!("Excerpts:\n{context}\n\nQuestion: {question}")}
        ],
        "max_tokens": 700
    });
    chat_raw(url, api_key, body)
}

fn build_prompt(items: &[String]) -> String {
    let n = items.len();
    let mut lines = vec![
        format!("For EACH of the {n} items below output exactly one line, in the same order."),
        "Format:  KEEP <canonical name> <type>   or   DROP <reason>".into(),
        "Types: person, concept, technology, organization, work, place.".into(),
        "DROP document structure, roman numerals, citation artefacts, section headings.".into(),
        format!("You must output exactly {n} lines and nothing else."),
        String::new(),
    ];
    lines.extend(items.iter().cloned());
    lines.join("\n")
}

fn parse_verdicts(text: &str, items: &[String]) -> Option<Vec<Verdict>> {
    let mut lines: Vec<_> = text
        .trim()
        .lines()
        .map(|l| l.trim())
        .filter(|l| l.to_ascii_uppercase().starts_with("KEEP") || l.to_ascii_uppercase().starts_with("DROP"))
        .collect();
    if lines.len() < items.len() {
        return None;
    }
    lines.truncate(items.len());
    Some(
        lines
            .into_iter()
            .enumerate()
            .map(|(i, line)| {
                let keep = line.to_ascii_uppercase().starts_with("KEEP");
                if !keep {
                    return Verdict {
                        input: items[i].clone(),
                        keep: false,
                        canonical: items[i].clone(),
                        kind: String::new(),
                    };
                }
                let mut rest = line[4..].trim().to_string();
                if let Some(idx) = rest.rfind("KEEP ") {
                    rest = rest[idx + 5..].trim().to_string();
                }
                let kinds = [
                    "person",
                    "concept",
                    "technology",
                    "organization",
                    "work",
                    "place",
                ];
                let lower = rest.to_ascii_lowercase();
                for k in kinds {
                    if lower.ends_with(k) {
                        let name = rest[..rest.len() - k.len()].trim().to_string();
                        return Verdict {
                            input: items[i].clone(),
                            keep: true,
                            canonical: if name.is_empty() {
                                items[i].clone()
                            } else {
                                name
                            },
                            kind: k.into(),
                        };
                    }
                }
                Verdict {
                    input: items[i].clone(),
                    keep: true,
                    canonical: rest,
                    kind: "concept".into(),
                }
            })
            .collect(),
    )
}

fn adjudicate_batch(
    items: &[String],
    url: &str,
    api_key: &str,
    model: &str,
) -> Option<Vec<Verdict>> {
    let body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": "Output verdict lines only. No preamble."},
            {"role": "user", "content": build_prompt(items)}
        ],
        "max_tokens": 900
    });
    let content = chat_raw(url, api_key, body).ok()?;
    parse_verdicts(&content, items)
}

/// Deterministic name fold. Same slug is one entity. A uuid, a roman numeral,
/// or a token shorter than 4 characters (acronyms of 3+ capitals stay) is refused.
pub fn normalize_name(raw: &str) -> Option<String> {
    let t = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.trim_matches(|c: char| {
        matches!(c, '"' | '\'' | '“' | '”' | ',' | '.' | ':' | ';' | '(' | ')')
    });
    if t.chars().count() < 3 || t.chars().count() > 60 {
        return None;
    }
    if STRUCTURAL.is_match(t) || UUID_NAME.is_match(t) {
        return None;
    }
    let letters: Vec<char> = t.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() < 3 {
        return None;
    }
    let acronym = letters.len() >= 3 && letters.iter().all(|c| c.is_uppercase()) && !t.contains(' ');
    if t.chars().count() < 4 && !acronym {
        return None;
    }
    Some(t.to_string())
}

struct Bucket {
    surfaces: HashMap<String, u32>,
    files: HashMap<String, u32>,
    sightings: HashMap<(String, u32, String), u32>,
    mentions: u32,
    /// (path, chunk) occupied by this entity — used to pair co-occurrences.
    chunks: HashSet<(String, u32)>,
}

/// Extract → normalize. Identical slugs become one entity and the fold is recorded.
/// Different names that share a chunk get RELATES_TO. Nothing here merges on similarity.
pub fn link_corpus(docs: &[DistillDoc]) -> (Vec<Entity>, Vec<EntityEdge>, Value) {
    if docs.is_empty() {
        return (vec![], vec![], json!({ "note": "no documents" }));
    }
    let mut buckets: HashMap<String, Bucket> = HashMap::new();
    let mut candidates = 0u32;
    for d in docs {
        for (raw, n) in extract_candidates(&d.content) {
            candidates += 1;
            let Some(surface) = normalize_name(&raw) else { continue };
            let key = slug(&surface);
            if key.len() < 2 {
                continue;
            }
            let b = buckets.entry(key).or_insert_with(|| Bucket {
                surfaces: HashMap::new(),
                files: HashMap::new(),
                sightings: HashMap::new(),
                mentions: 0,
                chunks: HashSet::new(),
            });
            *b.surfaces.entry(surface.clone()).or_insert(0) += n;
            *b.files.entry(d.rel_path.clone()).or_insert(0) += n;
            *b.sightings
                .entry((d.rel_path.clone(), d.chunk_index, surface))
                .or_insert(0) += n;
            b.mentions += n;
            b.chunks.insert((d.rel_path.clone(), d.chunk_index));
        }
    }
    let mut per_chunk: HashMap<(String, u32), Vec<String>> = HashMap::new();
    let mut ranked: Vec<(String, u32)> = buckets
        .iter()
        .filter(|(_, b)| b.mentions >= MIN_FREQUENCY)
        .map(|(k, b)| (k.clone(), b.mentions))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(MAX_ENTITIES);
    let keep: HashSet<String> = ranked.iter().map(|(k, _)| k.clone()).collect();

    for (key, b) in &buckets {
        if !keep.contains(key) {
            continue;
        }
        let id = format!("entity:{key}");
        for (path, chunk) in &b.chunks {
            per_chunk.entry((path.clone(), *chunk)).or_default().push(id.clone());
        }
    }

    let mut entities: Vec<Entity> = Vec::new();
    let mut edges: Vec<EntityEdge> = Vec::new();
    let mut merges = 0u32;
    for (key, mentions) in &ranked {
        let b = &buckets[key];
        let (name, aliases, merge_notes) = display_and_merges(&b.surfaces, key);
        if !merge_notes.is_empty() {
            merges += 1;
        }
        let mut files: Vec<(String, u32)> = b.files.iter().map(|(p, n)| (p.clone(), *n)).collect();
        files.sort_by(|a, c| c.1.cmp(&a.1).then_with(|| a.0.cmp(&c.0)));
        let mut sightings: Vec<Sighting> = b
            .sightings
            .iter()
            .map(|((path, chunk, surface), count)| Sighting {
                path: path.clone(),
                chunk_index: *chunk,
                surface: surface.clone(),
                count: *count,
            })
            .collect();
        sightings.sort_by(|a, c| c.count.cmp(&a.count).then_with(|| a.path.cmp(&c.path)));
        sightings.truncate(MAX_SIGHTINGS);
        let id = format!("entity:{key}");
        for (path, n) in &files {
            edges.push(EntityEdge {
                from: id.clone(),
                to: path.clone(),
                rel: "mentions".into(),
                weight: *n,
            });
        }
        entities.push(Entity {
            id,
            name,
            kind: "concept".into(),
            aliases,
            files: files.into_iter().map(|(p, _)| p).collect(),
            mentions: *mentions,
            sightings,
            merges: merge_notes,
            ..Entity::default()
        });
    }

    let mut pair: HashMap<(String, String), u32> = HashMap::new();
    for ids in per_chunk.values_mut() {
        ids.sort();
        ids.dedup();
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                let (a, b) = if ids[i] < ids[j] {
                    (ids[i].clone(), ids[j].clone())
                } else {
                    (ids[j].clone(), ids[i].clone())
                };
                *pair.entry((a, b)).or_insert(0) += 1;
            }
        }
    }
    let mut relates = 0u32;
    for ((a, b), n) in pair {
        relates += 1;
        edges.push(EntityEdge {
            from: a,
            to: b,
            rel: "relates_to".into(),
            weight: n,
        });
    }

    let mentions_n = edges.iter().filter(|e| e.rel == "mentions").count() as u32;
    let cross_file = entities.iter().filter(|e| e.files.len() > 1).count() as u32;
    let stats = json!({
        "linked": true,
        "named": false,
        "candidates": candidates,
        "entities": entities.len(),
        "visible": entities.len(),
        "cross_file": cross_file,
        "relates_to": relates,
        "mentions": mentions_n,
        "conflicts": 0,
        "withheld": 0,
        "merges": merges,
    });
    (entities, edges, stats)
}

pub fn display_and_merges(
    surfaces: &HashMap<String, u32>,
    fallback: &str,
) -> (String, Vec<String>, Vec<MergeNote>) {
    let mut ranked: Vec<(String, u32)> = surfaces.iter().map(|(s, n)| (s.clone(), *n)).collect();
    ranked.sort_by(|a, c| c.1.cmp(&a.1).then_with(|| a.0.cmp(&c.0)));
    let name = ranked
        .first()
        .map(|(s, _)| s.clone())
        .unwrap_or_else(|| fallback.to_string());
    let aliases: Vec<String> = ranked.iter().skip(1).map(|(s, _)| s.clone()).collect();
    let notes = if ranked.len() > 1 {
        vec![MergeNote {
            reason: "normalize".into(),
            surfaces: ranked.into_iter().map(|(s, _)| s).collect(),
        }]
    } else {
        vec![]
    };
    (name, aliases, notes)
}

pub fn betweenness_undirected(ids: &[String], pairs: &[(String, String)]) -> HashMap<String, f32> {
    let n = ids.len();
    let mut out: HashMap<String, f32> = ids.iter().map(|id| (id.clone(), 0.0)).collect();
    if n < 3 {
        return out;
    }
    let index: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();
    let mut adj = vec![Vec::<usize>::new(); n];
    for (a, b) in pairs {
        if let (Some(&i), Some(&j)) = (index.get(a.as_str()), index.get(b.as_str())) {
            if i != j {
                adj[i].push(j);
                adj[j].push(i);
            }
        }
    }
    let mut score = vec![0.0f32; n];
    for s in 0..n {
        let mut stack = Vec::new();
        let mut pred: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut sigma = vec![0.0f32; n];
        let mut dist = vec![-1i32; n];
        sigma[s] = 1.0;
        dist[s] = 0;
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(s);
        while let Some(v) = queue.pop_front() {
            stack.push(v);
            for &w in &adj[v] {
                if dist[w] < 0 {
                    dist[w] = dist[v] + 1;
                    queue.push_back(w);
                }
                if dist[w] == dist[v] + 1 {
                    sigma[w] += sigma[v];
                    pred[w].push(v);
                }
            }
        }
        let mut delta = vec![0.0f32; n];
        while let Some(w) = stack.pop() {
            for &v in &pred[w] {
                if sigma[w] > 0.0 {
                    delta[v] += (sigma[v] / sigma[w]) * (1.0 + delta[w]);
                }
            }
            if w != s {
                score[w] += delta[w];
            }
        }
    }
    let norm = ((n - 1) * (n - 2)) as f32;
    for (i, id) in ids.iter().enumerate() {
        out.insert(id.clone(), if norm > 0.0 { score[i] / norm } else { 0.0 });
    }
    out
}

fn apply_betweenness(entities: &mut [Entity], edges: &[EntityEdge]) {
    let ids: Vec<String> = entities.iter().filter(|e| !e.withheld).map(|e| e.id.clone()).collect();
    let pairs: Vec<(String, String)> = edges
        .iter()
        .filter(|e| e.rel == "relates_to")
        .map(|e| (e.from.clone(), e.to.clone()))
        .collect();
    let scores = betweenness_undirected(&ids, &pairs);
    for e in entities.iter_mut() {
        e.betweenness = scores.get(&e.id).copied().unwrap_or(0.0);
        e.degree = edges
            .iter()
            .filter(|edge| edge.from == e.id || edge.to == e.id)
            .filter(|edge| edge.rel != "mentions")
            .count() as u32;
    }
}

fn flag_conflicts(entities: &[Entity], store: &Store) -> Result<Vec<EntityEdge>, String> {
    let mut ranked: Vec<&Entity> = entities.iter().filter(|e| !e.withheld).collect();
    ranked.sort_by(|a, b| b.mentions.cmp(&a.mentions).then_with(|| a.id.cmp(&b.id)));
    ranked.truncate(MAX_ADJUDICATED);
    if ranked.len() < 2 {
        return Ok(vec![]);
    }
    let mut vecs = Vec::with_capacity(ranked.len());
    for e in &ranked {
        match store.embed_one(&e.name, false) {
            Ok(v) => vecs.push(v),
            Err(err) => return Err(err),
        }
    }
    let names: Vec<String> = ranked.iter().map(|e| e.id.clone()).collect();
    let mut edges = Vec::new();
    for (i, j, sim) in near_pairs(&names, &vecs, BLOCK_THRESHOLD) {
        let weight = (sim * 100.0).round().clamp(0.0, 100.0) as u32;
        edges.push(EntityEdge {
            from: ranked[i].id.clone(),
            to: ranked[j].id.clone(),
            rel: "conflicts_with".into(),
            weight,
        });
    }
    Ok(edges)
}

fn adjudicate(entities: &mut [Entity], ep: &crate::settings::Endpoint) {
    let mut order: Vec<usize> = entities.iter().enumerate().map(|(i, _)| i).collect();
    order.sort_by(|&a, &b| {
        entities[b]
            .mentions
            .cmp(&entities[a].mentions)
            .then_with(|| entities[a].id.cmp(&entities[b].id))
    });
    order.truncate(MAX_ADJUDICATED);
    let inputs: Vec<String> = order.iter().map(|&i| entities[i].name.clone()).collect();
    let mut verdicts: HashMap<String, Verdict> = HashMap::new();
    for chunk in inputs.chunks(ADJUDICATION_BATCH) {
        let mut v = adjudicate_batch(chunk, &ep.url, &ep.api_key, &ep.model);
        if v.is_none() {
            v = adjudicate_batch(chunk, &ep.url, &ep.api_key, &ep.model);
        }
        if let Some(batch) = v {
            for item in batch {
                verdicts.insert(item.input.clone(), item);
            }
        }
    }
    for &i in &order {
        let Some(v) = verdicts.get(&entities[i].name) else { continue };
        if !v.keep {
            entities[i].withheld = true;
            if !entities[i].flags.iter().any(|f| f == "endpoint_rejected") {
                entities[i].flags.push("endpoint_rejected".into());
            }
            continue;
        }
        entities[i].withheld = false;
        entities[i].flags.retain(|f| f != "endpoint_rejected");
        if !v.kind.is_empty() {
            entities[i].kind = v.kind.clone();
        }
        let suggested = normalize_name(&v.canonical).unwrap_or_else(|| v.canonical.clone());
        if slug(&suggested) != slug(&entities[i].name) && !suggested.is_empty() {
            let flag = format!("endpoint_disagrees:{suggested}");
            if !entities[i].flags.iter().any(|f| f == &flag) {
                entities[i].flags.push(flag);
            }
        }
    }
}

pub fn distill(
    docs: &[DistillDoc],
    store: &Store,
    endpoint: Option<&crate::settings::Endpoint>,
) -> Result<(Vec<Entity>, Vec<EntityEdge>, Value), String> {
    let (mut entities, mut edges, mut stats) = link_corpus(docs);
    match flag_conflicts(&entities, store) {
        Ok(conflicts) => {
            let n = conflicts.len() as u32;
            edges.extend(conflicts);
            stats["conflicts"] = json!(n);
        }
        Err(err) => {
            stats["conflicts_error"] = json!(err);
        }
    }
    let named = if let Some(ep) = endpoint.filter(|e| !e.url.trim().is_empty()) {
        adjudicate(&mut entities, ep);
        true
    } else {
        false
    };
    apply_betweenness(&mut entities, &edges);
    let withheld = entities.iter().filter(|e| e.withheld).count() as u32;
    let visible = entities.len() as u32 - withheld;
    let cross_file = entities.iter().filter(|e| !e.withheld && e.files.len() > 1).count();
    stats["named"] = json!(named);
    stats["withheld"] = json!(withheld);
    stats["visible"] = json!(visible);
    stats["cross_file"] = json!(cross_file);
    stats["entities"] = json!(entities.len());
    Ok((entities, edges, stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shannon_is_a_candidate() {
        let t = "Claude Shannon defined Information Theory at Bell Labs.";
        let c = extract_candidates(t);
        assert!(
            c.keys().any(|k| k.contains("Shannon") || k.contains("Information")),
            "{c:?}"
        );
    }

    #[test]
    fn roman_numerals_dropped() {
        let c = extract_candidates("Chapter III and PROP MSS appear often.");
        assert!(!c.keys().any(|k| k == "III" || k == "PROP" || k == "MSS"));
    }

    #[test]
    fn slug_stable() {
        assert_eq!(slug("Claude Shannon"), "claude-shannon");
        assert_eq!(slug("  AI  "), "ai");
    }

    #[test]
    fn normalize_drops_uuid_roman_and_tiny_tokens() {
        assert!(normalize_name("550e8400-e29b-41d4-a716-446655440000").is_none());
        assert!(normalize_name("III").is_none());
        assert!(normalize_name("AI").is_none());
        assert_eq!(normalize_name("RAG").as_deref(), Some("RAG"));
        assert_eq!(normalize_name("Bell Labs").as_deref(), Some("Bell Labs"));
    }

    #[test]
    fn folded_spellings_keep_a_merge_note() {
        let mut surfaces = HashMap::new();
        surfaces.insert("Bell Labs".into(), 3);
        surfaces.insert("Bell  Labs".into(), 1);
        let (name, aliases, notes) = display_and_merges(&surfaces, "bell-labs");
        assert_eq!(name, "Bell Labs");
        assert_eq!(aliases, vec!["Bell  Labs".to_string()]);
        assert_eq!(notes[0].reason, "normalize");
        assert_eq!(notes[0].surfaces.len(), 2);
    }

    #[test]
    fn same_name_in_two_files_is_one_entity_with_a_cross_file_edge() {
        let docs = vec![
            DistillDoc {
                rel_path: "a.md".into(),
                chunk_index: 0,
                content: "Claude Shannon defined Information Theory. Claude Shannon published it.".into(),
            },
            DistillDoc {
                rel_path: "b.md".into(),
                chunk_index: 0,
                content: "Information Theory is what Claude Shannon started at Bell Labs.".into(),
            },
        ];
        let (ents, edges, stats) = link_corpus(&docs);
        let shannon = ents.iter().find(|e| e.name.contains("Shannon")).expect("shannon");
        assert!(shannon.files.len() >= 2, "{shannon:?}");
        assert!(!shannon.sightings.is_empty());
        assert!(edges.iter().any(|e| e.rel == "relates_to" && (e.from == shannon.id || e.to == shannon.id)));
        assert!(edges.iter().any(|e| e.rel == "mentions" && e.from == shannon.id && e.weight >= 1));
        assert_eq!(stats["conflicts"], 0);
        assert!(stats["cross_file"].as_u64().unwrap_or(0) >= 1);
        let mut ids: Vec<_> = ents.iter().map(|e| e.id.clone()).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "one id per entity");
    }

    #[test]
    fn near_pair_is_a_flag_not_a_merge() {
        let names = vec!["entity:shannon".into(), "entity:landauer".into()];
        let vecs = vec![vec![1.0, 0.0], vec![0.99, 0.01]];
        let pairs = near_pairs(&names, &vecs, 0.82);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, 0);
        assert_eq!(pairs[0].1, 1);
    }

    #[test]
    fn bridge_outranks_a_leaf() {
        let ids = vec!["a".into(), "b".into(), "c".into()];
        let pairs = vec![("a".into(), "b".into()), ("b".into(), "c".into())];
        let scores = betweenness_undirected(&ids, &pairs);
        assert!(scores["b"] > scores["a"]);
        assert!(scores["b"] > scores["c"]);
    }

    #[test]
    fn tool_call_arguments_parse_and_a_plain_error_is_not_a_tool_refusal() {
        let message = json!({
            "content": null,
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": {"name": "hop", "arguments": "{\"focus\":\"entity:shannon\"}"}
            }]
        });
        let calls = tool_calls_from(&message);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "hop");
        assert_eq!(calls[0].arguments["focus"], "entity:shannon");
        assert!(message_text(&message).is_empty());
        assert!(tools_refused("endpoint HTTP 400: tools are not supported"));
        assert!(!tools_refused("endpoint HTTP 400: model is required"));
    }
}
