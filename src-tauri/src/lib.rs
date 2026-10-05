mod census;
mod entities;
mod extract;
mod graph;
mod ports;
mod retrieve;
mod rrf;
mod settings;
mod sidecar;
mod store;

use census::{census_dir, is_text_ingest, walk_files};
use entities::{
    chat_with_context, distill, message_text, post_completion, probe_endpoint, tool_calls_from,
    tools_refused, DistillDoc, Entity, EntityEdge,
};
use extract::{chunk_text, ext_of, read_text, MAX_CHUNKS, MAX_CHUNKS_PER_FILE};
use graph::{
    blast, folder_graph, graft_chunks, graft_entities, hop, run_graph_tool, FolderGraph, IngestedChunk,
};
use retrieve::{fuse_retrieve, ChunkRec, RetrieveReport};
use serde::Serialize;
use serde_json::{json, Value};
use settings::{Endpoint, Settings};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use store::{point_uuid, Store};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

struct LabState {
    data_dir: PathBuf,
    _resource_dir: PathBuf,
    daemons: Mutex<Option<sidecar::Daemons>>,
    store: Mutex<Option<Store>>,
    settings: Mutex<Settings>,
    graph: Mutex<Option<FolderGraph>>,
    folder: Mutex<Option<String>>,
    census: Mutex<Option<census::FolderCensus>>,
    entities: Mutex<Vec<Entity>>,
    entity_edges: Mutex<Vec<EntityEdge>>,
    chunks: Mutex<Vec<ChunkRec>>,
    boot_error: Mutex<Option<String>>,
}

#[derive(Serialize)]
struct EndpointResult {
    settings: Settings,
    graph: Option<FolderGraph>,
    entity_stats: Value,
}

#[derive(Serialize)]
struct ScanResult {
    census: census::FolderCensus,
    graph: FolderGraph,
}

#[derive(Serialize, Debug)]
struct IngestReport {
    folder: String,
    files: u32,
    chunks: u32,
    points: u32,
    skipped: Vec<String>,
    collection: String,
    crystal_points: u64,
    graph: FolderGraph,
    named: bool,
    entity_stats: Value,
}

fn clone_store(state: &LabState) -> Result<Store, String> {
    state
        .store
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "local engines are not running".into())
}

fn ports_of(state: &LabState) -> (u16, u16) {
    state
        .daemons
        .lock()
        .ok()
        .and_then(|d| d.as_ref().map(|x| (x.qdrant_port, x.embed_port)))
        .unwrap_or((0, 0))
}

#[tauri::command]
fn lab_status(state: State<LabState>) -> store::LabStatus {
    let (qp, ep) = ports_of(&state);
    let endpoint = state
        .settings
        .lock()
        .ok()
        .map(|s| s.has_endpoint())
        .unwrap_or(false);
    if let Ok(store) = clone_store(&state) {
        let mut st = store.status(qp, ep, endpoint);
        if let Ok(err) = state.boot_error.lock() {
            if st.error.is_none() {
                st.error = err.clone();
            }
        }
        st
    } else {
        store::LabStatus {
            qdrant: false,
            embed: false,
            embed_dims: None,
            embed_model: "nomic-embed-text-v1.5".into(),
            qdrant_port: qp,
            embed_port: ep,
            crystal_points: 0,
            collection: store::COLLECTION.into(),
            endpoint,
            error: state.boot_error.lock().ok().and_then(|e| e.clone()),
        }
    }
}

#[tauri::command]
fn get_settings(state: State<LabState>) -> Result<Settings, String> {
    Ok(state.settings.lock().map_err(|e| e.to_string())?.clone())
}

#[tauri::command]
fn dismiss_welcome(state: State<LabState>) -> Result<Settings, String> {
    let mut s = state.settings.lock().map_err(|e| e.to_string())?;
    s.welcome_seen = true;
    s.save(&state.data_dir)?;
    Ok(s.clone())
}

