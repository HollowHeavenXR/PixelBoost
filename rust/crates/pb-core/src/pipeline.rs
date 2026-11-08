//! The full upscale: model -> fidelity -> detail -> alpha -> output size.

use crate::engine::{Engine, TileEvent, TileOptions};
use crate::enhance::sharpen_detail;
use crate::fidelity::{fidelity_db, refine, Fidelity};
use crate::image::Image;
use crate::resample::{resize, Filter};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    /// Output size relative to the input (1..=16). Above the model's scale
    /// the model runs again on its own output.
    pub scale: f32,
    pub fidelity: Fidelity,
    /// Halo-free detail boost, 0..1.
    pub detail: f32,
    pub tile: usize,
    pub pad: usize,
    /// Self-ensemble: average the model over 1, 2, 4 or 8 flipped/rotated
    /// copies. Cleaner, steadier detail at a proportional time cost.
    #[serde(default = "one")]
    pub ensemble: usize,
}

fn one() -> usize {
    1
}

impl Default for Settings {
    fn default() -> Self {
        Settings { scale: 4.0, fidelity: Fidelity::Natural, detail: 0.35, tile: 128, pad: 16, ensemble: 1 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Report {
    pub model: String,
    pub in_w: usize,
    pub in_h: usize,
    pub out_w: usize,
    pub out_h: usize,
    pub passes: usize,
    pub tiles: usize,
    pub seconds: f64,
    /// Source vs result shrunk back (dB). Higher = more faithful.
    pub fidelity_db: f64,
    pub had_alpha: bool,
}

/// Give fully transparent pixels the colour of their nearest visible
/// neighbours so the model never sees the arbitrary colour stored under
/// transparency (which would bleed into edges).
pub(crate) fn bleed_transparent(img: &Image) -> Image {
    let mut out = img.clone();
    let (w, h) = (img.w, img.h);
    let mut known: Vec<bool> = img.px.iter().map(|p| p[3] > 0.004).collect();
    if known.iter().all(|k| *k) || !known.iter().any(|k| *k) {
        return out;
    }
    for _ in 0..64 {
        let mut changed = false;
        let prev = out.px.clone();
        let prev_known = known.clone();
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if prev_known[i] {
                    continue;
                }
                let mut acc = [0.0f32; 3];
                let mut n = 0.0;
                for (dx, dy) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                    let (xx, yy) = (x as isize + dx, y as isize + dy);
                    if xx < 0 || yy < 0 || xx >= w as isize || yy >= h as isize {
                        continue;
                    }
                    let j = yy as usize * w + xx as usize;
                    if prev_known[j] {
                        for c in 0..3 {
                            acc[c] += prev[j][c];
                        }
                        n += 1.0;
                    }
                }
                if n > 0.0 {
                    out.px[i] = [acc[0] / n, acc[1] / n, acc[2] / n, img.px[i][3]];
                    known[i] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    out
}

/// One model pass with fidelity + detail at the model's native scale.
fn pass(engine: &Engine, src: &Image, st: &Settings, progress: &mut dyn FnMut(TileEvent<'_>) -> bool, tiles: &mut usize) -> Option<Image> {
    let s = engine.scale();
    let n = match st.ensemble {
        0 | 1 => 1,
        2 => 2,
        3 | 4 => 4,
        _ => 8,
    };
    // Transforms in order of usefulness: identity, flip h, transpose, ...
    let order = [0usize, 1, 4, 6, 2, 3, 5, 7];
    let mut up: Option<Image> = None;
    for (i, &k) in order.iter().take(n).enumerate() {
        let t = src.dihedral(k);
        let base = i;
        let o = engine.run(&t, TileOptions { tile: st.tile, pad: st.pad }, &mut |e| {
            *tiles = (*tiles).max(e.total * n);
            progress(TileEvent { done: base * e.total + e.done, total: e.total * n, rect: if k == 0 { e.rect } else { [0; 4] }, tile: e.tile })
        })?;
        let o = o.dihedral_inverse(k);
        up = Some(match up {
            None => o,
            Some(mut acc) => {
                acc.px.par_iter_mut().zip(o.px.par_iter()).for_each(|(a, b)| {
                    for c in 0..3 {
                        a[c] += b[c];
                    }
                });
                acc
            }
        });
    }
    let mut up = up?;
    if n > 1 {
        let inv = 1.0 / n as f32;
        up.px.par_iter_mut().for_each(|a| {
            for c in 0..3 {
                a[c] *= inv;
            }
        });
    }
    if let Some(p) = st.fidelity.params() {
        refine(src, &mut up, s, p);
    }
    // Detail radius tracks the scale: ~1px at 4x, ~0.6px at 2x.
    sharpen_detail(&mut up, st.detail, (s as f32 / 4.0).max(0.5));
    Some(up)
}

pub fn upscale(engine: &Engine, img: &Image, st: &Settings, progress: &mut dyn FnMut(TileEvent<'_>) -> bool) -> Option<(Image, Report)> {
    let t = Instant::now();
    let had_alpha = img.has_alpha();
    let target = st.scale.clamp(1.0, 16.0);
    let (tw, th) = (((img.w as f32) * target).round().max(1.0) as usize, ((img.h as f32) * target).round().max(1.0) as usize);
    let rgb = if had_alpha { bleed_transparent(img) } else { img.clone() };
    let s = engine.scale() as f32;
    let mut cur = rgb;
    let mut passes = 0;
    let mut tiles = 0;
    let mut first_up: Option<Image> = None;
    // Run the model while the remaining factor still needs more than a
    // mild resample (more than 1.5x).
    let mut reached = 1.0f32;
    while passes == 0 || target / reached > 1.5 {
        let src = cur.clone();
        let up = pass(engine, &src, st, progress, &mut tiles)?;
        if passes == 0 {
            first_up = Some(up.clone());
        }
        reached *= s;
        passes += 1;
        cur = up;
        if reached >= target {
            break;
        }
    }
    let fid = first_up.as_ref().map(|u| fidelity_db(img, u, engine.scale())).unwrap_or(0.0);
    let mut out = if (cur.w, cur.h) != (tw, th) {
        // Downscale with antialiasing (Lanczos) to keep the sharpness the
        // model produced; upscale the small remainder with Catmull-Rom.
        let f = if cur.w > tw { Filter::Lanczos3 } else { Filter::CatmullRom };
        resize(&cur, tw, th, f)
    } else {
        cur
    };
    if had_alpha {
        let mut a = Image::new(img.w, img.h);
        a.px.par_iter_mut().zip(img.px.par_iter()).for_each(|(d, s)| *d = [s[3], s[3], s[3], 1.0]);
        let au = resize(&a, tw, th, Filter::CatmullRom);
        out.px.par_iter_mut().zip(au.px.par_iter()).for_each(|(o, a)| o[3] = a[0].clamp(0.0, 1.0));
    }
    let report = Report {
        model: engine.info.id.to_string(),
        in_w: img.w,
        in_h: img.h,
        out_w: out.w,
        out_h: out.h,
        passes,
        tiles,
        seconds: t.elapsed().as_secs_f64(),
        fidelity_db: fid,
        had_alpha,
    };
    Some((out, report))
}
