use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub const COLLECTION: &str = "crystal_lab";
pub const NOMIC_DIMS: usize = 768;
#[allow(dead_code)]
pub const BGE_DIMS: usize = 1024;

#[derive(Clone, Debug)]
pub struct Store {
    pub qdrant: String,
    pub embed: String,
    pub dims: usize,
    pub model: String,
}

impl Store {
    pub fn nomic(qdrant: String, embed: String) -> Self {
        Self {
            qdrant,
            embed,
            dims: NOMIC_DIMS,
            model: "nomic-embed-text-v1.5".into(),
        }
    }

    pub fn is_nomic(&self) -> bool {
        self.model.contains("nomic")
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct LabStatus {
    pub qdrant: bool,
    pub embed: bool,
    pub embed_dims: Option<usize>,
    pub embed_model: String,
    pub qdrant_port: u16,
    pub embed_port: u16,
    pub crystal_points: u64,
    pub collection: String,
    pub endpoint: bool,
    pub error: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct SearchHit {
    pub score: f32,
    pub rrf_score: Option<f32>,
    pub lists: Vec<String>,
    pub path: String,
    pub chunk_index: u32,
    pub preview: String,
    pub payload: Value,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(2))
        .timeout_connect(Duration::from_secs(1))
        .build()
}

fn agent_long() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(60))
        .build()
}

fn get_json(url: &str) -> Result<Value, String> {
    let resp = agent().get(url).call().map_err(|e| e.to_string())?;
    resp.into_json().map_err(|e| e.to_string())
}

fn send_json(method: &str, url: &str, body: Value) -> Result<Value, String> {
    let req = match method {
        "PUT" => agent_long().put(url),
        _ => agent_long().post(url),
    };
    let resp = match req.send_json(body) {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            return Err(format!(
                "HTTP {code}: {}",
                text.chars().take(240).collect::<String>()
            ));
        }
        Err(e) => return Err(e.to_string()),
    };
    let status = resp.status();
    let text = resp.into_string().map_err(|e| e.to_string())?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "HTTP {status}: {}",
            text.chars().take(240).collect::<String>()
        ));
    }
    if text.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

impl Store {
    pub fn status(&self, q_port: u16, e_port: u16, endpoint: bool) -> LabStatus {
        let qdrant = get_json(&format!("{}/collections", self.qdrant)).is_ok();
        let embed = agent()
            .get(&format!("{}/health", self.embed))
            .call()
            .map(|r| r.status() == 200)
            .unwrap_or(false);
        let crystal_points = self.collection_count(COLLECTION).unwrap_or(0);
        let error = if !qdrant {
            Some("local qdrant is not answering".into())
        } else if !embed {
            Some("local embed server is not answering".into())
        } else {
            None
        };
        LabStatus {
            qdrant,
            embed,
            embed_dims: if embed { Some(self.dims) } else { None },
            embed_model: self.model.clone(),
            qdrant_port: q_port,
            embed_port: e_port,
            crystal_points,
            collection: COLLECTION.into(),
            endpoint,
            error,
        }
    }

    pub fn collection_count(&self, name: &str) -> Result<u64, String> {
        let j = get_json(&format!("{}/collections/{name}", self.qdrant))?;
        Ok(j["result"]["points_count"].as_u64().unwrap_or(0))
    }

    pub fn ensure_collection(&self) -> Result<(), String> {
        let url = format!("{}/collections/{COLLECTION}", self.qdrant);
        if get_json(&url).is_err() {
            send_json(
                "PUT",
                &url,
                json!({ "vectors": { "size": self.dims, "distance": "Cosine" } }),
            )?;
        }
        let _ = send_json(
            "PUT",
            &format!("{url}/index"),
            json!({ "field_name": "folder", "field_schema": "keyword" }),
        );
        Ok(())
    }

    fn prefix(&self, text: &str, query: bool) -> String {
        if self.is_nomic() {
            if query {
                format!("search_query: {text}")
            } else {
                format!("search_document: {text}")
            }
        } else {
            text.to_string()
        }
    }

