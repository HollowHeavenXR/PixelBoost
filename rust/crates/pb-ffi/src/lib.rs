//! PixelBoostCore — the Rust engine as a Swift module (UniFFI).
//!
//! Images cross the boundary as files (full-size work) or raw RGBA8 buffers
//! (previews), never as UIImage/CGImage, so Swift does no pixel work.

use pb_core::fidelity::Fidelity;
use pb_core::image::Format;
use pb_core::pipeline::Settings;
use pb_core::{classify, models, ops, Engine, Image};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub mod editor;
pub mod history;
pub mod server;

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum PbError {
    #[error("{message}")]
    Failed { message: String },
    #[error("cancelled")]
    Cancelled,
}

impl From<String> for PbError {
    fn from(message: String) -> Self {
        PbError::Failed { message }
    }
}

#[derive(uniffi::Record, Clone)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    pub best_for: String,
    pub scale: u32,
    pub cost: f32,
}

#[derive(uniffi::Record, Clone)]
pub struct UpscaleSettings {
    /// A model id, or "auto".
    pub model: String,
    pub scale: f32,
    /// "off", "natural" or "faithful".
    pub fidelity: String,
    pub detail: f32,
    /// 0..1, only used by the "fast" model.
    pub denoise: f32,
    /// 1, 2, 4 or 8.
    pub ensemble: u32,
    pub tile: u32,
    /// Output cap in megapixels (0 = default 120).
    pub max_output_mp: f32,
}

#[derive(uniffi::Record, Clone)]
pub struct UpscaleOutcome {
    pub output_path: String,
    pub width: u32,
    pub height: u32,
    pub model: String,
    pub seconds: f64,
    pub fidelity_db: f64,
    pub tiles: u32,
    pub passes: u32,
    /// The output was made smaller than asked to fit the size cap.
    pub capped: bool,
}

#[derive(uniffi::Record, Clone)]
pub struct RunPlan {
    pub model: String,
    pub out_width: u32,
    pub out_height: u32,
    pub tiles: u32,
    pub gflop: f64,
}

#[derive(uniffi::Record, Clone)]
pub struct ImageInfo {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub has_alpha: bool,
}

#[derive(uniffi::Record, Clone)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Receives live progress. `rgba` is the finished tile's pixels (straight
/// RGBA8, `pixel_w` x `pixel_h`) to draw into the rectangle (`x`,`y`,`w`,`h`)
/// of the final output; empty when there is nothing to draw.
#[uniffi::export(with_foreign)]
pub trait UpscaleListener: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    fn on_tile(&self, done: u32, total: u32, x: u32, y: u32, w: u32, h: u32, pixel_w: u32, pixel_h: u32, rgba: Vec<u8>);
}

/// Cancellation handle shared between Swift and a running job.
#[derive(uniffi::Object, Default)]
pub struct CancelToken {
    flag: AtomicBool,
}

#[uniffi::export]
impl CancelToken {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

#[derive(uniffi::Object)]
pub struct PixelBoostCore {
    models_dir: PathBuf,
    /// Loaded networks, keyed by model id (+ denoise level), kept warm
    /// between runs.
    cache: Mutex<HashMap<String, Arc<Engine>>>,
}

fn fmt_for(path: &str) -> Format {
    Format::from_path(path)
}

fn to_rgba(img: &Image) -> RgbaImage {
    RgbaImage { width: img.w as u32, height: img.h as u32, rgba: img.to_rgba8() }
}

pub(crate) fn from_rgba(r: &RgbaImage) -> Result<Image, PbError> {
    let (w, h) = (r.width as usize, r.height as usize);
    if r.rgba.len() != w * h * 4 {
        return Err(PbError::from(format!("rgba buffer is {} bytes, expected {}", r.rgba.len(), w * h * 4)));
    }
    let px = r.rgba.chunks_exact(4).map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]).collect();
    Ok(Image { w, h, px })
}

#[uniffi::export]
impl PixelBoostCore {
    /// `models_dir` holds the bundled `.pbnn` files.
    #[uniffi::constructor]
    pub fn new(models_dir: String) -> Arc<Self> {
        Arc::new(PixelBoostCore { models_dir: PathBuf::from(models_dir), cache: Mutex::new(HashMap::new()) })
    }

    pub fn models(&self) -> Vec<ModelEntry> {
        models::MODELS
            .iter()
            .filter(|m| m.id != "fast-denoise")
            .map(|m| ModelEntry { id: m.id.into(), name: m.name.into(), best_for: m.best_for.into(), scale: m.scale, cost: m.cost })
            .collect()
    }

    pub fn kernel(&self) -> String {
        pb_nn::conv::kernel_name().into()
    }

    /// Which model Auto would use for this image.
    pub fn auto_model(&self, input_path: String) -> Result<String, PbError> {
        let img = Image::open(&input_path)?;
        Ok(classify::pick_model(&img).into())
    }

    pub fn image_info(&self, path: String) -> Result<ImageInfo, PbError> {
        let img = Image::open(&path)?;
        Ok(ImageInfo { width: img.w as u32, height: img.h as u32, has_alpha: img.has_alpha(), path })
    }

    /// Output size, tile count and compute cost before running.
    pub fn plan(&self, width: u32, height: u32, model: String, scale: f32, tile: u32) -> RunPlan {
        let m = models::find(&model).copied().unwrap_or(models::MODELS[0]);
        let t = tile.max(32);
        let tiles = width.div_ceil(t) * height.div_ceil(t);
        // RRDB-23 is ~18.4M MACs per input pixel at full cost.
        let gflop = 2.0 * 18.4e6 * m.cost as f64 * (width as f64) * (height as f64) / 1e9;
        RunPlan {
            model: m.id.into(),
            out_width: (width as f32 * scale).round() as u32,
            out_height: (height as f32 * scale).round() as u32,
            tiles,
            gflop,
        }
    }

