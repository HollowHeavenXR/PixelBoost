//! On-device upscale history: a JSON index plus small JPEG thumbnails in
//! the app's documents folder.

use crate::PbError;
use pb_core::image::Format;
use pb_core::resample::{resize, Filter};
use pb_core::Image;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(uniffi::Record, Serialize, Deserialize, Clone)]
pub struct HistoryEntry {
    pub id: String,
    /// Seconds since 1970.
    pub created_at: f64,
    pub model: String,
    pub scale: f32,
    pub in_width: u32,
    pub in_height: u32,
    pub out_width: u32,
    pub out_height: u32,
    pub seconds: f64,
    pub fidelity_db: f64,
    pub result_path: String,
    pub thumb_path: String,
}

#[derive(uniffi::Object)]
pub struct HistoryStore {
    dir: PathBuf,
    entries: Mutex<Vec<HistoryEntry>>,
}

impl HistoryStore {
    fn save(&self, e: &[HistoryEntry]) -> Result<(), PbError> {
        let tmp = self.dir.join("history.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(e).unwrap()).map_err(|e| PbError::Failed { message: e.to_string() })?;
        std::fs::rename(&tmp, self.dir.join("history.json")).map_err(|e| PbError::Failed { message: e.to_string() })
    }
}

#[uniffi::export]
impl HistoryStore {
    #[uniffi::constructor]
    pub fn new(dir: String) -> Arc<Self> {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).ok();
        let entries = std::fs::read(dir.join("history.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Arc::new(HistoryStore { dir, entries: Mutex::new(entries) })
    }

    pub fn list(&self) -> Vec<HistoryEntry> {
        let mut v = self.entries.lock().unwrap().clone();
        v.sort_by(|a, b| b.created_at.partial_cmp(&a.created_at).unwrap());
        v
    }

    /// Record a result: copies it into the history folder and makes a
    /// 320px thumbnail. Fills in id, paths and timestamp.
    pub fn add(&self, mut entry: HistoryEntry, result_file: String) -> Result<HistoryEntry, PbError> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
        entry.id = format!("{:x}", now.as_nanos());
        entry.created_at = now.as_secs_f64();
        let ext = std::path::Path::new(&result_file).extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "png".into());
        let dest = self.dir.join(format!("{}.{ext}", entry.id));
        std::fs::copy(&result_file, &dest).map_err(|e| PbError::Failed { message: e.to_string() })?;
        let img = Image::open(&result_file)?;
        let f = 320.0 / img.w.max(img.h) as f32;
        let t = if f < 1.0 { resize(&img, ((img.w as f32 * f) as usize).max(1), ((img.h as f32 * f) as usize).max(1), Filter::Lanczos3) } else { img };
        let thumb = self.dir.join(format!("{}_t.jpg", entry.id));
        std::fs::write(&thumb, t.encode(Format::Jpeg, 85)?).map_err(|e| PbError::Failed { message: e.to_string() })?;
        entry.result_path = dest.to_string_lossy().into();
        entry.thumb_path = thumb.to_string_lossy().into();
        let mut v = self.entries.lock().unwrap();
        v.push(entry.clone());
        // Keep the newest 200.
        if v.len() > 200 {
            v.sort_by(|a, b| b.created_at.partial_cmp(&a.created_at).unwrap());
            for old in v.drain(200..) {
                std::fs::remove_file(&old.result_path).ok();
                std::fs::remove_file(&old.thumb_path).ok();
            }
        }
        self.save(&v)?;
        Ok(entry)
    }

    pub fn delete(&self, id: String) -> Result<(), PbError> {
        let mut v = self.entries.lock().unwrap();
        if let Some(i) = v.iter().position(|e| e.id == id) {
            let e = v.remove(i);
            std::fs::remove_file(&e.result_path).ok();
            std::fs::remove_file(&e.thumb_path).ok();
        }
        self.save(&v)
    }

    pub fn clear(&self) -> Result<(), PbError> {
        let mut v = self.entries.lock().unwrap();
        for e in v.drain(..) {
            std::fs::remove_file(&e.result_path).ok();
            std::fs::remove_file(&e.thumb_path).ok();
        }
        self.save(&v)
    }

    /// Bytes used by stored results and thumbnails.
    pub fn disk_bytes(&self) -> u64 {
        self.entries.lock().unwrap().iter().map(|e| std::fs::metadata(&e.result_path).map(|m| m.len()).unwrap_or(0) + std::fs::metadata(&e.thumb_path).map(|m| m.len()).unwrap_or(0)).sum()
    }
}
