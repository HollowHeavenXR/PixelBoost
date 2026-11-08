//! Picks a model for "Auto" from simple, fast image statistics.
//!
//! Drawn art (anime, cel shading, line art) has large flat fills separated
//! by hard edges and almost no sensor-like noise; photos and rendered 3D
//! scenes have texture almost everywhere.

use crate::image::Image;
use crate::resample::{resize, Filter};
use serde::Serialize;

#[derive(Serialize, Debug, Clone, Copy)]
pub struct Stats {
    /// Share of pixels whose 3x3 neighbourhood is essentially one colour.
    pub flat: f32,
    /// Share of pixels on a hard edge.
    pub edges: f32,
    /// Median absolute Laplacian on non-edge pixels (noise / fine texture).
    pub texture: f32,
}

pub fn stats(img: &Image) -> Stats {
    let f = 512.0 / img.w.max(img.h) as f32;
    let small = if f < 1.0 { resize(img, ((img.w as f32 * f) as usize).max(8), ((img.h as f32 * f) as usize).max(8), Filter::Box) } else { img.clone() };
    let (w, h) = (small.w, small.h);
    let l: Vec<f32> = small.px.iter().map(|p| Image::luma(*p)).collect();
    let (mut flat, mut edges, mut n) = (0usize, 0usize, 0usize);
    let mut tex = Vec::new();
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let c = l[y * w + x];
            let mut lo = c;
            let mut hi = c;
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let v = l[((y as isize + dy) as usize) * w + (x as isize + dx) as usize];
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            let range = hi - lo;
            n += 1;
            if range < 0.02 {
                flat += 1;
            }
            if range > 0.25 {
                edges += 1;
            } else {
                let lap = 4.0 * c - l[y * w + x - 1] - l[y * w + x + 1] - l[(y - 1) * w + x] - l[(y + 1) * w + x];
                tex.push(lap.abs());
            }
        }
    }
    tex.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = n.max(1) as f32;
    Stats { flat: flat as f32 / n, edges: edges as f32 / n, texture: tex.get(tex.len() / 2).copied().unwrap_or(0.0) }
}

/// Content type only: "anime" (drawn) or "photo" (photo / render).
pub fn content_kind(img: &Image) -> &'static str {
    let s = stats(img);
    if (s.flat > 0.35 && s.texture < 0.013) || s.flat > 0.55 {
        "anime"
    } else {
        "photo"
    }
}

/// Model for Auto: the best model for the content whose cost fits a
/// compute budget (phones run these on the CPU). The heavy 23-block photo
/// network is used up to ~1 MP; above that the compact general model,
/// which scores as well on the bench at a fifteenth of the cost.
pub fn pick_model(img: &Image) -> &'static str {
    let mp = (img.w * img.h) as f32 / 1e6;
    match content_kind(img) {
        "anime" if mp <= 4.0 => "anime",
        "anime" => "lineart",
        _ if mp <= 1.0 => "photo",
        _ => "fast",
    }
}
