//! Client for upscaler-bridge (debug logging, temporary cloud storage,
//! device settings backup). Blocking calls — Swift runs them off the main
//! thread; logging calls queue and send in the background.

use crate::PbError;
use serde_json::Value;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

#[derive(uniffi::Record, Clone)]
pub struct StoredImage {
    pub id: String,
    pub filename: String,
    pub size_bytes: u64,
    pub created_at: String,
    pub expires_at: String,
    pub label: String,
}

/// A server-side upscale job.
#[derive(uniffi::Record, Clone)]
pub struct CloudJob {
    pub id: String,
    /// queued | running | done | failed | cancelled
    pub status: String,
    pub progress: f32,
    /// 1 = next up (0 when not queued).
    pub queue_position: u32,
    pub label: String,
    pub filename: String,
    pub created_at: f64,
    pub expires_at: f64,
    pub in_width: u32,
    pub in_height: u32,
    pub out_width: u32,
    pub out_height: u32,
    pub scale: f32,
    pub model: String,
    pub seconds: f64,
    pub fidelity_db: f64,
    pub result_bytes: u64,
    pub capped: bool,
    pub error: String,
}

/// Download progress (bytes so far, total).
#[uniffi::export(with_foreign)]
pub trait DownloadListener: Send + Sync {
    fn on_progress(&self, received: u64, total: u64);
}

fn job_from(v: &Value) -> CloudJob {
    let st = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
    let f = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
    CloudJob {
        id: st("id"),
        status: st("status"),
        progress: f("progress") as f32,
        queue_position: f("queue_position") as u32,
        label: st("label"),
        filename: st("filename"),
        created_at: f("created_at"),
        expires_at: f("expires_at"),
        in_width: f("in_w") as u32,
        in_height: f("in_h") as u32,
        out_width: f("out_w") as u32,
        out_height: f("out_h") as u32,
        scale: v.get("settings").and_then(|s| s.get("scale")).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
        model: st("model_used"),
        seconds: f("seconds"),
        fidelity_db: f("fidelity_db"),
        result_bytes: f("result_bytes") as u64,
        capped: v.get("capped").and_then(|x| x.as_bool()).unwrap_or(false),
        error: st("error"),
    }
}

#[derive(Clone)]
struct Config {
    base: String,
    key: String,
    device: String,
}

#[derive(uniffi::Object)]
pub struct ServerClient {
    cfg: Arc<Mutex<Config>>,
    agent: ureq::Agent,
    queue: Mutex<mpsc::Sender<(String, Value)>>,
}

fn err(e: impl std::fmt::Display) -> PbError {
    PbError::Failed { message: e.to_string() }
}

fn auth(req: ureq::Request, key: &str) -> ureq::Request {
    if key.is_empty() {
        req
    } else {
        req.set("Authorization", &format!("Bearer {key}"))
    }
}

fn read_err(e: ureq::Error) -> PbError {
    match e {
        ureq::Error::Status(code, resp) => {
            let detail = resp.into_json::<Value>().ok().and_then(|v| v.get("detail").and_then(|d| d.as_str()).map(String::from));
            err(match detail {
                Some(d) => format!("server returned {code}: {d}"),
                None => format!("server returned {code}"),
            })
        }
        other => err(other),
    }
}

