//! upscaler-bridge — PixelBoost's optional server: debug logging,
//! temporary image storage, device settings backup, model registry.
//! Same routes, payloads and Postgres schema as the FastAPI version it
//! replaces, so existing app builds keep working.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde_json::{json, Map, Value};
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

const VERSION: &str = "2.0.0";
const MAX_UPLOAD_BYTES: usize = 60 * 1024 * 1024;
const DEFAULT_TTL_HOURS: i64 = 24;
const MAX_TTL_HOURS: i64 = 24 * 7;

#[derive(Clone)]
struct App {
    db: PgPool,
    api_key: Arc<String>,
}

// ---------------------------------------------------------------- errors

struct ApiError(StatusCode, Value);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "detail": self.1 }))).into_response()
    }
}

fn err(code: StatusCode, msg: impl Into<String>) -> ApiError {
    ApiError(code, Value::String(msg.into()))
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        if let sqlx::Error::Database(d) = &e {
            // Bad types / constraint violations are the client's fault.
            if let Some(code) = d.code() {
                if code.starts_with("22") || code.starts_with("23") {
                    return err(StatusCode::UNPROCESSABLE_ENTITY, d.message().to_string());
                }
            }
        }
        eprintln!("db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    }
}

type R<T = Json<Value>> = Result<T, ApiError>;

// ---------------------------------------------------------------- helpers

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[derive(Clone, Copy)]
enum Ty {
    Str,
    Int,
    Bool,
    Arr,
}

/// FastAPI-style validation of required fields (422 with a detail list).
fn require(body: &Value, fields: &[(&str, Ty)]) -> Result<(), ApiError> {
    let Some(obj) = body.as_object() else {
        return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "body must be a JSON object"));
    };
    let mut problems = Vec::new();
    for (name, ty) in fields {
        let ok = match (obj.get(*name), ty) {
            (Some(Value::String(_)), Ty::Str) => true,
            (Some(Value::Number(n)), Ty::Int) => n.is_i64() || n.is_u64(),
            (Some(Value::Bool(_)), Ty::Bool) => true,
            (Some(Value::Array(_)), Ty::Arr) => true,
            _ => false,
        };
        if !ok {
            problems.push(json!({"loc": ["body", name], "msg": "field required or wrong type", "type": "value_error"}));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, Value::Array(problems)))
    }
}

/// Insert a JSON object as a row: columns are matched by name, unknown keys
/// ignored, `id` and `created_at` set here.
async fn insert_json(db: &PgPool, table: &str, body: &Value, id: &str) -> Result<(), ApiError> {
    let sql = format!(
        "INSERT INTO {table} SELECT * FROM json_populate_record(NULL::{table}, ($1::jsonb || jsonb_build_object('id', $2::text, 'created_at', LOCALTIMESTAMP))::json)"
    );
    sqlx::query(&sql).bind(body).bind(id).execute(db).await?;
    Ok(())
}

/// Run a SELECT and return its rows as a JSON array.
fn rows_sql(inner: &str) -> String {
    format!("SELECT COALESCE(json_agg(t), '[]'::json) FROM ({inner}) t")
}

fn row_sql(inner: &str) -> String {
    format!("SELECT row_to_json(t) FROM ({inner}) t")
}

fn limit_offset(q: &HashMap<String, String>) -> Result<(i64, i64), ApiError> {
    let limit: i64 = q.get("limit").map(|v| v.parse().unwrap_or(-1)).unwrap_or(50);
    let offset: i64 = q.get("offset").map(|v| v.parse().unwrap_or(-1)).unwrap_or(0);
    if !(1..=200).contains(&limit) || offset < 0 {
        return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "limit must be 1..200 and offset >= 0"));
    }
    Ok((limit, offset))
}

fn device_param(q: &HashMap<String, String>) -> Result<String, ApiError> {
    q.get("device_id").filter(|v| !v.is_empty()).cloned().ok_or_else(|| err(StatusCode::UNPROCESSABLE_ENTITY, "device_id is required"))
}

// ---------------------------------------------------------------- auth

async fn auth(State(app): State<App>, headers: HeaderMap, req: Request, next: Next) -> Response {
    if app.api_key.is_empty() || req.uri().path() == "/health" {
        return next.run(req).await;
    }
    let h = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("");
    match h.strip_prefix("Bearer ") {
        None => err(StatusCode::UNAUTHORIZED, "Missing Authorization header").into_response(),
        Some(t) if t == app.api_key.as_str() => next.run(req).await,
        Some(_) => err(StatusCode::FORBIDDEN, "Invalid API key").into_response(),
    }
}