    pub fn embed_one(&self, text: &str, query: bool) -> Result<Vec<f32>, String> {
        let sliced: String = text.chars().take(crate::extract::EMBED_CHARS).collect();
        let input = self.prefix(&sliced, query);
        let j = send_json(
            "POST",
            &format!("{}/v1/embeddings", self.embed),
            json!({ "model": self.model, "input": input }),
        )?;
        let arr = j["data"][0]["embedding"]
            .as_array()
            .ok_or_else(|| "embed: no vector".to_string())?;
        Ok(arr.iter().filter_map(|x| x.as_f64().map(|n| n as f32)).collect())
    }

    pub fn scroll_folder_chunks(&self, folder: &str) -> Result<Vec<crate::graph::IngestedChunk>, String> {
        let j = send_json(
            "POST",
            &format!("{}/collections/{COLLECTION}/points/scroll", self.qdrant),
            json!({
                "filter": { "must": [{ "key": "folder", "match": { "value": folder } }] },
                "limit": 400,
                "with_payload": true,
                "with_vector": false
            }),
        )?;
        let mut out = Vec::new();
        if let Some(pts) = j["result"]["points"].as_array() {
            for p in pts {
                let rel = p["payload"]["rel_path"].as_str().unwrap_or("").to_string();
                let i = p["payload"]["chunk_index"].as_u64().unwrap_or(0) as u32;
                if !rel.is_empty() {
                    out.push(crate::graph::IngestedChunk {
                        rel_path: rel,
                        chunk_index: i,
                    });
                }
            }
        }
        Ok(out)
    }

    pub fn delete_folder_points(&self, folder: &str) -> Result<(), String> {
        let _ = send_json(
            "POST",
            &format!("{}/collections/{COLLECTION}/points/delete?wait=true", self.qdrant),
            json!({ "filter": { "must": [{ "key": "folder", "match": { "value": folder } }] } }),
        );
        Ok(())
    }

    pub fn upsert_points(&self, points: Vec<Value>) -> Result<(), String> {
        if points.is_empty() {
            return Ok(());
        }
        send_json(
            "PUT",
            &format!("{}/collections/{COLLECTION}/points?wait=true", self.qdrant),
            json!({ "points": points }),
        )?;
        Ok(())
    }

    pub fn search_vector(
        &self,
        query: &str,
        folder: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchHit>, String> {
        let vector = self.embed_one(query, true)?;
        if vector.len() != self.dims {
            return Err(format!(
                "embed returned {} dims, expected {}",
                vector.len(),
                self.dims
            ));
        }
        let mut body = json!({
            "vector": vector,
            "limit": limit,
            "with_payload": true,
        });
        if let Some(folder) = folder {
            body["filter"] = json!({ "must": [{ "key": "folder", "match": { "value": folder } }] });
        }
        let j = send_json(
            "POST",
            &format!("{}/collections/{COLLECTION}/points/search", self.qdrant),
            body,
        )?;
        let mut hits = Vec::new();
        if let Some(arr) = j["result"].as_array() {
            for h in arr {
                let payload = h["payload"].clone();
                let preview = payload["content"].as_str().unwrap_or("");
                let preview: String = preview.chars().take(220).collect();
                hits.push(SearchHit {
                    score: h["score"].as_f64().unwrap_or(0.0) as f32,
                    rrf_score: None,
                    lists: vec!["vector".into()],
                    path: payload["rel_path"].as_str().unwrap_or("").to_string(),
                    chunk_index: payload["chunk_index"].as_u64().unwrap_or(0) as u32,
                    preview,
                    payload,
                });
            }
        }
        Ok(hits)
    }
}

pub fn point_uuid(key: &str) -> String {
    let d = Sha256::digest(key.as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_is_hyphenated() {
        let u = point_uuid("note.md#0");
        assert_eq!(u.len(), 36);
        assert_eq!(u.chars().filter(|c| *c == '-').count(), 4);
    }
}
