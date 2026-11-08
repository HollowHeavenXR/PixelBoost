//! Tiled model execution: every source pixel goes through the network.
//!
//! The image is cut into tiles with a context margin; each tile runs through
//! the network with the margin, and only its inner part is kept, so seams
//! see the same neighbourhood a whole-image pass would. Identical tiles
//! (flat backgrounds, letterboxing, UI) run once.

use crate::image::Image;
use crate::models::{self, ModelInfo};
use pb_nn::pbm::{self, Weights};
use pb_nn::{Fmap, SrNetwork};
use std::collections::HashMap;
use std::path::Path;

pub struct Engine {
    net: Box<dyn SrNetwork>,
    pub info: ModelInfo,
}

#[derive(Clone, Copy, Debug)]
pub struct TileOptions {
    /// Inner tile size in source pixels.
    pub tile: usize,
    /// Context margin in source pixels on every side.
    pub pad: usize,
}

impl Default for TileOptions {
    fn default() -> Self {
        TileOptions { tile: 128, pad: 16 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TileEvent<'a> {
    pub done: usize,
    pub total: usize,
    /// Finished tile in output pixels: x, y, w, h.
    pub rect: [usize; 4],
    /// The tile's model output (for live previews).
    pub tile: &'a Image,
}

fn read(dir: &Path, file: &str) -> Result<Weights, String> {
    let p = dir.join(file);
    let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    pbm::read(&bytes)
}

impl Engine {
    /// Load a bundled model by id. `denoise` (0..1) only applies to `fast`:
    /// it blends in the denoising twin weights.
    pub fn load(dir: &Path, id: &str, denoise: Option<f32>) -> Result<Engine, String> {
        let info = *models::find(id).ok_or_else(|| format!("unknown model {id}"))?;
        let mut w = read(dir, info.file)?;
        if let (Some(d), "fast") = (denoise, id) {
            if d > 0.0 {
                let wdn = read(dir, models::find("fast-denoise").unwrap().file)?;
                w = pbm::blend(&wdn, &w, d.clamp(0.0, 1.0))?;
            }
        }
        let net = pb_nn::build(&w)?;
        Ok(Engine { net, info })
    }

    pub fn from_weights(w: &Weights, info: ModelInfo) -> Result<Engine, String> {
        Ok(Engine { net: pb_nn::build(w)?, info })
    }

    pub fn scale(&self) -> usize {
        self.net.scale()
    }

    pub fn macs_per_px(&self) -> f64 {
        self.net.macs_per_px()
    }

    /// Run the network over the whole image (RGB only; alpha is ignored and
    /// returned as 1). Returns `None` if `progress` asked to stop.
    pub fn run(&self, img: &Image, opt: TileOptions, progress: &mut dyn FnMut(TileEvent<'_>) -> bool) -> Option<Image> {
        let s = self.net.scale();
        let a = self.net.align();
        let round = |v: usize| v.div_ceil(a) * a;
        let (w, h) = (round(img.w), round(img.h));
        let tile = round(opt.tile.max(a * 8));
        let pad = round(opt.pad);
        let mut out = Image::new(w * s, h * s);
        let cols = w.div_ceil(tile);
        let rows = h.div_ceil(tile);
        let total = cols * rows;
        let mut cache: HashMap<u64, Image> = HashMap::new();
        let mut done = 0;
        for ty in 0..rows {
            for tx in 0..cols {
                let (ix, iy) = (tx * tile, ty * tile);
                let (iw, ih) = (tile.min(w - ix), tile.min(h - iy));
                let rx = ix.saturating_sub(pad);
                let ry = iy.saturating_sub(pad);
                let rw = (ix + iw + pad).min(w) - rx;
                let rh = (iy + ih + pad).min(h) - ry;
                let mut fm = Fmap::new(rh, rw, 3);
                let mut hasher = Fnv::new();
                hasher.add(&[rx as u64 % a as u64, ry as u64 % a as u64, rw as u64, rh as u64, (ix - rx) as u64, (iy - ry) as u64]);
                for y in 0..rh {
                    for x in 0..rw {
                        let p = img.at_clamped((rx + x) as isize, (ry + y) as isize);
                        let d = fm.px_mut(y, x);
                        d.copy_from_slice(&p[..3]);
                        hasher.add_f(&p[..3]);
                    }
                }
                let key = hasher.0;
                let part = match cache.get(&key) {
                    Some(p) => p.clone(),
                    None => {
                        let o = self.net.forward(&fm);
                        let mut part = Image::new(iw * s, ih * s);
                        let (ox, oy) = ((ix - rx) * s, (iy - ry) * s);
                        for y in 0..ih * s {
                            for x in 0..iw * s {
                                let v = o.px(oy + y, ox + x);
                                part.px[y * iw * s + x] = [v[0], v[1], v[2], 1.0];
                            }
                        }
                        cache.insert(key, part.clone());
                        part
                    }
                };
                out.paste(&part, ix * s, iy * s);
                done += 1;
                if !progress(TileEvent { done, total, rect: [ix * s, iy * s, iw * s, ih * s], tile: &part }) {
                    return None;
                }
            }
        }
        if out.w != img.w * s || out.h != img.h * s {
            out = out.crop(0, 0, img.w * s, img.h * s);
        }
        Some(out)
    }
}

/// FNV-1a over tile contents (exact float bits) — identical tiles only.
struct Fnv(u64);
impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn add(&mut self, v: &[u64]) {
        for x in v {
            for b in x.to_le_bytes() {
                self.0 ^= b as u64;
                self.0 = self.0.wrapping_mul(0x100_0000_01b3);
            }
        }
    }
    fn add_f(&mut self, v: &[f32]) {
        for x in v {
            self.0 ^= x.to_bits() as u64;
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }
}