// ---------------------------------------------------------------- logging

async fn health() -> Json<Value> {
    Json(json!({"status": "ok", "version": VERSION}))
}

async fn log_upscale(State(app): State<App>, Json(body): Json<Value>) -> R {
    require(&body, &[("device_id", Ty::Str), ("source_width", Ty::Int), ("source_height", Ty::Int), ("technique", Ty::Str), ("scale_factor", Ty::Int), ("processing_ms", Ty::Int), ("success", Ty::Bool)])?;
    let id = new_id();
    insert_json(&app.db, "upscale_history", &body, &id).await?;
    Ok(Json(json!({"id": id})))
}

async fn get_history(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> R {
    let (limit, offset) = limit_offset(&q)?;
    let cols = "id, device_id, created_at, source_width, source_height, source_file_size_bytes, technique, model_name, tile_size, overlap, scale_factor, tile_count, output_width, output_height, processing_ms, success, error_message, app_version, os_version, device_model";
    let v: Value = match q.get("device_id") {
        Some(d) => sqlx::query_scalar(&rows_sql(&format!("SELECT {cols} FROM upscale_history WHERE device_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3"))).bind(d).bind(limit).bind(offset).fetch_one(&app.db).await?,
        None => sqlx::query_scalar(&rows_sql(&format!("SELECT {cols} FROM upscale_history ORDER BY created_at DESC LIMIT $1 OFFSET $2"))).bind(limit).bind(offset).fetch_one(&app.db).await?,
    };
    Ok(Json(json!({"entries": v})))
}

async fn delete_history_entry(State(app): State<App>, Path(id): Path<String>) -> R {
    let n = sqlx::query("DELETE FROM upscale_history WHERE id = $1").bind(&id).execute(&app.db).await?.rows_affected();
    if n == 0 {
        return Err(err(StatusCode::NOT_FOUND, "History entry not found"));
    }
    Ok(Json(json!({"deleted": true})))
}

async fn clear_history(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> R {
    let d = device_param(&q)?;
    let n = sqlx::query("DELETE FROM upscale_history WHERE device_id = $1").bind(&d).execute(&app.db).await?.rows_affected();
    Ok(Json(json!({"deleted_count": n})))
}

const ACTION_FIELDS: &[(&str, Ty)] = &[("device_id", Ty::Str), ("action", Ty::Str)];

async fn log_action(State(app): State<App>, Json(body): Json<Value>) -> R {
    require(&body, ACTION_FIELDS)?;
    let id = new_id();
    insert_json(&app.db, "action_log", &body, &id).await?;
    Ok(Json(json!({"id": id})))
}

async fn log_actions(State(app): State<App>, Json(body): Json<Value>) -> R {
    require(&body, &[("entries", Ty::Arr)])?;
    let entries = body["entries"].as_array().cloned().unwrap_or_default();
    if entries.is_empty() {
        return Ok(Json(json!({"ids": []})));
    }
    if entries.len() > 200 {
        return Err(err(StatusCode::PAYLOAD_TOO_LARGE, "At most 200 entries per batch"));
    }
    for e in &entries {
        require(e, ACTION_FIELDS)?;
    }
    let mut tx = app.db.begin().await?;
    let mut ids = Vec::with_capacity(entries.len());
    for e in &entries {
        let id = new_id();
        sqlx::query("INSERT INTO action_log SELECT * FROM json_populate_record(NULL::action_log, ($1::jsonb || jsonb_build_object('id', $2::text, 'created_at', LOCALTIMESTAMP))::json)")
            .bind(e)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        ids.push(id);
    }
    tx.commit().await?;
    Ok(Json(json!({"ids": ids})))
}

async fn get_action_history(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> R {
    let (limit, offset) = limit_offset(&q)?;
    let v: Value = sqlx::query_scalar(&rows_sql(
        "SELECT id, device_id, action, detail, created_at, app_version, os_version, device_model, session_id, outcome, duration_ms, thermal_state
         FROM action_log WHERE ($1::text IS NULL OR device_id = $1) AND ($2::text IS NULL OR action = $2)
         ORDER BY created_at DESC LIMIT $3 OFFSET $4",
    ))
    .bind(q.get("device_id").filter(|s| !s.is_empty()))
    .bind(q.get("action").filter(|s| !s.is_empty()))
    .bind(limit)
    .bind(offset)
    .fetch_one(&app.db)
    .await?;
    Ok(Json(json!({"entries": v})))
}

async fn log_snapshot(State(app): State<App>, Json(body): Json<Value>) -> R {
    require(&body, &[("device_id", Ty::Str)])?;
    let id = new_id();
    insert_json(&app.db, "device_snapshots", &body, &id).await?;
    Ok(Json(json!({"id": id})))
}

async fn get_snapshots(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> R {
    let d = device_param(&q)?;
    let (limit, offset) = limit_offset(&q)?;
    let v: Value = sqlx::query_scalar(&rows_sql("SELECT * FROM device_snapshots WHERE device_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3")).bind(&d).bind(limit).bind(offset).fetch_one(&app.db).await?;
    Ok(Json(json!({"entries": v})))
}

async fn get_stats(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> R {
    let d = device_param(&q)?;
    let row: Value = sqlx::query_scalar(&row_sql(
        "SELECT COUNT(*) AS total, COUNT(*) FILTER (WHERE success) AS successes, AVG(processing_ms)::float8 AS avg_processing_ms,
                COALESCE(SUM(CASE WHEN success THEN output_width::bigint * output_height ELSE 0 END), 0) AS total_output_pixels
         FROM upscale_history WHERE device_id = $1",
    ))
    .bind(&d)
    .fetch_one(&app.db)
    .await?;
    let total = row["total"].as_i64().unwrap_or(0);
    let successes = row["successes"].as_i64().unwrap_or(0);
    Ok(Json(json!({
        "total": total,
        "successes": successes,
        "failures": total - successes,
        "success_rate": if total > 0 { json!(successes as f64 / total as f64) } else { Value::Null },
        "avg_processing_ms": row["avg_processing_ms"],
        "total_output_pixels": row["total_output_pixels"].as_i64().unwrap_or(0),
    })))
}

// ---------------------------------------------------------------- comparisons

async fn log_comparison(State(app): State<App>, Json(body): Json<Value>) -> R {
    require(&body, &[("comparison_id", Ty::Str), ("device_id", Ty::Str)])?;
    let cands = body.get("candidates").and_then(|c| c.as_array()).cloned().unwrap_or_default();
    for c in &cands {
        require(c, &[("model_name", Ty::Str), ("candidate_index", Ty::Int), ("succeeded", Ty::Bool)])?;
    }
    let cid = body["comparison_id"].as_str().unwrap().to_string();
    let succeeded = cands.iter().filter(|c| c["succeeded"].as_bool() == Some(true)).count() as i64;
    let mut row = body.as_object().cloned().unwrap_or_default();
    row.remove("candidates");
    row.insert("candidate_count".into(), json!(cands.len()));
    row.insert("succeeded_count".into(), json!(succeeded));
    row.insert("cancelled".into(), json!(body["cancelled"].as_bool().unwrap_or(false)));
    row.remove("picked_model");
    row.remove("picked_at");
    let mut tx = app.db.begin().await?;
    sqlx::query(
        "INSERT INTO model_comparisons SELECT * FROM json_populate_record(NULL::model_comparisons, ($1::jsonb || jsonb_build_object('created_at', LOCALTIMESTAMP))::json)
         ON CONFLICT (comparison_id) DO UPDATE SET candidate_count = EXCLUDED.candidate_count, succeeded_count = EXCLUDED.succeeded_count,
           cancelled = EXCLUDED.cancelled, total_ms = EXCLUDED.total_ms, auto_pick_model = EXCLUDED.auto_pick_model, thermal_state_end = EXCLUDED.thermal_state_end",
    )
    .bind(Value::Object(row))
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM model_comparison_candidates WHERE comparison_id = $1").bind(&cid).execute(&mut *tx).await?;
    for c in &cands {
        let mut m: Map<String, Value> = c.as_object().cloned().unwrap_or_default();
        m.insert("comparison_id".into(), json!(cid));
        sqlx::query("INSERT INTO model_comparison_candidates SELECT * FROM json_populate_record(NULL::model_comparison_candidates, ($1::jsonb || jsonb_build_object('id', $2::text, 'created_at', LOCALTIMESTAMP))::json)")
            .bind(Value::Object(m))
            .bind(new_id())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"comparison_id": cid, "candidates": cands.len()})))
}

async fn log_comparison_pick(State(app): State<App>, Path(cid): Path<String>, Json(body): Json<Value>) -> R {
    require(&body, &[("model_name", Ty::Str)])?;
    let m = body["model_name"].as_str().unwrap();
    let n = sqlx::query("UPDATE model_comparisons SET picked_model = $1, picked_at = CURRENT_TIMESTAMP WHERE comparison_id = $2").bind(m).bind(&cid).execute(&app.db).await?.rows_affected();
    if n == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Unknown comparison_id"));
    }
    Ok(Json(json!({"comparison_id": cid, "picked_model": m})))
}

async fn get_comparisons(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> R {
    let (limit, offset) = limit_offset(&q)?;
    let v: Value = sqlx::query_scalar(&rows_sql(
        "SELECT c.comparison_id, c.device_id, c.created_at, c.source_width, c.source_height, c.candidate_count, c.succeeded_count, c.cancelled, c.total_ms,
                c.auto_pick_model, c.picked_model, c.picked_at, c.source_noise_sigma, c.thermal_state_start, c.thermal_state_end,
                c.app_version, c.os_version, c.device_model,
                COALESCE((SELECT json_agg(x ORDER BY x.candidate_index) FROM (
                    SELECT model_name, candidate_index, succeeded, processing_ms, sharpness_score, fidelity_psnr, tile_count, tiles_reused,
                           render_denoise_applied, error_message
                    FROM model_comparison_candidates k WHERE k.comparison_id = c.comparison_id) x), '[]'::json) AS candidates
         FROM model_comparisons c WHERE ($1::text IS NULL OR c.device_id = $1)
         ORDER BY c.created_at DESC LIMIT $2 OFFSET $3",
    ))
    .bind(q.get("device_id").filter(|s| !s.is_empty()))
    .bind(limit)
    .bind(offset)
    .fetch_one(&app.db)
    .await?;
    Ok(Json(json!({"entries": v})))
}

async fn get_overview(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> R {
    let days: i32 = q.get("days").map(|v| v.parse().unwrap_or(0)).unwrap_or(7);
    if !(1..=90).contains(&days) {
        return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "days must be 1..90"));
    }
    let w = format!("created_at > NOW() - make_interval(days => {days})");
    let db = &app.db;
    let totals: Value = sqlx::query_scalar(&row_sql(&format!(
        "SELECT COUNT(*) AS runs, COUNT(*) FILTER (WHERE success) AS successes, COUNT(*) FILTER (WHERE cancelled) AS cancelled, COUNT(DISTINCT device_id) AS devices FROM upscale_history WHERE {w}"
    )))
    .fetch_one(db)
    .await?;
    let q_rows = |s: String| async move { sqlx::query_scalar::<_, Value>(&rows_sql(&s)).fetch_one(db).await };
    let by_model = q_rows(format!(
        "SELECT COALESCE(run_kind, 'unknown') AS run_kind, model_name, COUNT(*) AS runs, COUNT(*) FILTER (WHERE NOT success) AS failures,
                ROUND(AVG(processing_ms)) AS avg_ms, PERCENTILE_DISC(0.95) WITHIN GROUP (ORDER BY processing_ms) AS p95_ms,
                ROUND(AVG(fidelity_psnr)::numeric, 2) AS avg_fidelity_psnr, ROUND(AVG(peak_memory_mb)) AS avg_peak_memory_mb,
                COUNT(*) FILTER (WHERE output_was_capped) AS capped, COUNT(*) FILTER (WHERE thermal_state_end IN ('serious', 'critical')) AS ended_hot
         FROM upscale_history WHERE {w} GROUP BY 1, 2 ORDER BY runs DESC"
    ))
    .await?;
    let by_version = q_rows(format!(
        "SELECT COALESCE(app_version, 'unknown') AS app_version, COUNT(*) AS runs, COUNT(*) FILTER (WHERE NOT success) AS failures,
                ROUND(AVG(processing_ms)) AS avg_ms, COUNT(DISTINCT device_id) AS devices
         FROM upscale_history WHERE {w} GROUP BY 1 ORDER BY runs DESC"
    ))
    .await?;
    let top_errors = q_rows(format!(
        "SELECT error_message, model_name, COUNT(*) AS occurrences, COUNT(DISTINCT device_id) AS devices, MAX(created_at) AS last_seen
         FROM upscale_history WHERE {w} AND NOT success AND error_message IS NOT NULL GROUP BY 1, 2 ORDER BY occurrences DESC LIMIT 20"
    ))
    .await?;
    let by_action = q_rows(format!(
        "SELECT action, COALESCE(outcome, 'unknown') AS outcome, COUNT(*) AS events, COUNT(DISTINCT device_id) AS devices, ROUND(AVG(duration_ms)) AS avg_duration_ms
         FROM action_log WHERE {w} GROUP BY 1, 2 ORDER BY events DESC LIMIT 60"
    ))
    .await?;
    let model_picks = q_rows(format!(
        "SELECT c.model_name, COUNT(*) AS shown, COUNT(*) FILTER (WHERE m.picked_model = c.model_name) AS picks,
                ROUND(AVG(c.sharpness_score)::numeric, 4) AS avg_sharpness, ROUND(AVG(c.processing_ms)) AS avg_ms,
                COUNT(*) FILTER (WHERE NOT c.succeeded) AS failures
         FROM model_comparison_candidates c JOIN model_comparisons m ON m.comparison_id = c.comparison_id
         WHERE c.{w} GROUP BY 1 ORDER BY picks DESC, shown DESC"
    ))
    .await?;
    let agree: Value = sqlx::query_scalar(&row_sql(&format!(
        "SELECT COUNT(*) AS decided_sweeps, COUNT(*) FILTER (WHERE picked_model = auto_pick_model) AS auto_agreed
         FROM model_comparisons WHERE {w} AND picked_model IS NOT NULL AND auto_pick_model IS NOT NULL"
    )))
    .fetch_one(db)
    .await?;
    let thermal = q_rows(format!(
        "SELECT COALESCE(thermal_state, 'unknown') AS thermal_state, COUNT(*) AS snapshots, COUNT(DISTINCT device_id) AS devices,
                ROUND(AVG(used_memory_mb)) AS avg_used_memory_mb, ROUND(AVG(free_disk_mb)) AS avg_free_disk_mb
         FROM device_snapshots WHERE {w} GROUP BY 1 ORDER BY snapshots DESC"
    ))
    .await?;
    let runs = totals["runs"].as_i64().unwrap_or(0);
    let succ = totals["successes"].as_i64().unwrap_or(0);
    let decided = agree["decided_sweeps"].as_i64().unwrap_or(0);
    let agreed = agree["auto_agreed"].as_i64().unwrap_or(0);
    Ok(Json(json!({
        "window_days": days,
        "totals": {
            "runs": runs, "successes": succ, "failures": runs - succ,
            "success_rate": if runs > 0 { json!(succ as f64 / runs as f64) } else { Value::Null },
            "cancelled": totals["cancelled"].as_i64().unwrap_or(0),
            "devices": totals["devices"].as_i64().unwrap_or(0),
        },
        "by_model": by_model,
        "by_app_version": by_version,
        "top_errors": top_errors,
        "by_action": by_action,
        "model_picks": model_picks,
        "auto_pick_agreement": {
            "decided_sweeps": decided, "auto_agreed": agreed,
            "agreement_rate": if decided > 0 { json!(agreed as f64 / decided as f64) } else { Value::Null },
        },
        "thermal_states": thermal,
    })))
}

// ---------------------------------------------------------------- storage

#[derive(Clone, Copy)]
enum Store {
    Imports,
    Exports,
}

impl Store {
    fn table(self) -> &'static str {
        match self {
            Store::Imports => "image_imports",
            Store::Exports => "image_exports",
        }
    }
    fn default_type(self) -> &'static str {
        match self {
            Store::Imports => "image/jpeg",
            Store::Exports => "image/png",
        }
    }
}

