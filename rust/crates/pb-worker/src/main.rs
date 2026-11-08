//! pixelboost-worker — server-side upscaling.
//!
//! The app uploads a photo with its settings and can close; this service
//! queues the job, runs the same Rust pipeline the app uses (streamed, so
//! big outputs fit in memory), and keeps the full-resolution result for the
//! app to download later. Reached only through upscaler-bridge, which
//! forwards `/jobs*` with the shared `X-Worker-Key`.

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use pb_core::fidelity::Fidelity;
use pb_core::image::Format;
use pb_core::pipeline::Settings;
use pb_core::resample::{resize, Filter};
use pb_core::stream::{self, Limits};
use pb_core::{classify, Engine, Image};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const MAX_UPLOAD: usize = 150 * 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug)]
struct JobSettings {
    #[serde(default = "auto")]
    model: String,
    #[serde(default = "four")]
    scale: f32,
    #[serde(default = "natural")]
    fidelity: String,
    #[serde(default = "detail")]
    detail: f32,
    #[serde(default)]
    denoise: f32,
    #[serde(default = "one")]
    ensemble: u32,
    /// "png" or "jpg".
    #[serde(default = "png")]
    format: String,
}

fn auto() -> String {
    "auto".into()
}
fn four() -> f32 {
    4.0
}
fn natural() -> String {
    "natural".into()
}
fn detail() -> f32 {
    0.35
}
fn one() -> u32 {
    1
}
fn png() -> String {
    "png".into()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Job {
    id: String,
    device_id: String,
    label: String,
    filename: String,
    created_at: f64,
    started_at: Option<f64>,
    finished_at: Option<f64>,
    expires_at: f64,
    /// queued | running | done | failed | cancelled
    status: String,
    progress: f32,
    done_tiles: u32,
    total_tiles: u32,
    queue_position: u32,
    settings: JobSettings,
    in_w: u32,
    in_h: u32,
    out_w: u32,
    out_h: u32,
    model_used: String,
    seconds: f64,
    fidelity_db: f64,
    capped: bool,
    result_bytes: u64,
    error: Option<String>,
}

struct Shared {
    dir: PathBuf,
    models: PathBuf,
    jobs: Mutex<HashMap<String, Job>>,
    cancel: Mutex<HashMap<String, Arc<AtomicBool>>>,
    wake: tokio::sync::Notify,
    key: String,
    ttl_hours: f64,
    max_px: usize,
}

type App = Arc<Shared>;

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64()
}

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({"detail": msg.into()}))).into_response()
}

impl Shared {
    fn job_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    fn persist(&self, job: &Job) {
        let d = self.job_dir(&job.id);
        let tmp = d.join("job.json.tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(job).unwrap()).is_ok() {
            let _ = std::fs::rename(tmp, d.join("job.json"));
        }
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Job), save: bool) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(j) = jobs.get_mut(id) {
            f(j);
            if save {
                self.persist(j);
            }
        }
    }

    fn with_positions(&self, mut list: Vec<Job>) -> Vec<Job> {
        let jobs = self.jobs.lock().unwrap();
        let mut queued: Vec<&Job> = jobs.values().filter(|j| j.status == "queued").collect();
        queued.sort_by(|a, b| a.created_at.partial_cmp(&b.created_at).unwrap());
        let running = jobs.values().any(|j| j.status == "running") as u32;
        for j in list.iter_mut() {
            j.queue_position = queued.iter().position(|q| q.id == j.id).map(|p| p as u32 + 1 + running).unwrap_or(0);
        }
        list
    }
}

// ---------------------------------------------------------------- HTTP

async fn auth(State(app): State<App>, headers: HeaderMap, req: Request, next: axum::middleware::Next) -> Response {
    if req.uri().path() == "/health" || app.key.is_empty() {
        return next.run(req).await;
    }
    match headers.get("x-worker-key").and_then(|v| v.to_str().ok()) {
        Some(k) if k == app.key => next.run(req).await,
        _ => err(StatusCode::FORBIDDEN, "bad worker key"),
    }
}

