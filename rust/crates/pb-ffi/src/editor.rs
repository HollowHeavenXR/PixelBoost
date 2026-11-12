//! Photo-editor API for the app: fonts, effects, presets, histogram, colour
//! pickers, export. Pixel work stays here; Swift only shows the results.

use crate::{from_rgba, PbError, PixelBoostCore, ImageInfo, RgbaImage};
use pb_core::image::Format;
use pb_core::resample::{resize, Filter};
use pb_core::{ops, Image};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(uniffi::Record, Clone)]
pub struct FontEntry {
    pub id: String,
    pub name: String,
}

#[derive(uniffi::Record, Clone)]
pub struct WhiteBalance {
    pub warmth: f32,
    pub tint: f32,
}

#[derive(uniffi::Record, Clone)]
pub struct OpInfo {
    /// Can be saved as a preset / pasted onto another photo.
    pub portable: bool,
    pub changes_geometry: bool,
}

/// 256 bins each for red, green, blue and luma (1024 values), plus the
/// fraction of clipped shadows / highlights.
#[derive(uniffi::Record, Clone)]
pub struct Histogram {
    pub bins: Vec<u32>,
    pub peak: u32,
    pub clipped_shadows: f32,
    pub clipped_highlights: f32,
}

fn avg_at(img: &RgbaImage, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = (img.width as i64, img.height as i64);
    let cx = (x.clamp(0.0, 1.0) * (w - 1) as f32).round() as i64;
    let cy = (y.clamp(0.0, 1.0) * (h - 1) as f32).round() as i64;
    let mut acc = [0.0f32; 3];
    let mut n = 0.0f32;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let (xx, yy) = (cx + dx, cy + dy);
            if xx < 0 || yy < 0 || xx >= w || yy >= h {
                continue;
            }
            let i = ((yy * w + xx) * 4) as usize;
            for c in 0..3 {
                acc[c] += img.rgba[i + c] as f32 / 255.0;
            }
            n += 1.0;
        }
    }
    acc.map(|v| v / n.max(1.0))
}

#[uniffi::export]
impl PixelBoostCore {
    pub fn fonts(&self) -> Vec<FontEntry> {
        pb_core::text::FONTS.iter().map(|f| FontEntry { id: f.id.into(), name: f.name.into() }).collect()
    }

    pub fn effects(&self) -> Vec<String> {
        pb_core::fx::EFFECTS.iter().map(|s| s.to_string()).collect()
    }

    pub fn blend_modes(&self) -> Vec<String> {
        pb_core::compose::BLEND_MODES.iter().map(|s| s.to_string()).collect()
    }

    pub fn op_info(&self, op_json: String) -> Result<OpInfo, PbError> {
        let op = ops::parse(&op_json)?;
        Ok(OpInfo { portable: !op.uses_files(), changes_geometry: op.changes_geometry() })
    }

    pub fn histogram(&self, image: RgbaImage) -> Histogram {
        let mut bins = vec![0u32; 1024];
        let px = image.rgba.chunks_exact(4);
        let total = (image.rgba.len() / 4).max(1) as f32;
        let (mut lo, mut hi) = (0u32, 0u32);
        for c in px {
            if c[3] == 0 {
                continue;
            }
            bins[c[0] as usize] += 1;
            bins[256 + c[1] as usize] += 1;
            bins[512 + c[2] as usize] += 1;
            let l = ((2126 * c[0] as u32 + 7152 * c[1] as u32 + 722 * c[2] as u32) / 10000).min(255);
            bins[768 + l as usize] += 1;
            if c[0] <= 1 && c[1] <= 1 && c[2] <= 1 {
                lo += 1;
            }
            if c[0] >= 254 || c[1] >= 254 || c[2] >= 254 {
                hi += 1;
            }
        }
        // Peak ignoring the end bins so clipping doesn't flatten the graph.
        let peak = (0..4).flat_map(|ch| bins[ch * 256 + 1..ch * 256 + 255].iter().copied()).max().unwrap_or(1).max(1);
        Histogram { bins, peak, clipped_shadows: lo as f32 / total, clipped_highlights: hi as f32 / total }
    }

