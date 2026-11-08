//! Halo-free detail enhancement for upscaled output.
//!
//! Classic unsharp masking draws bright/dark rims along edges. Here the
//! sharpened luma of every pixel is limited to the range of its own 3x3
//! neighbourhood, so edges get steeper but never overshoot, and the boost is
//! gated by local contrast so flat areas (skies, skin, cel-shaded fills) do
//! not pick up amplified noise. Only luma changes, so no colour fringes.

use crate::image::Image;
use rayon::prelude::*;

fn blur(p: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let r = (sigma * 2.5).ceil().max(1.0) as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let ks: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / ks).collect();
    let mut tmp = vec![0.0; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &p[y * w..(y + 1) * w];
        for x in 0..w {
            let mut s = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let xx = (x as isize + i as isize - r).clamp(0, w as isize - 1) as usize;
                s += kv * src[xx];
            }
            row[x] = s;
        }
    });
    let mut out = vec![0.0; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (i, kv) in k.iter().enumerate() {
            let yy = (y as isize + i as isize - r).clamp(0, h as isize - 1) as usize;
            let src = &tmp[yy * w..(yy + 1) * w];
            for x in 0..w {
                row[x] += kv * src[x];
            }
        }
    });
    out
}

/// `amount` 0..1 (0 = untouched). `radius` is the detail scale in output
/// pixels (about 1 for a 4x result).
pub fn sharpen_detail(img: &mut Image, amount: f32, radius: f32) {
    if amount <= 0.0 {
        return;
    }
    let (w, h) = (img.w, img.h);
    let y: Vec<f32> = img.px.par_iter().map(|p| Image::luma(*p)).collect();
    let base = blur(&y, w, h, radius.max(0.4));
    let gain = 2.5 * amount;
    // Noise floor: detail below this is left alone (soft threshold).
    let floor = 0.004;
    img.px.par_chunks_mut(w).enumerate().for_each(|(yy, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let i = yy * w + x;
            let d = y[i] - base[i];
            let ad = d.abs();
            if ad <= floor {
                continue;
            }
            let d = d.signum() * (ad - floor);
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            for dy in -1isize..=1 {
                let ry = (yy as isize + dy).clamp(0, h as isize - 1) as usize;
                for dx in -1isize..=1 {
                    let rx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                    let v = y[ry * w + rx];
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            let target = (y[i] + gain * d).clamp(lo, hi);
            let delta = target - y[i];
            for c in 0..3 {
                p[c] = (p[c] + delta).clamp(0.0, 1.0);
            }
        }
    });
}