async fn health(State(app): State<App>) -> Json<Value> {
    let jobs = app.jobs.lock().unwrap();
    let queued = jobs.values().filter(|j| j.status == "queued").count();
    let running = jobs.values().filter(|j| j.status == "running").count();
    Json(json!({"status": "ok", "queued": queued, "running": running, "kernel": pb_nn_kernel()}))
}

fn pb_nn_kernel() -> &'static str {
    // pb-nn is a dependency of pb-core; ask it which SIMD path is active.
    pb_core::kernel_name()
}

async fn create(State(app): State<App>, mut mp: Multipart) -> Response {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let dir = app.job_dir(&id);
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut filename = String::from("photo");
    let mut input: Option<PathBuf> = None;
    loop {
        let f = match mp.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                let _ = tokio::fs::remove_dir_all(&dir).await;
                return err(StatusCode::BAD_REQUEST, e.to_string());
            }
        };
        let name = f.name().unwrap_or_default().to_string();
        if name == "file" {
            filename = f.file_name().unwrap_or("photo").chars().take(120).collect();
            let data = match f.bytes().await {
                Ok(d) => d,
                Err(e) => {
                    let _ = tokio::fs::remove_dir_all(&dir).await;
                    return err(StatusCode::PAYLOAD_TOO_LARGE, e.to_string());
                }
            };
            let p = dir.join("input");
            if let Err(e) = tokio::fs::write(&p, &data).await {
                return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
            }
            input = Some(p);
        } else {
            fields.insert(name, f.text().await.unwrap_or_default());
        }
    }
    let device = match fields.get("device_id").filter(|s| !s.is_empty()) {
        Some(d) => d.clone(),
        None => {
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return err(StatusCode::UNPROCESSABLE_ENTITY, "device_id is required");
        }
    };
    let Some(input) = input else {
        let _ = tokio::fs::remove_dir_all(&dir).await;
        return err(StatusCode::UNPROCESSABLE_ENTITY, "file is required");
    };
    let settings: JobSettings = match serde_json::from_str(fields.get("settings").map(|s| s.as_str()).unwrap_or("{}")) {
        Ok(s) => s,
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return err(StatusCode::UNPROCESSABLE_ENTITY, format!("settings: {e}"));
        }
    };
    // Read the size now so a bad upload fails immediately.
    let path = input.clone();
    let dims = tokio::task::spawn_blocking(move || -> Result<(usize, usize), String> {
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        pb_core::image::dimensions(&bytes)
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    let (w, h) = match dims {
        Ok(d) => d,
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return err(StatusCode::BAD_REQUEST, format!("could not read image: {e}"));
        }
    };
    let t = now();
    let job = Job {
        id: id.clone(),
        device_id: device,
        label: fields.get("label").cloned().unwrap_or_default().chars().take(120).collect(),
        filename,
        created_at: t,
        started_at: None,
        finished_at: None,
        expires_at: t + app.ttl_hours * 3600.0,
        status: "queued".into(),
        progress: 0.0,
        done_tiles: 0,
        total_tiles: 0,
        queue_position: 0,
        settings,
        in_w: w as u32,
        in_h: h as u32,
        out_w: 0,
        out_h: 0,
        model_used: String::new(),
        seconds: 0.0,
        fidelity_db: 0.0,
        capped: false,
        result_bytes: 0,
        error: None,
    };
    app.persist(&job);
    app.jobs.lock().unwrap().insert(id.clone(), job.clone());
    app.wake.notify_one();
    println!("job {id}: queued {w}x{h} {:?}", job.settings);
    Json(app.with_positions(vec![job]).pop().unwrap()).into_response()
}

async fn list(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(dev) = q.get("device_id").filter(|s| !s.is_empty()) else {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "device_id is required");
    };
    let mut v: Vec<Job> = app.jobs.lock().unwrap().values().filter(|j| &j.device_id == dev).cloned().collect();
    v.sort_by(|a, b| b.created_at.partial_cmp(&a.created_at).unwrap());
    Json(json!({"entries": app.with_positions(v)})).into_response()
}

fn find(app: &App, id: &str) -> Option<Job> {
    app.jobs.lock().unwrap().get(id).cloned()
}