async fn cleanup_once(db: &PgPool) {
    let a = sqlx::query("DELETE FROM image_imports WHERE expires_at < NOW()").execute(db).await.map(|r| r.rows_affected()).unwrap_or(0);
    let b = sqlx::query("DELETE FROM image_exports WHERE expires_at < NOW()").execute(db).await.map(|r| r.rows_affected()).unwrap_or(0);
    if a + b > 0 {
        println!("cleanup: deleted {a} expired imports, {b} expired exports");
    }
}

async fn create_stored(app: App, store: Store, mut mp: Multipart) -> R {
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut file: Option<(Option<String>, Option<String>, Bytes)> = None;
    while let Some(f) = mp.next_field().await.map_err(|e| err(StatusCode::BAD_REQUEST, e.to_string()))? {
        let name = f.name().unwrap_or_default().to_string();
        if name == "file" {
            let fname = f.file_name().map(String::from);
            let ctype = f.content_type().map(String::from);
            let data = f.bytes().await.map_err(|e| err(StatusCode::PAYLOAD_TOO_LARGE, e.to_string()))?;
            file = Some((fname, ctype, data));
        } else {
            fields.insert(name, f.text().await.unwrap_or_default());
        }
    }
    let device = fields.get("device_id").filter(|s| !s.is_empty()).cloned().ok_or_else(|| err(StatusCode::UNPROCESSABLE_ENTITY, "device_id is required"))?;
    let (fname, ctype, data) = file.ok_or_else(|| err(StatusCode::UNPROCESSABLE_ENTITY, "file is required"))?;
    if data.len() > MAX_UPLOAD_BYTES {
        return Err(err(StatusCode::PAYLOAD_TOO_LARGE, format!("Upload exceeds {MAX_UPLOAD_BYTES}-byte limit")));
    }
    let size = imagesize::blob_size(&data).map_err(|_| err(StatusCode::BAD_REQUEST, "Could not read image data"))?;
    let hours = fields.get("ttl_hours").and_then(|v| v.parse::<i64>().ok()).map(|h| h.clamp(1, MAX_TTL_HOURS)).unwrap_or(DEFAULT_TTL_HOURS);
    let is_auto = matches!(fields.get("is_auto").map(|s| s.to_ascii_lowercase()).as_deref(), Some("true" | "1" | "yes" | "on"));
    let label: Option<String> = fields.get("label").filter(|s| !s.is_empty()).map(|s| s.chars().take(120).collect());
    let history_id = match store {
        Store::Exports => fields.get("history_id").filter(|s| !s.is_empty()).cloned(),
        Store::Imports => None,
    };
    let id = new_id();
    let sql = match store {
        Store::Imports => "INSERT INTO image_imports (id, device_id, expires_at, filename, content_type, width, height, file_size_bytes, image_data, is_auto, label)
                            VALUES ($1, $2, NOW() + make_interval(hours => $3::int), $4, $5, $6, $7, $8, $9, $10, $11)",
        Store::Exports => "INSERT INTO image_exports (id, device_id, expires_at, filename, content_type, width, height, file_size_bytes, image_data, is_auto, label, history_id)
                            VALUES ($1, $2, NOW() + make_interval(hours => $3::int), $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    };
    let mut q = sqlx::query(sql)
        .bind(&id)
        .bind(&device)
        .bind(hours as i32)
        .bind(fname)
        .bind(ctype.unwrap_or_else(|| store.default_type().to_string()))
        .bind(size.width as i32)
        .bind(size.height as i32)
        .bind(data.len() as i32)
        .bind(data.to_vec())
        .bind(is_auto)
        .bind(label);
    if let Store::Exports = store {
        q = q.bind(history_id);
    }
    q.execute(&app.db).await?;
    let db = app.db.clone();
    tokio::spawn(async move { cleanup_once(&db).await });
    Ok(Json(json!({"id": id, "expires_in_hours": hours, "width": size.width, "height": size.height, "file_size_bytes": data.len()})))
}

async fn get_stored(app: &App, store: Store, id: &str) -> Result<Response, ApiError> {
    let row: Option<(Vec<u8>, Option<String>)> = sqlx::query_as(&format!("SELECT image_data, content_type FROM {} WHERE id = $1 AND expires_at > NOW()", store.table())).bind(id).fetch_optional(&app.db).await?;
    let (data, ctype) = row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Not found or expired"))?;
    Ok(([(header::CONTENT_TYPE, ctype.unwrap_or_else(|| store.default_type().into()))], data).into_response())
}

async fn list_stored(app: &App, store: Store, q: &HashMap<String, String>) -> R {
    let d = device_param(q)?;
    let v: Value = sqlx::query_scalar(&rows_sql(&format!(
        "SELECT id, device_id, created_at, expires_at, filename, content_type, width, height, file_size_bytes, is_auto, label
         FROM {} WHERE device_id = $1 AND expires_at > NOW() ORDER BY created_at DESC",
        store.table()
    )))
    .bind(&d)
    .fetch_one(&app.db)
    .await?;
    Ok(Json(json!({"entries": v})))
}

async fn delete_stored(app: &App, store: Store, id: &str) -> R {
    let n = sqlx::query(&format!("DELETE FROM {} WHERE id = $1", store.table())).bind(id).execute(&app.db).await?.rows_affected();
    if n == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Not found"));
    }
    Ok(Json(json!({"deleted": true})))
}