    /// Warmth/tint that make the tapped spot (normalised x, y) neutral.
    pub fn pick_balance(&self, image: RgbaImage, x: f32, y: f32) -> WhiteBalance {
        let (warmth, tint) = pb_core::develop::neutral_balance(avg_at(&image, x, y));
        WhiteBalance { warmth, tint }
    }

    /// Average colour (0..1 RGB) around a normalised point.
    pub fn pick_color(&self, image: RgbaImage, x: f32, y: f32) -> Vec<f32> {
        avg_at(&image, x, y).to_vec()
    }

    /// Apply an op to a preview buffer at a smaller size (preset thumbnails).
    pub fn edit_thumbnail(&self, image: RgbaImage, op_json: String, max_side: u32) -> Result<RgbaImage, PbError> {
        let img = from_rgba(&image)?;
        let m = max_side.max(16) as usize;
        let f = m as f32 / img.w.max(img.h) as f32;
        let small = if f < 1.0 { resize(&img, ((img.w as f32 * f).round() as usize).max(1), ((img.h as f32 * f).round() as usize).max(1), Filter::Triangle) } else { img };
        let out = ops::apply(&small, &ops::parse(&op_json)?)?;
        Ok(RgbaImage { width: out.w as u32, height: out.h as u32, rgba: out.to_rgba8() })
    }

    /// Final export: optional long-side limit, format from the extension
    /// (jpg / png / webp / tiff), JPEG quality 1..100.
    pub fn export_file(&self, input_path: String, output_path: String, quality: u8, max_side: u32) -> Result<ImageInfo, PbError> {
        let mut img = Image::open(&input_path)?;
        if max_side > 0 && img.w.max(img.h) > max_side as usize {
            let f = max_side as f32 / img.w.max(img.h) as f32;
            img = resize(&img, ((img.w as f32 * f).round() as usize).max(1), ((img.h as f32 * f).round() as usize).max(1), Filter::Lanczos3);
        }
        let bytes = img.encode(Format::from_path(&output_path), quality.clamp(1, 100))?;
        std::fs::write(&output_path, bytes).map_err(|e| PbError::from(e.to_string()))?;
        Ok(ImageInfo { width: img.w as u32, height: img.h as u32, has_alpha: img.has_alpha(), path: output_path })
    }
}

// ------------------------------------------------------------ presets

#[derive(uniffi::Record, Serialize, Deserialize, Clone)]
pub struct PresetEntry {
    pub id: String,
    pub name: String,
    pub op_json: String,
    pub builtin: bool,
    pub created_at: f64,
}

#[derive(Serialize, Deserialize, Default)]
struct PresetFile {
    presets: Vec<PresetEntry>,
}

/// Built-in presets plus the user's own, stored as JSON in the app's
/// documents folder.
#[derive(uniffi::Object)]
pub struct PresetStore {
    file: PathBuf,
    user: Mutex<Vec<PresetEntry>>,
}

