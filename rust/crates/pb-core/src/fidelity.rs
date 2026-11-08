//! Fidelity refinement: make the AI output agree with the source, pixel by
//! pixel.
//!
//! GAN upscalers repaint: they flatten subtle shading into smooth patches,
//! drop faint lines, shift colours, and draw dark/bright outlines along
//! edges. Two passes undo that without losing the model's sharpness:
//!
//! 1. **Back-projection** — shrink the result back to source size, compare
//!    with the source, and push the difference back in. Detail the model
//!    erased comes back; detail it added that the source supports stays.
//! 2. **Halo guard** — every output pixel is limited to the colour range of
//!    its source neighbourhood (plus a small tolerance), which removes
//!    ringing/outline halos and invented bright/dark specks.

use crate::image::Image;
use crate::resample::shrink_box;
use rayon::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fidelity {
    Off,
    Natural,
    Faithful,
}

impl Fidelity {
    pub fn parse(s: &str) -> Option<Fidelity> {
        match s {
            "off" => Some(Fidelity::Off),
            "natural" => Some(Fidelity::Natural),
            "faithful" => Some(Fidelity::Faithful),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FidelityParams {
    pub iterations: usize,
    /// Strength of the low-frequency (tone / colour / shading) correction.
    pub strength: f32,
    /// Correct only the smooth part of the error (tone, colour, shading) and
    /// leave edge placement to the model.
    pub smooth_only: bool,
    /// Colour distance over which a correction still spreads (guided
    /// upsampling range sigma).
    pub sigma_r: f32,
    /// Halo guard tolerance (colour units); `None` disables the guard.
    pub tolerance: Option<f32>,
}

impl Fidelity {
    pub fn params(self) -> Option<FidelityParams> {
        match self {
            Fidelity::Off => None,
            Fidelity::Natural => Some(FidelityParams { iterations: 3, strength: 1.0, smooth_only: true, sigma_r: 0.06, tolerance: Some(0.06) }),
            Fidelity::Faithful => Some(FidelityParams { iterations: 4, strength: 1.0, smooth_only: false, sigma_r: 0.12, tolerance: Some(0.03) }),
        }
    }
}

fn blur3(img: &Image) -> Image {
    // [1 2 1]/4 separable, clamp-to-edge.
    let (w, h) = (img.w, img.h);
    let mut tmp = img.clone();
    tmp.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let a = img.at_clamped(x as isize - 1, y as isize);
            let b = img.at(x, y);
            let c = img.at_clamped(x as isize + 1, y as isize);
            for k in 0..3 {
                o[k] = 0.25 * a[k] + 0.5 * b[k] + 0.25 * c[k];
            }
        }
    });
    let mut out = tmp.clone();
    out.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let a = tmp.at_clamped(x as isize, y as isize - 1);
            let b = tmp.at(x, y);
            let c = tmp.at_clamped(x as isize, (y + 1).min(h - 1) as isize);
            for k in 0..3 {
                o[k] = 0.25 * a[k] + 0.5 * b[k] + 0.25 * c[k];
            }
        }
    });
    out
}

/// Spread a source-resolution correction onto the output, guided by the
/// output itself: an output pixel takes correction only from source pixels
/// whose (shrunk) colour matches its own, so corrections never leak across
/// an edge and never re-imprint the source's pixel staircase.
fn guided_upsample(err: &Image, down: &Image, up: &Image, s: usize, sigma_r: f32) -> Image {
    let inv = 1.0 / (2.0 * sigma_r * sigma_r);
    let sf = s as f32;
    let mut out = Image::new(up.w, up.h);
    out.px.par_chunks_mut(up.w).enumerate().for_each(|(y, row)| {
        let v = (y as f32 + 0.5) / sf - 0.5;
        let vy = v.floor() as isize;
        for (x, o) in row.iter_mut().enumerate() {
            let u = (x as f32 + 0.5) / sf - 0.5;
            let ux = u.floor() as isize;
            let g = up.px[y * up.w + x];
            let mut acc = [0.0f32; 3];
            let mut ws = 0.0f32;
            for j in -1..=2isize {
                for i in -1..=2isize {
                    let (qx, qy) = (ux + i, vy + j);
                    let dx = qx as f32 - u;
                    let dy = qy as f32 - v;
                    let spatial = (-(dx * dx + dy * dy) / 1.2).exp();
                    let d = down.at_clamped(qx, qy);
                    let r2 = (d[0] - g[0]).powi(2) + (d[1] - g[1]).powi(2) + (d[2] - g[2]).powi(2);
                    let wgt = spatial * (-r2 * inv).exp();
                    let e = err.at_clamped(qx, qy);
                    for c in 0..3 {
                        acc[c] += wgt * e[c];
                    }
                    ws += wgt;
                }
            }
            if ws > 1e-6 {
                *o = [acc[0] / ws, acc[1] / ws, acc[2] / ws, 1.0];
            } else {
                *o = [0.0, 0.0, 0.0, 1.0];
            }
        }
    });
    out
}

