//! Separable resampling with precomputed weight tables. Downscaling widens
//! the kernel by the scale factor (proper antialiasing); colour is filtered
//! premultiplied so transparent pixels don't bleed their colour into edges.

use crate::image::Image;
use rayon::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Filter {
    /// Exact area average (box) — the right model of a camera / renderer
    /// integrating light over a pixel when shrinking by an integer factor.
    Box,
    Triangle,
    /// Bicubic, a = -0.5.
    CatmullRom,
    Mitchell,
    Lanczos3,
}

impl Filter {
    pub fn support(self) -> f32 {
        match self {
            Filter::Box => 0.5,
            Filter::Triangle => 1.0,
            Filter::CatmullRom | Filter::Mitchell => 2.0,
            Filter::Lanczos3 => 3.0,
        }
    }

    pub fn eval(self, x: f32) -> f32 {
        let x = x.abs();
        match self {
            Filter::Box => {
                if x <= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            Filter::Triangle => (1.0 - x).max(0.0),
            Filter::CatmullRom => cubic(x, 0.0, 0.5),
            Filter::Mitchell => cubic(x, 1.0 / 3.0, 1.0 / 3.0),
            Filter::Lanczos3 => {
                if x < 1e-6 {
                    1.0
                } else if x < 3.0 {
                    let px = std::f32::consts::PI * x;
                    3.0 * px.sin() * (px / 3.0).sin() / (px * px)
                } else {
                    0.0
                }
            }
        }
    }
}

fn cubic(x: f32, b: f32, c: f32) -> f32 {
    if x < 1.0 {
        ((12.0 - 9.0 * b - 6.0 * c) * x * x * x + (-18.0 + 12.0 * b + 6.0 * c) * x * x + (6.0 - 2.0 * b)) / 6.0
    } else if x < 2.0 {
        ((-b - 6.0 * c) * x * x * x + (6.0 * b + 30.0 * c) * x * x + (-12.0 * b - 48.0 * c) * x + (8.0 * b + 24.0 * c)) / 6.0
    } else {
        0.0
    }
}

/// For each output index: `n` clamped source indices and normalised weights.
pub(crate) struct Taps {
    pub n: usize,
    pub idx: Vec<u32>,
    pub w: Vec<f32>,
}

pub(crate) fn taps(src: usize, dst: usize, f: Filter) -> Taps {
    let scale = dst as f32 / src as f32;
    let fs = if scale < 1.0 { 1.0 / scale } else { 1.0 };
    let support = f.support() * fs;
    let n = (support * 2.0).ceil() as usize + 1;
    let mut idx = vec![0u32; dst * n];
    let mut w = vec![0.0f32; dst * n];
    for i in 0..dst {
        let center = (i as f32 + 0.5) / scale - 0.5;
        let lo = (center - support).ceil() as isize;
        let mut sum = 0.0;
        for k in 0..n {
            let s = lo + k as isize;
            let v = f.eval((s as f32 - center) / fs);
            w[i * n + k] = v;
            idx[i * n + k] = s.clamp(0, src as isize - 1) as u32;
            sum += v;
        }
        if sum.abs() > 1e-8 {
            w[i * n..(i + 1) * n].iter_mut().for_each(|v| *v /= sum);
        }
    }
    Taps { n, idx, w }
}

pub fn resize(img: &Image, nw: usize, nh: usize, f: Filter) -> Image {
    if nw == img.w && nh == img.h {
        return img.clone();
    }
    // Premultiply.
    let pre: Vec<[f32; 4]> = img.px.par_iter().map(|p| [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]).collect();
    // Horizontal pass: h x nw.
    let tx = taps(img.w, nw, f);
    let mut mid = vec![[0.0f32; 4]; img.h * nw];
    mid.par_chunks_mut(nw).enumerate().for_each(|(y, row)| {
        let src = &pre[y * img.w..(y + 1) * img.w];
        for (x, o) in row.iter_mut().enumerate() {
            let ws = &tx.w[x * tx.n..(x + 1) * tx.n];
            let is = &tx.idx[x * tx.n..(x + 1) * tx.n];
            let mut acc = [0.0f32; 4];
            for (&wk, &si) in ws.iter().zip(is) {
                let p = src[si as usize];
                for c in 0..4 {
                    acc[c] += wk * p[c];
                }
            }
            *o = acc;
        }
    });
    // Vertical pass.
    let ty = taps(img.h, nh, f);
    let mut out = vec![[0.0f32; 4]; nh * nw];
    out.par_chunks_mut(nw).enumerate().for_each(|(y, row)| {
        let ws = &ty.w[y * ty.n..(y + 1) * ty.n];
        let is = &ty.idx[y * ty.n..(y + 1) * ty.n];
        for (&wk, &si) in ws.iter().zip(is) {
            if wk == 0.0 {
                continue;
            }
            let si = si as usize;
            let src = &mid[si * nw..(si + 1) * nw];
            for (o, p) in row.iter_mut().zip(src) {
                for c in 0..4 {
                    o[c] += wk * p[c];
                }
            }
        }
        for o in row.iter_mut() {
            let a = o[3].clamp(0.0, 1.0);
            if a > 1e-6 {
                o[0] /= a;
                o[1] /= a;
                o[2] /= a;
            }
            o[3] = a;
        }
    });
    Image { w: nw, h: nh, px: out }
}

/// Fast exact box-average shrink by an integer factor.
pub fn shrink_box(img: &Image, f: usize) -> Image {
    let (nw, nh) = (img.w / f, img.h / f);
    let inv = 1.0 / (f * f) as f32;
    let mut out = vec![[0.0f32; 4]; nw * nh];
    out.par_chunks_mut(nw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 4];
            for dy in 0..f {
                for dx in 0..f {
                    let p = img.px[(y * f + dy) * img.w + x * f + dx];
                    acc[0] += p[0] * p[3];
                    acc[1] += p[1] * p[3];
                    acc[2] += p[2] * p[3];
                    acc[3] += p[3];
                }
            }
            let a = acc[3] * inv;
            *o = if a > 1e-6 { [acc[0] * inv / a, acc[1] * inv / a, acc[2] * inv / a, a] } else { [0.0, 0.0, 0.0, 0.0] };
        }
    });
    Image { w: nw, h: nh, px: out }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constant_stays_constant() {
        let mut img = Image::new(37, 23);
        img.px.iter_mut().for_each(|p| *p = [0.3, 0.6, 0.9, 1.0]);
        for f in [Filter::Box, Filter::Triangle, Filter::CatmullRom, Filter::Mitchell, Filter::Lanczos3] {
            for (w, h) in [(148, 92), (10, 7), (37, 23), (50, 11)] {
                let r = resize(&img, w, h, f);
                for p in &r.px {
                    assert!((p[0] - 0.3).abs() < 1e-4 && (p[2] - 0.9).abs() < 1e-4, "{f:?} {w}x{h} {p:?}");
                }
            }
        }
    }
}