#[tauri::command]
fn save_endpoint(
    state: State<LabState>,
    url: String,
    api_key: String,
    model: String,
) -> Result<EndpointResult, String> {
    let url = url.trim().to_string();
    let model = model.trim().to_string();
    if url.is_empty() || model.is_empty() {
        return Err("URL and model name are required".into());
    }
    probe_endpoint(&url, &api_key, &model)?;
    let mut s = state.settings.lock().map_err(|e| e.to_string())?;
    s.welcome_seen = true;
    s.endpoint = Some(Endpoint {
        url,
        api_key,
        model,
    });
    s.save(&state.data_dir)?;
    let cloned = s.clone();
    drop(s);
    let (graph, entity_stats) = rebuild_entities(&state)?;
    Ok(EndpointResult {
        settings: cloned,
        graph,
        entity_stats,
    })
}

#[tauri::command]
fn clear_endpoint(state: State<LabState>) -> Result<EndpointResult, String> {
    let mut s = state.settings.lock().map_err(|e| e.to_string())?;
    s.endpoint = None;
    s.welcome_seen = true;
    s.save(&state.data_dir)?;
    let cloned = s.clone();
    drop(s);
    let (graph, entity_stats) = rebuild_entities(&state)?;
    Ok(EndpointResult {
        settings: cloned,
        graph,
        entity_stats,
    })
}

#[tauri::command]
fn save_ports(state: State<LabState>, qdrant_port: u16, embed_port: u16) -> Result<Settings, String> {
    if crate::ports::is_reserved(qdrant_port) || crate::ports::is_reserved(embed_port) {
        return Err("that port is reserved; pick another".into());
    }
    let mut s = state.settings.lock().map_err(|e| e.to_string())?;
    s.qdrant_port = qdrant_port;
    s.embed_port = embed_port;
    s.save(&state.data_dir)?;
    Ok(s.clone())
}

fn rebuild_entities(state: &LabState) -> Result<(Option<FolderGraph>, Value), String> {
    let folder = state.folder.lock().map_err(|e| e.to_string())?.clone();
    let Some(folder) = folder else { return Ok((None, json!({}))) };
    let store = clone_store(state)?;
    let endpoint = state.settings.lock().map_err(|e| e.to_string())?.endpoint.clone();
    let chunks = state.chunks.lock().map_err(|e| e.to_string())?.clone();
    if chunks.is_empty() {
        return Ok((None, json!({})));
    }
    let docs: Vec<DistillDoc> = chunks
        .iter()
        .map(|c| DistillDoc {
            rel_path: c.path.clone(),
            chunk_index: c.chunk_index,
            content: c.content.clone(),
        })
        .collect();
    let (ents, edges, stats) = distill(&docs, &store, endpoint.as_ref())?;
    let mut graph = folder_graph(Path::new(&folder));
    let ingested: Vec<IngestedChunk> = chunks
        .iter()
        .map(|c| IngestedChunk {
            rel_path: c.path.clone(),
            chunk_index: c.chunk_index,
        })
        .collect();
    graft_chunks(&mut graph, &ingested);
    graft_entities(&mut graph, &ents, &edges);
    *state.entities.lock().map_err(|e| e.to_string())? = ents;
    *state.entity_edges.lock().map_err(|e| e.to_string())? = edges;
    *state.graph.lock().map_err(|e| e.to_string())? = Some(graph.clone());
    persist_snapshot(state)?;
    Ok((Some(graph), stats))
}

fn persist_snapshot(state: &LabState) -> Result<(), String> {
    let snap = json!({
        "graph": state.graph.lock().map_err(|e| e.to_string())?.clone(),
        "entities": state.entities.lock().map_err(|e| e.to_string())?.clone(),
        "entity_edges": state.entity_edges.lock().map_err(|e| e.to_string())?.clone(),
        "chunks": state.chunks.lock().map_err(|e| e.to_string())?.clone().into_iter().map(|c| json!({
            "path": c.path, "chunk_index": c.chunk_index, "content": c.content
        })).collect::<Vec<_>>(),
        "folder": state.folder.lock().map_err(|e| e.to_string())?.clone(),
    });
    std::fs::write(
        state.data_dir.join("snapshot.json"),
        serde_json::to_vec(&snap).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> Result<String, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |folder| {
        let _ = tx.send(folder);
    });
    match rx.await {
        Ok(Some(p)) => Ok(p.to_string()),
        Ok(None) => Err("no folder selected".into()),
        Err(_) => Err("dialog failed".into()),
    }
}