async fn get_job(State(app): State<App>, Path(id): Path<String>) -> Response {
    match find(&app, &id) {
        Some(j) => Json(app.with_positions(vec![j]).pop().unwrap()).into_response(),
        None => err(StatusCode::NOT_FOUND, "Not found or expired"),
    }
}

async fn delete_job(State(app): State<App>, Path(id): Path<String>) -> Response {
    if let Some(flag) = app.cancel.lock().unwrap().get(&id) {
        flag.store(true, Ordering::SeqCst);
    }
    let existed = app.jobs.lock().unwrap().remove(&id).is_some();
    if !existed {
        return err(StatusCode::NOT_FOUND, "Not found");
    }
    let dir = app.job_dir(&id);
    // A running job finishes its current tile first; remove after it stops.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = tokio::fs::remove_dir_all(dir).await;
    });
    Json(json!({"deleted": true})).into_response()
}

/// Stream a file with Range support (resumable downloads of big results).
async fn send_file(path: PathBuf, ctype: &str, filename: Option<String>, headers: &HeaderMap) -> Response {
    let mut f = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(_) => return err(StatusCode::NOT_FOUND, "Not found"),
    };
    let len = f.metadata().await.map(|m| m.len()).unwrap_or(0);
    let mut start = 0u64;
    let mut end = len.saturating_sub(1);
    let mut partial = false;
    if let Some(r) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("bytes=")) {
        let mut it = r.splitn(2, '-');
        let a = it.next().unwrap_or("");
        let b = it.next().unwrap_or("");
        if let Ok(s) = a.parse::<u64>() {
            start = s;
            if let Ok(e) = b.parse::<u64>() {
                end = e.min(end);
            }
            partial = true;
        } else if let Ok(n) = b.parse::<u64>() {
            start = len.saturating_sub(n);
            partial = true;
        }
        if start > end || start >= len {
            return (StatusCode::RANGE_NOT_SATISFIABLE, [(header::CONTENT_RANGE, format!("bytes */{len}"))]).into_response();
        }
    }
    if start > 0 && f.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "seek failed");
    }
    let n = end - start + 1;
    let stream = tokio_util::io::ReaderStream::with_capacity(f.take(n), 256 * 1024);
    let mut resp = Response::new(Body::from_stream(stream));
    *resp.status_mut() = if partial { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK };
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(ctype).unwrap());
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(n));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if partial {
        h.insert(header::CONTENT_RANGE, HeaderValue::from_str(&format!("bytes {start}-{end}/{len}")).unwrap());
    }
    if let Some(name) = filename {
        let safe: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || "._- ".contains(*c)).collect();
        if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{safe}\"")) {
            h.insert(header::CONTENT_DISPOSITION, v);
        }
    }
    resp
}

async fn result(State(app): State<App>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    let Some(j) = find(&app, &id) else { return err(StatusCode::NOT_FOUND, "Not found or expired") };
    if j.status != "done" {
        return err(StatusCode::CONFLICT, format!("job is {}", j.status));
    }
    let ext = if j.settings.format == "jpg" { "jpg" } else { "png" };
    let stem = std::path::Path::new(&j.filename).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "photo".into());
    let name = format!("{stem}-pixelboost-{}x{}.{ext}", j.out_w, j.out_h);
    send_file(app.job_dir(&id).join(format!("result.{ext}")), if ext == "jpg" { "image/jpeg" } else { "image/png" }, Some(name), &headers).await
}

async fn thumb(State(app): State<App>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    let d = app.job_dir(&id);
    let p = if d.join("thumb.jpg").exists() { d.join("thumb.jpg") } else { d.join("input_thumb.jpg") };
    send_file(p, "image/jpeg", None, &headers).await
}

// ---------------------------------------------------------------- worker

fn make_thumb(img: &Image, path: &std::path::Path) {
    let f = 512.0 / img.w.max(img.h) as f32;
    let t = if f < 1.0 { resize(img, ((img.w as f32 * f) as usize).max(1), ((img.h as f32 * f) as usize).max(1), Filter::Box) } else { img.clone() };
    if let Ok(b) = t.encode(Format::Jpeg, 82) {
        let _ = std::fs::write(path, b);
    }
}