async fn clear_stored(app: &App, store: Store, q: &HashMap<String, String>) -> R {
    let d = device_param(q)?;
    let n = sqlx::query(&format!("DELETE FROM {} WHERE device_id = $1", store.table())).bind(&d).execute(&app.db).await?.rows_affected();
    Ok(Json(json!({"deleted": n})))
}

async fn storage_usage(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> R {
    let d = device_param(&q)?;
    let mut out = Map::new();
    for (store, key) in [(Store::Imports, "imports"), (Store::Exports, "exports")] {
        let v: Value = sqlx::query_scalar(&row_sql(&format!(
            "SELECT COUNT(*) AS count, COALESCE(SUM(file_size_bytes), 0) AS total_bytes, MIN(expires_at) AS next_expiry FROM {} WHERE device_id = $1 AND expires_at > NOW()",
            store.table()
        )))
        .bind(&d)
        .fetch_one(&app.db)
        .await?;
        out.insert(key.into(), v);
    }
    Ok(Json(Value::Object(out)))
}

// ---------------------------------------------------------------- presets / settings / models

async fn create_preset(State(app): State<App>, Json(body): Json<Value>) -> R {
    require(&body, &[("device_id", Ty::Str), ("name", Ty::Str), ("model_name", Ty::Str), ("overlap", Ty::Int)])?;
    let id: String = sqlx::query_scalar(
        "INSERT INTO custom_presets (id, device_id, name, model_name, overlap) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (device_id, name) DO UPDATE SET model_name = EXCLUDED.model_name, overlap = EXCLUDED.overlap
         RETURNING id",
    )
    .bind(new_id())
    .bind(body["device_id"].as_str())
    .bind(body["name"].as_str())
    .bind(body["model_name"].as_str())
    .bind(body["overlap"].as_i64().map(|v| v as i32))
    .fetch_one(&app.db)
    .await?;
    Ok(Json(json!({"id": id})))
}

async fn list_presets(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> R {
    let d = device_param(&q)?;
    let v: Value = sqlx::query_scalar(&rows_sql("SELECT id, device_id, name, model_name, overlap, created_at FROM custom_presets WHERE device_id = $1 ORDER BY created_at DESC")).bind(&d).fetch_one(&app.db).await?;
    Ok(Json(json!({"entries": v})))
}

async fn delete_preset(State(app): State<App>, Path(id): Path<String>) -> R {
    let n = sqlx::query("DELETE FROM custom_presets WHERE id = $1").bind(&id).execute(&app.db).await?.rows_affected();
    if n == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Preset not found"));
    }
    Ok(Json(json!({"deleted": true})))
}

async fn put_device_settings(State(app): State<App>, Json(body): Json<Value>) -> R {
    require(&body, &[("device_id", Ty::Str)])?;
    sqlx::query(
        "INSERT INTO device_settings (device_id, haptics_enabled, model_choice, quality) VALUES ($1, $2, $3, $4)
         ON CONFLICT (device_id) DO UPDATE SET haptics_enabled = EXCLUDED.haptics_enabled, model_choice = EXCLUDED.model_choice,
           quality = EXCLUDED.quality, updated_at = CURRENT_TIMESTAMP",
    )
    .bind(body["device_id"].as_str())
    .bind(body["haptics_enabled"].as_bool().unwrap_or(true))
    .bind(body["model_choice"].as_str().unwrap_or("generalPhoto"))
    .bind(body["quality"].as_str().unwrap_or("standard"))
    .execute(&app.db)
    .await?;
    Ok(Json(json!({"saved": true})))
}

async fn get_device_settings(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> R {
    let d = device_param(&q)?;
    let v: Option<Value> = sqlx::query_scalar(&row_sql("SELECT device_id, COALESCE(haptics_enabled, TRUE) AS haptics_enabled, model_choice, quality, updated_at FROM device_settings WHERE device_id = $1")).bind(&d).fetch_optional(&app.db).await?;
    v.map(Json).ok_or_else(|| err(StatusCode::NOT_FOUND, "No backup found for this device"))
}

async fn list_models(State(app): State<App>) -> R {
    let v: Value = sqlx::query_scalar(&rows_sql("SELECT model_name, display_name, description, license, tile_size, scale_factor, is_active FROM model_registry WHERE is_active = TRUE")).fetch_one(&app.db).await?;
    Ok(Json(json!({"entries": v})))
}

// ---------------------------------------------------------------- startup

fn strip_comments(sql: &str) -> String {
    sql.lines().map(|l| l.find("--").map_or(l, |i| &l[..i])).collect::<Vec<_>>().join("\n")
}

async fn init_db(db: &PgPool) -> Result<(), sqlx::Error> {
    let sql = strip_comments(include_str!("schema.sql"));
    let mut tx = db.begin().await?;
    for stmt in sql.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        sqlx::query(stmt).execute(&mut *tx).await?;
    }
    tx.commit().await
}

fn env(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

#[tokio::main]
async fn main() {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        format!(
            "postgres://{}:{}@{}:{}/{}",
            env("DB_USER", "upscaler"),
            urlenc(&env("DB_PASSWORD", "")),
            env("DB_HOST", "127.0.0.1"),
            env("DB_PORT", "5432"),
            env("DB_NAME", "image_upscaler")
        )
    });
    let db = PgPoolOptions::new()
        .max_connections(env("DB_POOL_MAX_SIZE", "4").parse().unwrap_or(4))
        .acquire_timeout(Duration::from_secs(10))
        .connect(&url)
        .await
        .expect("connect to Postgres");
    init_db(&db).await.expect("schema migration");
    let app = App { db: db.clone(), api_key: Arc::new(env("UPSCALER_BRIDGE_API_KEY", "")) };

    // Hourly expiry sweep.
    let sweep = db.clone();
    tokio::spawn(async move {
        loop {
            cleanup_once(&sweep).await;
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });

    let router = Router::new()
        .route("/health", get(health))
        .route("/log/upscale", post(log_upscale))
        .route("/log/history", get(get_history).delete(clear_history))
        .route("/log/history/{id}", delete(delete_history_entry))
        .route("/log/action", post(log_action))
        .route("/log/actions", post(log_actions))
        .route("/log/action-history", get(get_action_history))
        .route("/log/snapshot", post(log_snapshot))
        .route("/log/snapshots", get(get_snapshots))
        .route("/log/stats", get(get_stats))
        .route("/log/comparison", post(log_comparison))
        .route("/log/comparison/{cid}/pick", post(log_comparison_pick))
        .route("/log/comparisons", get(get_comparisons))
        .route("/log/overview", get(get_overview))
        .route(
            "/import",
            post(|State(a): State<App>, mp: Multipart| create_stored(a, Store::Imports, mp))
                .get(|State(a): State<App>, Query(q): Query<HashMap<String, String>>| async move { list_stored(&a, Store::Imports, &q).await })
                .delete(|State(a): State<App>, Query(q): Query<HashMap<String, String>>| async move { clear_stored(&a, Store::Imports, &q).await }),
        )
        .route(
            "/import/{id}",
            get(|State(a): State<App>, Path(id): Path<String>| async move { get_stored(&a, Store::Imports, &id).await })
                .delete(|State(a): State<App>, Path(id): Path<String>| async move { delete_stored(&a, Store::Imports, &id).await }),
        )
        .route(
            "/export",
            post(|State(a): State<App>, mp: Multipart| create_stored(a, Store::Exports, mp))
                .get(|State(a): State<App>, Query(q): Query<HashMap<String, String>>| async move { list_stored(&a, Store::Exports, &q).await })
                .delete(|State(a): State<App>, Query(q): Query<HashMap<String, String>>| async move { clear_stored(&a, Store::Exports, &q).await }),
        )
        .route(
            "/export/{id}",
            get(|State(a): State<App>, Path(id): Path<String>| async move { get_stored(&a, Store::Exports, &id).await })
                .delete(|State(a): State<App>, Path(id): Path<String>| async move { delete_stored(&a, Store::Exports, &id).await }),
        )
        .route("/storage/usage", get(storage_usage))
        .route("/presets", post(create_preset).get(list_presets))
        .route("/presets/{id}", delete(delete_preset))
        .route("/device-settings", put(put_device_settings).get(get_device_settings))
        .route("/models", get(list_models))
        .layer(middleware::from_fn_with_state(app.clone(), auth))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES + 1024 * 1024))
        .with_state(app);

    let addr = format!("{}:{}", env("HOST", "127.0.0.1"), env("PORT", "8003"));
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    println!("upscaler-bridge {VERSION} listening on {addr}");
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .unwrap();
}

fn urlenc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