    pub fn upscale(&self, input_path: String, output_path: String, settings: UpscaleSettings, listener: Arc<dyn UpscaleListener>, cancel: Arc<CancelToken>) -> Result<UpscaleOutcome, PbError> {
        let img = Image::open(&input_path)?;
        let model = if settings.model == "auto" { classify::pick_model(&img).to_string() } else { settings.model.clone() };
        let denoise = if model == "fast" && settings.denoise > 0.0 { Some(settings.denoise) } else { None };
        let key = format!("{model}:{:.2}", denoise.unwrap_or(0.0));
        let engine = {
            let mut c = self.cache.lock().unwrap();
            if let Some(e) = c.get(&key) {
                e.clone()
            } else {
                // Keep at most two networks resident (they are large).
                if c.len() >= 2 {
                    c.clear();
                }
                let e = Arc::new(Engine::load(&self.models_dir, &model, denoise)?);
                c.insert(key, e.clone());
                e
            }
        };
        let st = Settings {
            scale: settings.scale,
            fidelity: Fidelity::parse(&settings.fidelity).unwrap_or(Fidelity::Natural),
            detail: settings.detail.clamp(0.0, 1.0),
            tile: settings.tile.clamp(32, 512) as usize,
            pad: 16,
            ensemble: settings.ensemble.max(1) as usize,
        };
        let lim = pb_core::stream::Limits { max_output_px: if settings.max_output_mp > 0.0 { (settings.max_output_mp as f64 * 1e6) as usize } else { 120_000_000 } };
        let res = pb_core::stream::upscale(&engine, &img, &st, lim, &mut |e| {
            let [x, y, w, h] = e.rect;
            let rgba = if w > 0 { e.pixels.data.clone() } else { Vec::new() };
            let (pw, ph) = (e.pixels.w as u32, e.pixels.h as u32);
            // `rect` is where the tile lands in the final output; the pixels
            // may be at a different (first-pass) resolution.
            listener.on_tile(e.done as u32, e.total as u32, x as u32, y as u32, w as u32, h as u32, pw, ph, rgba);
            !cancel.is_cancelled()
        });
        drop(img);
        let (out, rep, capped) = res.ok_or(PbError::Cancelled)?;
        out.save(&output_path, 95)?;
        Ok(UpscaleOutcome {
            output_path,
            width: out.w as u32,
            height: out.h as u32,
            model: rep.model,
            seconds: rep.seconds,
            fidelity_db: rep.fidelity_db,
            tiles: rep.tiles as u32,
            passes: rep.passes as u32,
            capped,
        })
    }

    /// Run one editing operation (JSON, see pb_core::ops::Op) file to file.
    pub fn edit_file(&self, input_path: String, output_path: String, op_json: String) -> Result<ImageInfo, PbError> {
        let img = Image::open(&input_path)?;
        let op = ops::parse(&op_json)?;
        let out = ops::apply(&img, &op)?;
        let bytes = out.encode(fmt_for(&output_path), 95)?;
        std::fs::write(&output_path, bytes).map_err(|e| PbError::from(e.to_string()))?;
        Ok(ImageInfo { path: output_path, width: out.w as u32, height: out.h as u32, has_alpha: out.has_alpha() })
    }

    /// Same, on an in-memory RGBA8 buffer (live previews).
    pub fn edit_rgba(&self, image: RgbaImage, op_json: String) -> Result<RgbaImage, PbError> {
        let img = from_rgba(&image)?;
        let op = ops::parse(&op_json)?;
        Ok(to_rgba(&ops::apply(&img, &op)?))
    }

    /// Decode any supported file to RGBA8, shrunk to fit `max_side`.
    pub fn load_preview(&self, path: String, max_side: u32) -> Result<RgbaImage, PbError> {
        let img = Image::open(&path)?;
        let m = max_side.max(16) as usize;
        let f = m as f32 / img.w.max(img.h) as f32;
        let img = if f < 1.0 { pb_core::resample::resize(&img, ((img.w as f32 * f).round() as usize).max(1), ((img.h as f32 * f).round() as usize).max(1), pb_core::resample::Filter::Lanczos3) } else { img };
        Ok(to_rgba(&img))
    }

    pub fn save_rgba(&self, image: RgbaImage, path: String, quality: u8) -> Result<(), PbError> {
        let img = from_rgba(&image)?;
        let bytes = img.encode(fmt_for(&path), quality)?;
        std::fs::write(&path, bytes).map_err(|e| PbError::from(e.to_string()))
    }

    /// Source-vs-result agreement (dB) for any two files.
    pub fn fidelity(&self, source_path: String, result_path: String) -> Result<f64, PbError> {
        let a = Image::open(&source_path)?;
        let b = Image::open(&result_path)?;
        let s = (b.w / a.w).max(1);
        if b.w != a.w * s || b.h != a.h * s {
            return Err(PbError::from("result is not an integer multiple of the source".to_string()));
        }
        Ok(pb_core::fidelity::fidelity_db(&a, &b, s))
    }

    /// Names of the filter looks.
    pub fn looks(&self) -> Vec<String> {
        pb_core::edit::LOOKS.iter().map(|s| s.to_string()).collect()
    }
}