/// Refine `up` (exactly `s` times `src`) in place.
pub fn refine(src: &Image, up: &mut Image, s: usize, p: FidelityParams) {
    assert_eq!((up.w, up.h), (src.w * s, src.h * s));
    let (w, h) = (src.w, src.h);
    for _ in 0..p.iterations {
        let down = shrink_box(up, s);
        let mut err = Image::new(w, h);
        err.px.par_iter_mut().zip(src.px.par_iter().zip(down.px.par_iter())).for_each(|(e, (a, b))| {
            *e = [a[0] - b[0], a[1] - b[1], a[2] - b[2], 1.0];
        });
        let err = if p.smooth_only { blur3(&err) } else { err };
        let eu = guided_upsample(&err, &down, up, s, p.sigma_r);
        let k = p.strength;
        up.px.par_iter_mut().zip(eu.px.par_iter()).for_each(|(o, e)| {
            for c in 0..3 {
                o[c] = (o[c] + k * e[c]).clamp(0.0, 1.0);
            }
        });
    }
    if let Some(tol) = p.tolerance {
        halo_guard(src, up, s, tol);
    }
}

/// Limit each output pixel to the colour range of the source around it,
/// widened by `tol`. The per-source-pixel 3x3 min/max are interpolated
/// bilinearly to output positions so the limits vary smoothly — a hard
/// per-cell range would imprint the source's pixel grid onto edges.
pub fn halo_guard(src: &Image, up: &mut Image, s: usize, tol: f32) {
    let (w, h) = (src.w, src.h);
    let mut lo = vec![[0.0f32; 3]; w * h];
    let mut hi = vec![[0.0f32; 3]; w * h];
    lo.par_iter_mut().zip(hi.par_iter_mut()).enumerate().for_each(|(i, (l, u))| {
        let (x, y) = ((i % w) as isize, (i / w) as isize);
        *l = [f32::MAX; 3];
        *u = [f32::MIN; 3];
        for dy in -1..=1 {
            for dx in -1..=1 {
                let q = src.at_clamped(x + dx, y + dy);
                for c in 0..3 {
                    l[c] = l[c].min(q[c]);
                    u[c] = u[c].max(q[c]);
                }
            }
        }
    });
    let sf = s as f32;
    let ow = up.w;
    up.px.par_chunks_mut(ow).enumerate().for_each(|(y, row)| {
        let v = ((y as f32 + 0.5) / sf - 0.5).clamp(0.0, (h - 1) as f32);
        let y0 = (v as usize).min(h.saturating_sub(2));
        let fy = (v - y0 as f32).min(1.0);
        let y1 = (y0 + 1).min(h - 1);
        for (x, o) in row.iter_mut().enumerate() {
            let u = ((x as f32 + 0.5) / sf - 0.5).clamp(0.0, (w - 1) as f32);
            let x0 = (u as usize).min(w.saturating_sub(2));
            let fx = (u - x0 as f32).min(1.0);
            let x1 = (x0 + 1).min(w - 1);
            for c in 0..3 {
                let bl = |t: &Vec<[f32; 3]>| {
                    let a = t[y0 * w + x0][c] * (1.0 - fx) + t[y0 * w + x1][c] * fx;
                    let b = t[y1 * w + x0][c] * (1.0 - fx) + t[y1 * w + x1][c] * fx;
                    a * (1.0 - fy) + b * fy
                };
                o[c] = o[c].clamp(bl(&lo) - tol, bl(&hi) + tol);
            }
        }
    });
}

/// How closely the result agrees with the source: PSNR (dB) of the result
/// shrunk back to source size against the source.
pub fn fidelity_db(src: &Image, up: &Image, s: usize) -> f64 {
    let down = shrink_box(up, s);
    crate::metrics::psnr(src, &down, 0)
}