fn run_job(app: &App, id: &str, cancel: Arc<AtomicBool>, engines: &mut Option<(String, Engine)>) -> Result<(), String> {
    let job = find(app, id).ok_or("job vanished")?;
    let dir = app.job_dir(id);
    let img = Image::open(&dir.join("input").to_string_lossy())?;
    make_thumb(&img, &dir.join("input_thumb.jpg"));
    let s = &job.settings;
    let model = if s.model == "auto" {
        // The server has compute to spare: best model for the content, no
        // size budget.
        match classify::content_kind(&img) {
            "anime" => "anime",
            _ => "photo",
        }
        .to_string()
    } else {
        s.model.clone()
    };
    let key = format!("{model}:{:.2}", s.denoise);
    if engines.as_ref().map(|(k, _)| k != &key).unwrap_or(true) {
        *engines = None; // free the previous network first
        let denoise = (model == "fast" && s.denoise > 0.0).then_some(s.denoise);
        *engines = Some((key, Engine::load(&app.models, &model, denoise)?));
    }
    let engine = &engines.as_ref().unwrap().1;
    let st = Settings {
        scale: s.scale,
        fidelity: Fidelity::parse(&s.fidelity).unwrap_or(Fidelity::Natural),
        detail: s.detail.clamp(0.0, 1.0),
        tile: 192,
        pad: 16,
        ensemble: s.ensemble.max(1) as usize,
    };
    app.update(id, |j| j.model_used = model.clone(), false);
    let mut last_save = std::time::Instant::now();
    let res = stream::upscale(engine, &img, &st, Limits { max_output_px: app.max_px }, &mut |e| {
        let save = last_save.elapsed() > Duration::from_secs(10);
        if save {
            last_save = std::time::Instant::now();
        }
        app.update(
            id,
            |j| {
                j.done_tiles = e.done as u32;
                j.total_tiles = e.total as u32;
                j.progress = e.done as f32 / e.total.max(1) as f32;
            },
            save,
        );
        !cancel.load(Ordering::SeqCst)
    });
    drop(img);
    let (out, rep, capped) = res.ok_or("cancelled")?;
    let ext = if s.format == "jpg" { "jpg" } else { "png" };
    let rp = dir.join(format!("result.{ext}"));
    out.save(&rp.to_string_lossy(), 95)?;
    // Result thumbnail from the 8-bit output (box shrink in rows).
    let f = (512.0 / out.w.max(out.h) as f32).min(1.0);
    let (tw, th) = (((out.w as f32 * f) as usize).max(1), ((out.h as f32 * f) as usize).max(1));
    let mut t = Image::new(tw, th);
    for y in 0..th {
        for x in 0..tw {
            let (sx, sy) = ((x * out.w) / tw, (y * out.h) / th);
            let c = &out.data[(sy * out.w + sx) * 4..(sy * out.w + sx) * 4 + 4];
            t.px[y * tw + x] = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0];
        }
    }
    if let Ok(b) = t.encode(Format::Jpeg, 85) {
        let _ = std::fs::write(dir.join("thumb.jpg"), b);
    }
    let bytes = std::fs::metadata(&rp).map(|m| m.len()).unwrap_or(0);
    app.update(
        id,
        |j| {
            j.out_w = rep.out_w as u32;
            j.out_h = rep.out_h as u32;
            j.seconds = rep.seconds;
            j.fidelity_db = rep.fidelity_db;
            j.capped = capped;
            j.result_bytes = bytes;
            j.progress = 1.0;
        },
        false,
    );
    // The input is no longer needed.
    let _ = std::fs::remove_file(dir.join("input"));
    Ok(())
}