#[uniffi::export]
impl ServerClient {
    #[uniffi::constructor]
    pub fn new(base_url: String, api_key: String, device_id: String) -> Arc<Self> {
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(8)).timeout(Duration::from_secs(120)).build();
        let cfg = Arc::new(Mutex::new(Config { base: base_url.trim_end_matches('/').to_string(), key: api_key, device: device_id }));
        let (tx, rx) = mpsc::channel::<(String, Value)>();
        let (a2, c2) = (agent.clone(), cfg.clone());
        std::thread::spawn(move || {
            // Background sender for fire-and-forget logging.
            while let Ok((path, body)) = rx.recv() {
                let c = c2.lock().unwrap().clone();
                if c.base.is_empty() {
                    continue;
                }
                let _ = auth(a2.post(&format!("{}{path}", c.base)), &c.key).send_json(body);
            }
        });
        Arc::new(ServerClient { cfg, agent, queue: Mutex::new(tx) })
    }

    pub fn configure(&self, base_url: String, api_key: String) {
        let mut c = self.cfg.lock().unwrap();
        c.base = base_url.trim_end_matches('/').to_string();
        c.key = api_key;
    }

    pub fn health(&self) -> Result<String, PbError> {
        let c = self.cfg.lock().unwrap().clone();
        let v: Value = self.agent.get(&format!("{}/health", c.base)).call().map_err(read_err)?.into_json().map_err(err)?;
        Ok(v.get("version").and_then(|v| v.as_str()).unwrap_or("?").to_string())
    }

    /// Queue an upscale log entry (JSON object; device_id is filled in).
    pub fn log_upscale(&self, entry_json: String) {
        self.enqueue("/log/upscale", entry_json);
    }

    /// Queue an action log entry: {"action": ..., "detail": ...}.
    pub fn log_action(&self, action: String, detail_json: String) {
        let body = serde_json::json!({"action": action, "detail": detail_json});
        self.enqueue("/log/action", body.to_string());
    }

    /// Upload a finished result to temporary storage; returns its id.
    pub fn upload_export(&self, path: String, label: String, ttl_hours: u32) -> Result<String, PbError> {
        let c = self.cfg.lock().unwrap().clone();
        let bytes = std::fs::read(&path).map_err(err)?;
        let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "image.png".into());
        let ctype = if name.ends_with(".jpg") || name.ends_with(".jpeg") { "image/jpeg" } else { "image/png" };
        let boundary = format!("pb{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let mut body = Vec::with_capacity(bytes.len() + 1024);
        for (k, v) in [("device_id", c.device.clone()), ("ttl_hours", ttl_hours.max(1).to_string()), ("label", label)] {
            body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
        }
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {ctype}\r\n\r\n").as_bytes());
        body.extend_from_slice(&bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let v: Value = auth(self.agent.post(&format!("{}/export", c.base)), &c.key)
            .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
            .send_bytes(&body)
            .map_err(read_err)?
            .into_json()
            .map_err(err)?;
        Ok(v.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string())
    }

    /// Send a photo for server-side upscaling. `settings_json` uses the
    /// worker's keys: model, scale, fidelity, detail, denoise, ensemble,
    /// format ("png" | "jpg").
    pub fn submit_job(&self, path: String, settings_json: String, label: String) -> Result<CloudJob, PbError> {
        let c = self.cfg.lock().unwrap().clone();
        let bytes = std::fs::read(&path).map_err(err)?;
        let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "photo.png".into());
        let boundary = format!("pb{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let mut body = Vec::with_capacity(bytes.len() + 2048);
        for (k, v) in [("device_id", c.device.clone()), ("settings", settings_json), ("label", label)] {
            body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
        }
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        body.extend_from_slice(&bytes);
        drop(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let v: Value = auth(self.agent.post(&format!("{}/jobs", c.base)), &c.key)
            .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
            .send_bytes(&body)
            .map_err(read_err)?
            .into_json()
            .map_err(err)?;
        Ok(job_from(&v))
    }

    pub fn list_jobs(&self) -> Result<Vec<CloudJob>, PbError> {
        let c = self.cfg.lock().unwrap().clone();
        let v: Value = auth(self.agent.get(&format!("{}/jobs", c.base)).query("device_id", &c.device), &c.key).call().map_err(read_err)?.into_json().map_err(err)?;
        Ok(v.get("entries").and_then(|e| e.as_array()).map(|a| a.iter().map(job_from).collect()).unwrap_or_default())
    }

    pub fn get_job(&self, id: String) -> Result<CloudJob, PbError> {
        let c = self.cfg.lock().unwrap().clone();
        let v: Value = auth(self.agent.get(&format!("{}/jobs/{id}", c.base)), &c.key).call().map_err(read_err)?.into_json().map_err(err)?;
        Ok(job_from(&v))
    }

    /// Cancel (if still running) and delete a job and its result.
    pub fn delete_job(&self, id: String) -> Result<(), PbError> {
        let c = self.cfg.lock().unwrap().clone();
        auth(self.agent.delete(&format!("{}/jobs/{id}", c.base)), &c.key).call().map_err(read_err)?;
        Ok(())
    }

    pub fn download_job_thumb(&self, id: String, dest_path: String) -> Result<(), PbError> {
        let c = self.cfg.lock().unwrap().clone();
        let resp = auth(self.agent.get(&format!("{}/jobs/{id}/thumb", c.base)), &c.key).call().map_err(read_err)?;
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut resp.into_reader(), &mut buf).map_err(err)?;
        std::fs::write(dest_path, buf).map_err(err)
    }

    /// Download the full-resolution result to `dest_path`, resuming a
    /// partial `dest_path.part` from an earlier attempt.
    pub fn download_job(&self, id: String, dest_path: String, listener: Arc<dyn DownloadListener>) -> Result<(), PbError> {
        use std::io::{Read, Write};
        let c = self.cfg.lock().unwrap().clone();
        let part = format!("{dest_path}.part");
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let mut req = auth(self.agent.get(&format!("{}/jobs/{id}/result", c.base)), &c.key);
        if have > 0 {
            req = req.set("Range", &format!("bytes={have}-"));
        }
        let resp = req.call().map_err(read_err)?;
        let resumed = resp.status() == 206;
        let len: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
        let total = if resumed { have + len } else { len };
        let mut file = std::fs::OpenOptions::new().create(true).write(true).append(resumed).truncate(!resumed).open(&part).map_err(err)?;
        let mut got = if resumed { have } else { 0 };
        let mut reader = resp.into_reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut last = std::time::Instant::now();
        loop {
            let n = reader.read(&mut buf).map_err(err)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(err)?;
            got += n as u64;
            if last.elapsed().as_millis() > 150 {
                last = std::time::Instant::now();
                listener.on_progress(got, total);
            }
        }
        file.flush().map_err(err)?;
        if total > 0 && got < total {
            return Err(err(format!("download interrupted at {got} of {total} bytes — try again to resume")));
        }
        listener.on_progress(got, total.max(got));
        std::fs::rename(&part, &dest_path).map_err(err)
    }

    pub fn list_exports(&self) -> Result<Vec<StoredImage>, PbError> {
        let c = self.cfg.lock().unwrap().clone();
        let v: Value = auth(self.agent.get(&format!("{}/export", c.base)).query("device_id", &c.device), &c.key).call().map_err(read_err)?.into_json().map_err(err)?;
        let rows = v.get("entries").and_then(|e| e.as_array()).cloned().unwrap_or_default();
        let s = |r: &Value, k: &str| r.get(k).map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).unwrap_or_default();
        Ok(rows
            .iter()
            .map(|r| StoredImage {
                id: s(r, "id"),
                filename: s(r, "filename"),
                size_bytes: r.get("file_size_bytes").and_then(|v| v.as_u64()).unwrap_or(0),
                created_at: s(r, "created_at"),
                expires_at: s(r, "expires_at"),
                label: r.get("label").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            })
            .collect())
    }

    /// Download an export to `dest_path`.
    pub fn download_export(&self, id: String, dest_path: String) -> Result<(), PbError> {
        let c = self.cfg.lock().unwrap().clone();
        let resp = auth(self.agent.get(&format!("{}/export/{id}", c.base)), &c.key).call().map_err(read_err)?;
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut resp.into_reader(), &mut buf).map_err(err)?;
        std::fs::write(dest_path, buf).map_err(err)
    }

    pub fn delete_export(&self, id: String) -> Result<(), PbError> {
        let c = self.cfg.lock().unwrap().clone();
        auth(self.agent.delete(&format!("{}/export/{id}", c.base)), &c.key).call().map_err(read_err)?;
        Ok(())
    }
}

impl ServerClient {
    fn enqueue(&self, path: &str, json: String) {
        let mut v: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
        if let Value::Object(m) = &mut v {
            let dev = self.cfg.lock().unwrap().device.clone();
            m.entry("device_id").or_insert(Value::String(dev));
            let _ = self.queue.lock().unwrap().send((path.to_string(), v));
        }
    }
}