#[tauri::command]
async fn scan_folder(state: State<'_, LabState>, path: String) -> Result<ScanResult, String> {
    let store = clone_store(&state).ok();
    let ents = state.entities.lock().map_err(|e| e.to_string())?.clone();
    let eedges = state.entity_edges.lock().map_err(|e| e.to_string())?.clone();
    let path_for_state = path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let p = PathBuf::from(&path);
        if !p.is_dir() {
            return Err("not a directory".to_string());
        }
        let mut g = folder_graph(&p);
        if let Some(store) = store {
            if let Ok(chunks) = store.scroll_folder_chunks(&p.to_string_lossy()) {
                graft_chunks(&mut g, &chunks);
            }
        }
        graft_entities(&mut g, &ents, &eedges);
        Ok(ScanResult {
            census: census_dir(&p),
            graph: g,
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    *state.graph.lock().map_err(|e| e.to_string())? = Some(result.graph.clone());
    *state.folder.lock().map_err(|e| e.to_string())? = Some(path_for_state);
    *state.census.lock().map_err(|e| e.to_string())? = Some(result.census.clone());
    Ok(result)
}

#[tauri::command]
fn hop_cmd(state: State<LabState>, focus: String) -> Result<Vec<String>, String> {
    let g = state.graph.lock().map_err(|e| e.to_string())?;
    let g = g.as_ref().ok_or_else(|| "scan a folder first".to_string())?;
    Ok(hop(g, &focus))
}

#[tauri::command]
fn blast_cmd(state: State<LabState>, focus: String) -> Result<Vec<String>, String> {
    let g = state.graph.lock().map_err(|e| e.to_string())?;
    let g = g.as_ref().ok_or_else(|| "scan a folder first".to_string())?;
    Ok(blast(g, &focus, 2))
}

fn ingest_path(
    root: &Path,
    store: &Store,
    endpoint: Option<&Endpoint>,
    progress: &dyn Fn(&str),
) -> Result<IngestBundle, String> {
    let folder = root.to_string_lossy().into_owned();
    progress("ensuring collection…");
    store.ensure_collection()?;
    progress("probing embed…");
    let probe = store.embed_one("dim probe", false)?;
    if probe.len() != store.dims {
        return Err(format!(
            "embed server returned {} dims, expected {}",
            probe.len(),
            store.dims
        ));
    }
    store.delete_folder_points(&folder)?;

    let (files, _) = walk_files(root);
    progress(&format!("embedding {} files…", files.len()));
    let mut chunks_total = 0u32;
    let mut files_ok = 0u32;
    let mut skipped = Vec::new();
    let mut points = Vec::new();
    let mut ingested: Vec<IngestedChunk> = Vec::new();
    let mut recs: Vec<ChunkRec> = Vec::new();
    let mut per_file: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for path in files {
        let ext = ext_of(&path);
        if !is_text_ingest(&ext) {
            continue;
        }
        match read_text(&path) {
            Ok(text) => {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let chunks = chunk_text(&text);
                if chunks.is_empty() {
                    skipped.push(format!("{rel}: empty"));
                    continue;
                }
                let already = per_file.entry(rel.clone()).or_insert(0);
                files_ok += 1;
                progress(&format!("embedding {rel} ({files_ok})…"));
                for (i, chunk) in chunks.into_iter().enumerate() {
                    if *already >= MAX_CHUNKS_PER_FILE {
                        skipped.push(format!("{rel}: truncated at {MAX_CHUNKS_PER_FILE} chunks"));
                        break;
                    }
                    if points.len() >= MAX_CHUNKS {
                        skipped.push("truncated at 400 chunks".into());
                        break;
                    }
                    let vector = store.embed_one(&chunk, false)?;
                    if vector.len() != store.dims {
                        return Err(format!("embed width {}", vector.len()));
                    }
                    let id = point_uuid(&format!("{folder}:{rel}#{i}"));
                    points.push(json!({
                        "id": id,
                        "vector": vector,
                        "payload": {
                            "folder": folder,
                            "rel_path": rel,
                            "abs_path": path.to_string_lossy(),
                            "chunk_index": i,
                            "content": chunk,
                            "source": "crystal-lab",
                        }
                    }));
                    ingested.push(IngestedChunk {
                        rel_path: rel.clone(),
                        chunk_index: i as u32,
                    });
                    recs.push(ChunkRec {
                        path: rel.clone(),
                        chunk_index: i as u32,
                        content: chunk,
                    });
                    *already += 1;
                    chunks_total += 1;
                }
            }
            Err(e) => skipped.push(format!(
                "{}: {e}",
                path.strip_prefix(root).unwrap_or(&path).display()
            )),
        }
        if points.len() >= MAX_CHUNKS {
            break;
        }
    }

    progress(&format!("upserting {chunks_total} chunks…"));
    let n = points.len() as u32;
    store.upsert_points(points)?;

    let docs: Vec<DistillDoc> = recs
        .iter()
        .map(|c| DistillDoc {
            rel_path: c.path.clone(),
            chunk_index: c.chunk_index,
            content: c.content.clone(),
        })
        .collect();
    progress("linking entities…");
    let (ents, edges, stats) = distill(&docs, store, endpoint)?;
    let named = !ents.is_empty();

    let mut graph = folder_graph(root);
    graft_chunks(&mut graph, &ingested);
    graft_entities(&mut graph, &ents, &edges);
    Ok(IngestBundle {
        report: IngestReport {
            folder,
            files: files_ok,
            chunks: chunks_total,
            points: n,
            skipped,
            collection: store::COLLECTION.into(),
            crystal_points: store.collection_count(store::COLLECTION).unwrap_or(0),
            graph,
            named,
            entity_stats: stats,
        },
        ents,
        edges,
        recs,
    })
}

struct IngestBundle {
    report: IngestReport,
    ents: Vec<Entity>,
    edges: Vec<EntityEdge>,
    recs: Vec<ChunkRec>,
}

#[tauri::command]
async fn ingest_folder(
    app: AppHandle,
    state: State<'_, LabState>,
    path: String,
) -> Result<IngestReport, String> {
    let _ = app.emit("ingest-progress", "waiting for local engines…");
    let store = {
        let mut tries = 0u32;
        loop {
            match clone_store(&state) {
                Ok(s) => break s,
                Err(e) => {
                    if tries >= 90 {
                        return Err(e);
                    }
                    let _ = app.emit("ingest-progress", "waiting for local engines…");
                    let _ = tauri::async_runtime::spawn_blocking(|| {
                        std::thread::sleep(std::time::Duration::from_millis(500))
                    })
                    .await;
                    tries += 1;
                }
            }
        }
    };
    let endpoint = state.settings.lock().map_err(|e| e.to_string())?.endpoint.clone();
    let app2 = app.clone();
    let mem = tauri::async_runtime::spawn_blocking(move || {
        ingest_path(&PathBuf::from(path), &store, endpoint.as_ref(), &|msg| {
            let _ = app2.emit("ingest-progress", msg);
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    *state.graph.lock().map_err(|e| e.to_string())? = Some(mem.report.graph.clone());
    *state.folder.lock().map_err(|e| e.to_string())? = Some(mem.report.folder.clone());
    *state.entities.lock().map_err(|e| e.to_string())? = mem.ents;
    *state.entity_edges.lock().map_err(|e| e.to_string())? = mem.edges;
    *state.chunks.lock().map_err(|e| e.to_string())? = mem.recs;
    persist_snapshot(&state)?;
    Ok(mem.report)
}

#[tauri::command]
async fn retrieve(
    state: State<'_, LabState>,
    query: String,
    folder: Option<String>,
    limit: Option<usize>,
) -> Result<RetrieveReport, String> {
    let store = clone_store(&state)?;
    let chunks = state.chunks.lock().map_err(|e| e.to_string())?.clone();
    let ents = state.entities.lock().map_err(|e| e.to_string())?.clone();
    let edges = state.entity_edges.lock().map_err(|e| e.to_string())?.clone();
    let lim = limit.unwrap_or(10);
    tauri::async_runtime::spawn_blocking(move || {
        fuse_retrieve(&store, &query, folder.as_deref(), &chunks, &ents, &edges, lim)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
struct NeighborView {
    rel: String,
    weight: u32,
    id: String,
    label: String,
}

#[derive(Serialize)]
struct Passage {
    path: String,
    chunk_index: u32,
    excerpt: String,
    surface: String,
}

#[derive(Serialize)]
struct NodeDetail {
    id: String,
    kind: String,
    label: String,
    degree: u32,
    betweenness: f32,
    withheld: bool,
    aliases: Vec<String>,
    files: Vec<String>,
    mentions: u32,
    flags: Vec<String>,
    merges: Vec<entities::MergeNote>,
    neighbors: Vec<NeighborView>,
    passages: Vec<Passage>,
    payload: Value,
}

fn excerpt(text: &str) -> String {
    text.chars().take(280).collect()
}

#[derive(Serialize)]
struct EntityDetail {
    entity: Entity,
    neighbors: Vec<EntityEdge>,
    chunks: Vec<ChunkRec>,
}

#[tauri::command]
fn entity_detail(state: State<LabState>, id: String) -> Result<EntityDetail, String> {
    let ents = state.entities.lock().map_err(|e| e.to_string())?;
    let entity = ents
        .iter()
        .find(|e| e.id == id)
        .cloned()
        .ok_or_else(|| "unknown entity".to_string())?;
    let neighbors: Vec<_> = state
        .entity_edges
        .lock()
        .map_err(|e| e.to_string())?
        .iter()
        .filter(|e| e.from == id || e.to == id)
        .cloned()
        .collect();
    let files: std::collections::HashSet<_> = entity.files.iter().cloned().collect();
    let chunks: Vec<_> = state
        .chunks
        .lock()
        .map_err(|e| e.to_string())?
        .iter()
        .filter(|c| files.contains(&c.path))
        .cloned()
        .collect();
    Ok(EntityDetail {
        entity,
        neighbors,
        chunks,
    })
}

#[tauri::command]
fn node_detail(state: State<LabState>, id: String) -> Result<NodeDetail, String> {
    let ents = state.entities.lock().map_err(|e| e.to_string())?.clone();
    let edges = state.entity_edges.lock().map_err(|e| e.to_string())?.clone();
    let chunks = state.chunks.lock().map_err(|e| e.to_string())?.clone();
    let graph = state.graph.lock().map_err(|e| e.to_string())?.clone();
    let labels = |nid: &str| -> String {
        if let Some(e) = ents.iter().find(|e| e.id == nid) {
            return e.name.clone();
        }
        if let Some(g) = &graph {
            if let Some(n) = g.nodes.iter().find(|n| n.id == nid) {
                return n.label.clone();
            }
        }
        nid.to_string()
    };

    if let Some(entity) = ents.iter().find(|e| e.id == id) {
        let neighbors: Vec<NeighborView> = edges
            .iter()
            .filter(|e| e.from == id || e.to == id)
            .map(|e| {
                let other = if e.from == id { e.to.clone() } else { e.from.clone() };
                NeighborView {
                    rel: e.rel.clone(),
                    weight: e.weight,
                    id: other.clone(),
                    label: labels(&other),
                }
            })
            .collect();
        let mut passages: Vec<Passage> = Vec::new();
        for s in &entity.sightings {
            let body = chunks
                .iter()
                .find(|c| c.path == s.path && c.chunk_index == s.chunk_index)
                .map(|c| excerpt(&c.content))
                .unwrap_or_default();
            passages.push(Passage {
                path: s.path.clone(),
                chunk_index: s.chunk_index,
                excerpt: body,
                surface: s.surface.clone(),
            });
        }
        return Ok(NodeDetail {
            id: id.clone(),
            kind: entity.kind.clone(),
            label: entity.name.clone(),
            degree: entity.degree,
            betweenness: entity.betweenness,
            withheld: entity.withheld,
            aliases: entity.aliases.clone(),
            files: entity.files.clone(),
            mentions: entity.mentions,
            flags: entity.flags.clone(),
            merges: entity.merges.clone(),
            neighbors,
            passages,
            payload: serde_json::to_value(entity).unwrap_or(json!({})),
        });
    }

    let node = graph
        .as_ref()
        .and_then(|g| g.nodes.iter().find(|n| n.id == id));
    let kind = node.map(|n| n.kind.clone()).unwrap_or_else(|| "node".into());
    let label = node.map(|n| n.label.clone()).unwrap_or_else(|| id.clone());
    let mut neighbors = Vec::new();
    if let Some(g) = &graph {
        for e in &g.edges {
            if e.from == id || e.to == id {
                let other = if e.from == id { e.to.clone() } else { e.from.clone() };
                neighbors.push(NeighborView {
                    rel: e.rel.clone(),
                    weight: e.weight,
                    id: other.clone(),
                    label: labels(&other),
                });
            }
        }
    }
    let mut passages = Vec::new();
    if let Some((path, idx)) = id.rsplit_once('#') {
        if let Ok(chunk_index) = idx.parse::<u32>() {
            if let Some(c) = chunks.iter().find(|c| c.path == path && c.chunk_index == chunk_index) {
                passages.push(Passage {
                    path: c.path.clone(),
                    chunk_index: c.chunk_index,
                    excerpt: excerpt(&c.content),
                    surface: String::new(),
                });
            }
        }
    } else {
        for c in chunks.iter().filter(|c| c.path == id).take(6) {
            passages.push(Passage {
                path: c.path.clone(),
                chunk_index: c.chunk_index,
                excerpt: excerpt(&c.content),
                surface: String::new(),
            });
        }
    }
    let degree = neighbors.len() as u32;
    let payload = json!({ "kind": kind, "label": label });
    Ok(NodeDetail {
        id,
        kind,
        label,
        degree,
        betweenness: 0.0,
        withheld: false,
        aliases: vec![],
        files: vec![],
        mentions: 0,
        flags: vec![],
        merges: vec![],
        neighbors,
        passages,
        payload,
    })
}

#[derive(Serialize)]
struct ChatReply {
    answer: String,
    highlight: Vec<String>,
    trace: Vec<String>,
    tools: bool,
}

fn graph_tools() -> Value {
    json!([
        {"type":"function","function":{
            "name":"hop",
            "description":"One step along crystal edges from a node id. Returns neighbor ids and labels.",
            "parameters":{"type":"object","properties":{"focus":{"type":"string","description":"Node id: file path, path#chunk, or entity:slug"}},"required":["focus"]}
        }},
        {"type":"function","function":{
            "name":"blast",
            "description":"Walk the crystal from a node id up to depth steps (1 to 3, default 2).",
            "parameters":{"type":"object","properties":{"focus":{"type":"string"},"depth":{"type":"integer"}},"required":["focus"]}
        }},
        {"type":"function","function":{
            "name":"retrieve",
            "description":"Search the ingested folder. Returns passages.",
            "parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}
        }}
    ])
}

fn ids_of(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|n| n.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn run_chat_tool(
    name: &str,
    args: &Value,
    graph: Option<&FolderGraph>,
    store: &Store,
    folder: Option<&str>,
    chunks: &[ChunkRec],
    ents: &[Entity],
    edges: &[EntityEdge],
) -> (String, Vec<String>) {
    match name {
        "hop" | "blast" => {
            let Some(g) = graph else {
                return ("scan a folder first".into(), vec![]);
            };
            match run_graph_tool(name, args, g) {
                Ok(v) => {
                    let mut ids = ids_of(&v, if name == "hop" { "neighbors" } else { "nodes" });
                    if let Some(focus) = args.get("focus").and_then(|f| f.as_str()) {
                        ids.insert(0, focus.to_string());
                    }
                    (v.to_string(), ids)
                }
                Err(e) => (e, vec![]),
            }
        }
        "retrieve" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            match fuse_retrieve(store, query, folder, chunks, ents, edges, 8) {
                Ok(report) => {
                    let mut ids = Vec::new();
                    let mut lines = Vec::new();
                    for h in &report.hits {
                        ids.push(h.path.clone());
                        ids.push(format!("{}#{}", h.path, h.chunk_index));
                        lines.push(format!(
                            "{}#{} raw {:.3} fused {} [{}]",
                            h.path,
                            h.chunk_index,
                            h.score,
                            h.rrf_score
                                .map(|s| format!("{s:.4}"))
                                .unwrap_or_else(|| "-".into()),
                            h.preview.chars().take(160).collect::<String>()
                        ));
                    }
                    (lines.join("\n"), ids)
                }
                Err(e) => (e, vec![]),
            }
        }
        other => (format!("unknown tool {other}"), vec![]),
    }
}

#[tauri::command]
fn chat_ask(state: State<LabState>, question: String) -> Result<ChatReply, String> {
    let settings = state.settings.lock().map_err(|e| e.to_string())?.clone();
    let Some(ep) = settings.endpoint else {
        return Err("set an endpoint first".into());
    };
    let store = clone_store(&state)?;
    let chunks = state.chunks.lock().map_err(|e| e.to_string())?.clone();
    let ents = state.entities.lock().map_err(|e| e.to_string())?.clone();
    let edges = state.entity_edges.lock().map_err(|e| e.to_string())?.clone();
    let folder = state.folder.lock().map_err(|e| e.to_string())?.clone();
    let graph = state.graph.lock().map_err(|e| e.to_string())?.clone();
    let report = fuse_retrieve(
        &store,
        &question,
        folder.as_deref(),
        &chunks,
        &ents,
        &edges,
        8,
    )?;
    let mut ctx = String::new();
    for h in &report.hits {
        ctx.push_str(&format!("[{}#{}] {}\n", h.path, h.chunk_index, h.preview));
    }
    let mut index = String::new();
    if let Some(g) = &graph {
        for n in g.nodes.iter().filter(|n| n.kind != "chunk").take(60) {
            index.push_str(&format!("{} [{}]\n", n.id, n.label));
        }
    }
    let mut messages = vec![
        json!({
            "role": "system",
            "content": "Answer from this crystal. You may call hop, blast, and retrieve. Use only node ids from the list. Do not invent ids."
        }),
        json!({
            "role": "user",
            "content": format!("Nodes:\n{index}\nExcerpts:\n{ctx}\n\nQuestion: {question}")
        }),
    ];
    let mut highlight = Vec::new();
    let mut trace = Vec::new();
    for round in 0..4 {
        let body = json!({
            "model": ep.model,
            "messages": messages,
            "tools": graph_tools(),
            "tool_choice": "auto",
            "max_tokens": 700
        });
        let posted = post_completion(&ep.url, &ep.api_key, body);
        let completion = match posted {
            Err(err) if round == 0 && tools_refused(&err) => {
                trace.push("endpoint has no tools — answered from excerpts".into());
                let answer = chat_with_context(&ep.url, &ep.api_key, &ep.model, &question, &ctx)?;
                return Ok(ChatReply {
                    answer,
                    highlight,
                    trace,
                    tools: false,
                });
            }
            Err(err) => return Err(err),
            Ok(j) => j,
        };
        let message = completion["choices"][0]["message"].clone();
        let calls = tool_calls_from(&message);
        if calls.is_empty() {
            let answer = message_text(&message);
            if answer.is_empty() {
                return Err("endpoint returned an empty answer".into());
            }
            highlight.sort();
            highlight.dedup();
            return Ok(ChatReply {
                answer,
                highlight,
                trace,
                tools: true,
            });
        }
        messages.push(message);
        for call in calls {
            let (text, ids) = run_chat_tool(
                &call.name,
                &call.arguments,
                graph.as_ref(),
                &store,
                folder.as_deref(),
                &chunks,
                &ents,
                &edges,
            );
            trace.push(format!("{} {}", call.name, text.chars().take(160).collect::<String>()));
            highlight.extend(ids);
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": text,
            }));
        }
    }
    Err("endpoint kept calling tools without an answer".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("crystal-lab"));
            let _ = std::fs::create_dir_all(&data_dir);
            let resource_hint = app.path().resource_dir().ok();
            let resource_dir = sidecar::resolve_resources(resource_hint.as_deref());
            let settings = Settings::load(&data_dir);
            let boot = format!(
                "resource_dir={} exists_nomic={}\n",
                resource_dir.display(),
                resource_dir
                    .join("models")
                    .join("nomic-embed-text-v1.5.Q4_K_M.gguf")
                    .exists()
            );
            let _ = std::fs::write(data_dir.join("boot.log"), boot);
            app.manage(LabState {
                data_dir: data_dir.clone(),
                _resource_dir: resource_dir.clone(),
                daemons: Mutex::new(None),
                store: Mutex::new(None),
                settings: Mutex::new(settings.clone()),
                graph: Mutex::new(None),
                folder: Mutex::new(None),
                census: Mutex::new(None),
                entities: Mutex::new(Vec::new()),
                entity_edges: Mutex::new(Vec::new()),
                chunks: Mutex::new(Vec::new()),
                boot_error: Mutex::new(Some("starting local engines…".into())),
            });
            let handle = app.handle().clone();
            let q_pref = settings.qdrant_port;
            let e_pref = settings.embed_port;
            tauri::async_runtime::spawn(async move {
                let data = data_dir;
                let resources = resource_dir;
                let started = tauri::async_runtime::spawn_blocking(move || {
                    sidecar::start_daemons(&data, &resources, Some(q_pref), Some(e_pref))
                })
                .await;
                match started {
                    Ok(Ok(d)) => {
                        let store = Store::nomic(d.qdrant_url.clone(), d.embed_url.clone());
                        if let Some(state) = handle.try_state::<LabState>() {
                            if let Ok(mut slot) = state.store.lock() {
                                *slot = Some(store);
                            }
                            if let Ok(mut slot) = state.daemons.lock() {
                                *slot = Some(d);
                            }
                            if let Ok(mut slot) = state.boot_error.lock() {
                                *slot = None;
                            }
                        }
                        let _ = handle.emit("engines", "ready");
                    }
                    Ok(Err(e)) => {
                        if let Some(state) = handle.try_state::<LabState>() {
                            if let Ok(mut slot) = state.boot_error.lock() {
                                *slot = Some(e.clone());
                            }
                        }
                        let _ = handle.emit("engines", e);
                    }
                    Err(e) => {
                        let msg = format!("engine start cancelled: {e}");
                        if let Some(state) = handle.try_state::<LabState>() {
                            if let Ok(mut slot) = state.boot_error.lock() {
                                *slot = Some(msg.clone());
                            }
                        }
                        let _ = handle.emit("engines", msg);
                    }
                }
            });
            #[cfg(target_os = "macos")]
            {
                let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
            }
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            lab_status,
            get_settings,
            dismiss_welcome,
            save_endpoint,
            clear_endpoint,
            save_ports,
            pick_folder,
            scan_folder,
            hop_cmd,
            blast_cmd,
            ingest_folder,
            retrieve,
            entity_detail,
            node_detail,
            chat_ask
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn census_counts_md_pdf_image_code() {
        let dir = std::env::temp_dir().join(format!("crystal-lab-census-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("note.md"), "hi").unwrap();
        fs::write(dir.join("paper.pdf"), "%PDF").unwrap();
        fs::write(dir.join("fig.png"), "x").unwrap();
        fs::write(dir.join("main.ts"), "export {}\n").unwrap();
        fs::write(dir.join("skip.bin"), "x").unwrap();
        fs::create_dir_all(dir.join("node_modules")).unwrap();
        fs::write(dir.join("node_modules/dep.md"), "no").unwrap();

        let c = census_dir(&dir);
        assert_eq!(c.markdown, 1);
        assert_eq!(c.pdf, 1);
        assert_eq!(c.image, 1);
        assert_eq!(c.code, 1);
        assert_eq!(c.skipped, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_sidecar_ingest_retrieve() {
        let data = std::env::temp_dir().join(format!("crystal-lab-live-data-{}", std::process::id()));
        let _ = fs::remove_dir_all(&data);
        fs::create_dir_all(&data).unwrap();
        let resources = sidecar::crate_resources();
        let daemons = match sidecar::start_daemons(&data, &resources, None, None) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("skip live sidecar: {e}");
                return;
            }
        };
        let store = Store::nomic(daemons.qdrant_url.clone(), daemons.embed_url.clone());
        let dir = std::env::temp_dir().join(format!("crystal-lab-live-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("alpha.md"),
            "the crystal absorbs the folder and hops along the mycelium",
        )
        .unwrap();
        fs::write(dir.join("beta.md"), "unrelated potatoes and weather reports").unwrap();
        let long: String = (0..700).map(|i| format!("token{i} ")).collect();
        fs::write(dir.join("long.md"), &long).unwrap();
        store
            .embed_one(&long, false)
            .expect("chunk-sized embed must fit the llama batch");
        let mem = ingest_path(&dir, &store, None, &|_| {}).expect("ingest");
        assert!(mem.report.points >= 3, "{:?}", mem.report);
        assert!(mem.report.graph.nodes.iter().any(|n| n.kind == "chunk"));
        let hits = store
            .search_vector("crystal folder hops", Some(&mem.report.folder), 4)
            .expect("search");
        assert!(
            hits.iter().any(|h| h.path.contains("alpha")),
            "{hits:?}"
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&data);
        drop(daemons);
    }
}
