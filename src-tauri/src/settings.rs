use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Endpoint {
    pub url: String,
    pub api_key: String,
    pub model: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    #[serde(default)]
    pub welcome_seen: bool,
    #[serde(default)]
    pub endpoint: Option<Endpoint>,
    /// 0 means pick a free port at launch.
    #[serde(default)]
    pub qdrant_port: u16,
    #[serde(default)]
    pub embed_port: u16,
    #[serde(default = "nomic_id")]
    pub embed_model: String,
}

fn nomic_id() -> String {
    "nomic".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            welcome_seen: false,
            endpoint: None,
            qdrant_port: 0,
            embed_port: 0,
            embed_model: nomic_id(),
        }
    }
}

impl Settings {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("settings.json")
    }

    pub fn load(data_dir: &Path) -> Self {
        let p = Self::path(data_dir);
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
        let tmp = data_dir.join("settings.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        std::fs::rename(tmp, Self::path(data_dir)).map_err(|e| e.to_string())
    }

    pub fn has_endpoint(&self) -> bool {
        self.endpoint
            .as_ref()
            .map(|e| !e.url.trim().is_empty())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("crystal-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Settings::default();
        s.welcome_seen = true;
        s.endpoint = Some(Endpoint {
            url: "http://127.0.0.1:11434/v1".into(),
            api_key: String::new(),
            model: "llama".into(),
        });
        s.save(&dir).unwrap();
        let loaded = Settings::load(&dir);
        assert!(loaded.welcome_seen);
        assert_eq!(loaded.endpoint.unwrap().model, "llama");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