fn worker_loop(app: App, rt: tokio::runtime::Handle) {
    let mut engines: Option<(String, Engine)> = None;
    loop {
        let next = {
            let jobs = app.jobs.lock().unwrap();
            let mut q: Vec<&Job> = jobs.values().filter(|j| j.status == "queued").collect();
            q.sort_by(|a, b| a.created_at.partial_cmp(&b.created_at).unwrap());
            q.first().map(|j| j.id.clone())
        };
        let Some(id) = next else {
            // Drop the network while idle (it is ~100 MB) after a while.
            let woke = rt.block_on(async { tokio::time::timeout(Duration::from_secs(300), app.wake.notified()).await.is_ok() });
            if !woke {
                engines = None;
            }
            continue;
        };
        let flag = Arc::new(AtomicBool::new(false));
        app.cancel.lock().unwrap().insert(id.clone(), flag.clone());
        app.update(
            &id,
            |j| {
                j.status = "running".into();
                j.started_at = Some(now());
            },
            true,
        );
        println!("job {id}: running");
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_job(&app, &id, flag.clone(), &mut engines))).unwrap_or_else(|_| Err("internal error".into()));
        app.cancel.lock().unwrap().remove(&id);
        let cancelled = flag.load(Ordering::SeqCst);
        app.update(
            &id,
            |j| {
                j.finished_at = Some(now());
                match &r {
                    Ok(()) => j.status = "done".into(),
                    Err(_) if cancelled => j.status = "cancelled".into(),
                    Err(e) => {
                        j.status = "failed".into();
                        j.error = Some(e.clone());
                    }
                }
            },
            true,
        );
        match &r {
            Ok(()) => println!("job {id}: done"),
            Err(e) => println!("job {id}: {e}"),
        }
    }
}

fn load_jobs(app: &Shared) {
    let Ok(rd) = std::fs::read_dir(&app.dir) else { return };
    let mut jobs = app.jobs.lock().unwrap();
    for e in rd.flatten() {
        let p = e.path().join("job.json");
        let Some(mut j) = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Job>(&b).ok()) else {
            continue;
        };
        if j.status == "running" {
            // Interrupted by a restart: run it again.
            j.status = "queued".into();
            j.progress = 0.0;
            j.done_tiles = 0;
        }
        jobs.insert(j.id.clone(), j);
    }
}

async fn sweeper(app: App) {
    loop {
        let t = now();
        let expired: Vec<String> = app.jobs.lock().unwrap().values().filter(|j| j.expires_at < t && j.status != "running").map(|j| j.id.clone()).collect();
        for id in expired {
            app.jobs.lock().unwrap().remove(&id);
            let _ = tokio::fs::remove_dir_all(app.job_dir(&id)).await;
            println!("job {id}: expired");
        }
        tokio::time::sleep(Duration::from_secs(1800)).await;
    }
}

fn env(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

#[tokio::main]
async fn main() {
    let dir = PathBuf::from(env("JOBS_DIR", "./jobs"));
    std::fs::create_dir_all(&dir).expect("jobs dir");
    let app: App = Arc::new(Shared {
        dir,
        models: PathBuf::from(env("MODELS_DIR", "./models")),
        jobs: Mutex::new(HashMap::new()),
        cancel: Mutex::new(HashMap::new()),
        wake: tokio::sync::Notify::new(),
        key: env("WORKER_KEY", ""),
        ttl_hours: env("RESULT_TTL_HOURS", "168").parse().unwrap_or(168.0),
        max_px: (env("MAX_OUTPUT_MP", "300").parse::<f64>().unwrap_or(300.0) * 1e6) as usize,
    });
    if app.key.is_empty() {
        eprintln!("warning: WORKER_KEY is empty — anyone who can reach this port can submit jobs");
    }
    load_jobs(&app);
    let (a2, rt) = (app.clone(), tokio::runtime::Handle::current());
    std::thread::Builder::new().name("upscale".into()).spawn(move || worker_loop(a2, rt)).unwrap();
    tokio::spawn(sweeper(app.clone()));

    let router = Router::new()
        .route("/health", get(health))
        .route("/jobs", get(list).post(create))
        .route("/jobs/{id}", get(get_job).delete(delete_job))
        .route("/jobs/{id}/result", get(result))
        .route("/jobs/{id}/thumb", get(thumb))
        .layer(middleware::from_fn_with_state(app.clone(), auth))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD))
        .with_state(app);
    let addr = format!("{}:{}", env("HOST", "0.0.0.0"), env("PORT", "8095"));
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    println!("pixelboost-worker listening on {addr} (kernel {})", pb_core::kernel_name());
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .unwrap();
}
