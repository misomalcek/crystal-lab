use crate::ports::{free_port, is_reserved};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub fn qdrant_sidecar_name() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "qdrant-aarch64-apple-darwin"
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        "qdrant"
    }
}

#[cfg(debug_assertions)]
pub fn crate_resources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources")
}

#[cfg(not(debug_assertions))]
pub fn crate_resources() -> PathBuf {
    PathBuf::new()
}

#[cfg(debug_assertions)]
pub fn qdrant_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(qdrant_sidecar_name())
}

#[cfg(not(debug_assertions))]
pub fn qdrant_bin() -> PathBuf {
    PathBuf::new()
}

pub fn resolve_resources(hint: Option<&Path>) -> PathBuf {
    if let Some(p) = hint {
        if p.join("models")
            .join("nomic-embed-text-v1.5.Q4_K_M.gguf")
            .exists()
        {
            return p.to_path_buf();
        }
        let nested = p.join("resources");
        if nested
            .join("models")
            .join("nomic-embed-text-v1.5.Q4_K_M.gguf")
            .exists()
        {
            return nested;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos) = exe.parent() {
            let res = macos.join("../Resources");
            if res
                .join("models")
                .join("nomic-embed-text-v1.5.Q4_K_M.gguf")
                .exists()
            {
                return res;
            }
        }
    }
    crate_resources()
}

pub struct Daemons {
    pub qdrant_port: u16,
    pub embed_port: u16,
    pub qdrant_url: String,
    pub embed_url: String,
    qdrant: Child,
    embed: Child,
}

impl Drop for Daemons {
    fn drop(&mut self) {
        let _ = self.qdrant.kill();
        let _ = self.embed.kill();
        let _ = self.qdrant.wait();
        let _ = self.embed.wait();
    }
}

fn wait_http(url: &str, timeout: Duration) -> Result<(), String> {
    let t0 = Instant::now();
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(400))
        .timeout_connect(Duration::from_millis(400))
        .build();
    while t0.elapsed() < timeout {
        if agent.get(url).call().map(|r| r.status() < 500).unwrap_or(false) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(150));
    }
    Err(format!("timeout waiting for {url}"))
}

fn pick(preferred: Option<u16>) -> Result<u16, String> {
    match preferred {
        Some(p) if p != 0 && !is_reserved(p) => Ok(p),
        _ => free_port(),
    }
}

pub fn start_daemons(
    data_dir: &Path,
    resource_dir: &Path,
    preferred_qdrant: Option<u16>,
    preferred_embed: Option<u16>,
) -> Result<Daemons, String> {
    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let storage = data_dir.join("qdrant-storage");
    let snapshots = data_dir.join("qdrant-snapshots");
    std::fs::create_dir_all(&storage).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&snapshots).map_err(|e| e.to_string())?;

    let qport = pick(preferred_qdrant)?;
    let mut eport = pick(preferred_embed)?;
    if eport == qport {
        eport = free_port()?;
    }
    let grpc = free_port()?;

    let qbin = {
        let bundled = resource_dir.join("embed-runtime").join("qdrant");
        if bundled.exists() {
            bundled
        } else {
            qdrant_bin()
        }
    };
    if !qbin.exists() {
        return Err("qdrant binary is missing from the app bundle".into());
    }
    let qlog = File::create(data_dir.join("qdrant.log")).map_err(|e| e.to_string())?;
    let qerr = qlog.try_clone().map_err(|e| e.to_string())?;
    let mut qchild = Command::new(&qbin)
        .current_dir(data_dir)
        .env("QDRANT__SERVICE__HTTP_PORT", qport.to_string())
        .env("QDRANT__SERVICE__GRPC_PORT", grpc.to_string())
        .env("QDRANT__SERVICE__HOST", "127.0.0.1")
        .env("QDRANT__STORAGE__STORAGE_PATH", &storage)
        .env("QDRANT__STORAGE__SNAPSHOTS_PATH", &snapshots)
        .env("QDRANT__TELEMETRY_DISABLED", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::from(qlog))
        .stderr(Stdio::from(qerr))
        .spawn()
        .map_err(|e| format!("could not start qdrant: {e}"))?;

    let embed_dir = resource_dir.join("embed-runtime");
    let llama = embed_dir.join("llama-server");
    let model = resource_dir
        .join("models")
        .join("nomic-embed-text-v1.5.Q4_K_M.gguf");
    if !llama.exists() {
        return Err("embed runtime is missing from the app bundle".into());
    }
    if !model.exists() {
        return Err("nomic model is missing from the app bundle".into());
    }
    let elog = File::create(data_dir.join("embed.log")).map_err(|e| e.to_string())?;
    let eerr = elog.try_clone().map_err(|e| e.to_string())?;
    let mut echild = Command::new(&llama)
        .current_dir(&embed_dir)
        .env("DYLD_LIBRARY_PATH", &embed_dir)
        .env("DYLD_FALLBACK_LIBRARY_PATH", &embed_dir)
        .arg("-m")
        .arg(&model)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(eport.to_string())
        .arg("--embedding")
        .arg("--pooling")
        .arg("mean")
        .arg("-c")
        .arg("2048")
        .arg("-b")
        .arg("2048")
        .arg("-ub")
        .arg("2048")
        .arg("--no-webui")
        .stdin(Stdio::null())
        .stdout(Stdio::from(elog))
        .stderr(Stdio::from(eerr))
        .spawn()
        .map_err(|e| format!("could not start embed server: {e}"))?;

    let qurl = format!("http://127.0.0.1:{qport}");
    let eurl = format!("http://127.0.0.1:{eport}");
    if let Err(e) = wait_http(&format!("{qurl}/collections"), Duration::from_secs(15)) {
        let died = qchild.try_wait().ok().flatten();
        let _ = qchild.kill();
        let _ = echild.kill();
        return Err(match died {
            Some(st) => format!("{e}; qdrant exited {st}. see qdrant.log"),
            None => e,
        });
    }
    if let Err(e) = wait_http(&format!("{eurl}/health"), Duration::from_secs(45)) {
        let died = echild.try_wait().ok().flatten();
        let _ = qchild.kill();
        let _ = echild.kill();
        return Err(match died {
            Some(st) => format!("{e}; embed exited {st}. see embed.log"),
            None => e,
        });
    }

    Ok(Daemons {
        qdrant_port: qport,
        embed_port: eport,
        qdrant_url: qurl,
        embed_url: eurl,
        qdrant: qchild,
        embed: echild,
    })
}
