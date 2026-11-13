//! Smart settings, gallery replacement and shape crops for the app.

use crate::{PbError, PixelBoostCore, UpscaleSettings};
use pb_core::analyze::{self, Budget};
use pb_core::{meta, shape};

#[derive(uniffi::Record, Clone)]
pub struct PhotoAnalysis {
    /// "anime" or "photo".
    pub kind: String,
    pub width: u32,
    pub height: u32,
    /// Noise sigma in 8-bit levels.
    pub noise: f32,
    /// JPEG blocking 0..1.
    pub blocking: f32,
    /// Pixel-scale sharpness (~0.45 soft .. 0.85+ crisp).
    pub sharpness: f32,
    /// Skin-toned share of the middle of the frame.
    pub skin: f32,
    pub has_alpha: bool,
    /// The settings chosen for this photo.
    pub settings: UpscaleSettings,
    pub est_seconds: f32,
    pub summary: String,
    pub reasons: Vec<String>,
}

#[derive(uniffi::Record, Clone)]
pub struct ShapeEntry {
    pub id: String,
    pub name: String,
    /// Stretches to the crop box (else keeps its proportions: lock 1:1).
    pub stretches: bool,
}

#[derive(uniffi::Record, Clone)]
pub struct Replacement {
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// "jpg" or "png".
    pub format: String,
    pub had_exif: bool,
    pub had_profile: bool,
}

#[uniffi::export]
impl PixelBoostCore {
    /// Look at the photo and choose every upscale setting for it.
    /// `device_gflops`: measured phone speed; `max_seconds`: longest run the
    /// user accepts on the phone; `cloud`: plan for the server instead.
    pub fn analyze(&self, input_path: String, device_gflops: f32, max_seconds: f32, max_output_mp: f32, cloud: bool) -> Result<PhotoAnalysis, PbError> {
        let m = analyze::measure_file(&input_path)?;
        let budget = Budget { gflops: device_gflops.max(1.0), max_seconds: max_seconds.max(5.0), max_output_mp, cloud };
        let r = analyze::recommend(&m, budget);
        Ok(PhotoAnalysis {
            kind: m.kind.into(),
            width: m.width as u32,
            height: m.height as u32,
            noise: m.noise,
            blocking: m.blocking,
            sharpness: m.sharpness,
            skin: m.skin,
            has_alpha: m.has_alpha,
            settings: UpscaleSettings {
                model: r.model,
                scale: r.scale,
                fidelity: r.fidelity,
                detail: r.detail,
                denoise: r.denoise,
                ensemble: r.ensemble as u32,
                tile: 128,
                max_output_mp,
            },
            est_seconds: r.est_seconds,
            summary: r.summary,
            reasons: r.reasons,
        })
    }

    /// Build the file that replaces a photo in the library: the upscaled
    /// pixels with the original's EXIF (date, camera, place; orientation
    /// reset, size updated) and colour profile. JPEG unless the result has
    /// transparency and `allow_png`. `output_base` gets the extension.
    pub fn prepare_replacement(&self, original_path: String, result_path: String, output_base: String, quality: u8, allow_png: bool) -> Result<Replacement, PbError> {
        let bytes = std::fs::read(&original_path).map_err(|e| PbError::from(format!("{original_path}: {e}")))?;
        let m = meta::read(&bytes);
        drop(bytes);
        // Write to a temp name, then rename with the right extension.
        let tmp = format!("{output_base}.tmp");
        let (w, h, ext) = meta::write_with_meta(&result_path, &tmp, &m, quality, allow_png)?;
        let path = format!("{output_base}.{ext}");
        std::fs::rename(&tmp, &path).map_err(|e| PbError::from(e.to_string()))?;
        Ok(Replacement { path, width: w, height: h, format: ext.into(), had_exif: m.exif.is_some(), had_profile: m.icc.is_some() })
    }

    /// Adjustment data stored with an in-place edit, so Photos (and this
    /// app) can tell what was done. JSON, small.
    pub fn replacement_note(&self, model: String, scale: f32, in_width: u32, in_height: u32) -> String {
        serde_json::json!({ "app": "PixelBoost", "op": "upscale", "model": model, "scale": scale, "from": [in_width, in_height] }).to_string()
    }

    pub fn shapes(&self) -> Vec<ShapeEntry> {
        shape::SHAPES.iter().map(|(id, name)| ShapeEntry { id: (*id).into(), name: (*name).into(), stretches: shape::stretches(id) }).collect()
    }

    pub fn ring_styles(&self) -> Vec<ShapeEntry> {
        shape::RING_STYLES.iter().map(|(id, name)| ShapeEntry { id: (*id).into(), name: (*name).into(), stretches: false }).collect()
    }

    /// Outline of `shape` in a `width` x `height` (pixel) box, `offset`
    /// pixels outside the edge (negative = inside), as x,y pairs normalised
    /// to the box. For drawing the crop overlay and ring previews.
    #[allow(clippy::too_many_arguments)]
    pub fn shape_outline(&self, shape: String, width: f32, height: f32, rotation: f32, param: f32, offset: f32, points: u32) -> Result<Vec<f32>, PbError> {
        let pts = shape::outline(&shape, width.max(4.0), height.max(4.0), rotation, param, offset, points as usize)?;
        Ok(pts.into_iter().flatten().collect())
    }
}