impl PresetStore {
    fn persist(&self, list: &[PresetEntry]) -> Result<(), PbError> {
        let tmp = self.file.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(&PresetFile { presets: list.to_vec() }).map_err(|e| PbError::from(e.to_string()))?;
        std::fs::write(&tmp, data).map_err(|e| PbError::from(e.to_string()))?;
        std::fs::rename(&tmp, &self.file).map_err(|e| PbError::from(e.to_string()))
    }
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

#[uniffi::export]
impl PresetStore {
    #[uniffi::constructor]
    pub fn new(dir: String) -> Arc<Self> {
        let d = PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&d);
        let file = d.join("presets.json");
        let user = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<PresetFile>(&b).ok()).map(|f| f.presets).unwrap_or_default();
        Arc::new(PresetStore { file, user: Mutex::new(user) })
    }

    /// User presets first (newest first), then the built-ins.
    pub fn list(&self) -> Vec<PresetEntry> {
        let mut out: Vec<PresetEntry> = self.user.lock().unwrap().iter().rev().cloned().collect();
        out.extend(pb_core::presets::BUILTIN.iter().map(|b| PresetEntry { id: b.id.into(), name: b.name.into(), op_json: b.op.into(), builtin: true, created_at: 0.0 }));
        out
    }

    /// Save a recipe. Ops that read files (painted masks, overlays, LUTs)
    /// are dropped from a chain since they can't move to another photo.
    pub fn save(&self, name: String, op_json: String) -> Result<PresetEntry, PbError> {
        let op = ops::parse(&op_json)?;
        let json = portable_json(&op_json, &op)?;
        let mut list = self.user.lock().unwrap();
        let e = PresetEntry { id: format!("u{}", (now() * 1000.0) as u64), name: name.trim().chars().take(40).collect(), op_json: json, builtin: false, created_at: now() };
        list.push(e.clone());
        self.persist(&list)?;
        Ok(e)
    }

    pub fn rename(&self, id: String, name: String) -> Result<(), PbError> {
        let mut list = self.user.lock().unwrap();
        if let Some(e) = list.iter_mut().find(|e| e.id == id) {
            e.name = name.trim().chars().take(40).collect();
        }
        self.persist(&list)
    }

    pub fn delete(&self, id: String) -> Result<(), PbError> {
        let mut list = self.user.lock().unwrap();
        list.retain(|e| e.id != id);
        self.persist(&list)
    }
}

/// Keep only the portable parts of a recipe.
fn portable_json(raw: &str, op: &ops::Op) -> Result<String, PbError> {
    if !op.uses_files() {
        return Ok(raw.to_string());
    }
    let v: serde_json::Value = serde_json::from_str(raw).map_err(|e| PbError::from(e.to_string()))?;
    if v.get("op").and_then(|o| o.as_str()) == Some("chain") {
        let kept: Vec<serde_json::Value> = v
            .get("ops")
            .and_then(|o| o.as_array())
            .map(|a| a.iter().filter(|o| ops::parse(&o.to_string()).map(|p| !p.uses_files()).unwrap_or(false)).cloned().collect())
            .unwrap_or_default();
        if kept.is_empty() {
            return Err(PbError::from("Nothing in these edits can be saved as a preset (they use painted masks or files).".to_string()));
        }
        return Ok(serde_json::json!({"op": "chain", "ops": kept}).to_string());
    }
    Err(PbError::from("This edit uses a painted mask or a file, so it can't be saved as a preset.".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presets_round_trip_and_strip_files() {
        let dir = std::env::temp_dir().join(format!("pb-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = PresetStore::new(dir.to_string_lossy().into());
        let n0 = s.list().len();
        let e = s.save("Mine".into(), r#"{"op":"chain","ops":[{"op":"adjust","exposure":0.3},{"op":"inpaint","mask":"/x.png"}]}"#.into()).unwrap();
        assert!(!e.op_json.contains("inpaint"));
        assert!(s.save("Bad".into(), r#"{"op":"inpaint","mask":"/x.png"}"#.into()).is_err());
        let s2 = PresetStore::new(dir.to_string_lossy().into());
        assert_eq!(s2.list().len(), n0 + 1);
        assert_eq!(s2.list()[0].name, "Mine");
        s2.delete(e.id).unwrap();
        assert_eq!(s2.list().len(), n0);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn histogram_counts() {
        let core = PixelBoostCore::new("/nonexistent".into());
        let img = RgbaImage { width: 2, height: 1, rgba: vec![0, 0, 0, 255, 255, 255, 255, 255] };
        let h = core.histogram(img);
        assert_eq!(h.bins[0], 1);
        assert_eq!(h.bins[255], 1);
        assert!((h.clipped_highlights - 0.5).abs() < 1e-6);
    }
}
