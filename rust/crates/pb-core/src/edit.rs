//! Photo editing operations (every tool in the app that isn't the upscaler).
//! All operate on straight-alpha sRGB-encoded `Image`s and leave alpha alone
//! unless the tool is about alpha.

use crate::image::Image;
use crate::resample::{resize, Filter};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

fn map_rgb(img: &mut Image, f: impl Fn([f32; 3]) -> [f32; 3] + Sync) {
    img.px.par_iter_mut().for_each(|p| {
        let o = f([p[0], p[1], p[2]]);
        p[0] = o[0].clamp(0.0, 1.0);
        p[1] = o[1].clamp(0.0, 1.0);
        p[2] = o[2].clamp(0.0, 1.0);
    });
}

fn luma3(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(v: f32) -> f32 {
    let v = v.max(0.0);
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

// ------------------------------------------------------------ adjustments

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Adjustments {
    /// Additive, -1..1.
    pub brightness: f32,
    /// Gain around mid grey, 0..2.
    pub contrast: f32,
    /// 0 = grey, 1 = unchanged, 2 = double.
    pub saturation: f32,
    /// EV stops, -3..3 (applied in linear light).
    pub exposure: f32,
    /// Colour temperature shift, -1 (cool) .. 1 (warm).
    pub warmth: f32,
    /// Recover / deepen highlights and shadows, -1..1.
    pub highlights: f32,
    pub shadows: f32,
    /// Outputs at inputs 0, .25, .5, .75, 1 (monotone cubic through them).
    pub curve: [f32; 5],
    /// 0..1.
    pub vignette: f32,
}

impl Default for Adjustments {
    fn default() -> Self {
        Adjustments { brightness: 0.0, contrast: 1.0, saturation: 1.0, exposure: 0.0, warmth: 0.0, highlights: 0.0, shadows: 0.0, curve: [0.0, 0.25, 0.5, 0.75, 1.0], vignette: 0.0 }
    }
}

/// Monotone (Fritsch–Carlson) cubic through evenly spaced points, as a LUT.
pub fn curve_lut(pts: &[f32], n: usize) -> Vec<f32> {
    let m = pts.len();
    let h = 1.0 / (m - 1) as f32;
    let d: Vec<f32> = (0..m - 1).map(|i| (pts[i + 1] - pts[i]) / h).collect();
    let mut t = vec![0.0f32; m];
    t[0] = d[0];
    t[m - 1] = d[m - 2];
    for i in 1..m - 1 {
        t[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) / 2.0 };
    }
    for i in 0..m - 1 {
        if d[i] == 0.0 {
            t[i] = 0.0;
            t[i + 1] = 0.0;
        } else {
            let a = t[i] / d[i];
            let b = t[i + 1] / d[i];
            let s = a * a + b * b;
            if s > 9.0 {
                let k = 3.0 / s.sqrt();
                t[i] = k * a * d[i];
                t[i + 1] = k * b * d[i];
            }
        }
    }
    (0..n)
        .map(|j| {
            let x = j as f32 / (n - 1) as f32;
            let i = ((x / h) as usize).min(m - 2);
            let u = (x - i as f32 * h) / h;
            let (h00, h10, h01, h11) = (2.0 * u * u * u - 3.0 * u * u + 1.0, u * u * u - 2.0 * u * u + u, -2.0 * u * u * u + 3.0 * u * u, u * u * u - u * u);
            (h00 * pts[i] + h10 * h * t[i] + h01 * pts[i + 1] + h11 * h * t[i + 1]).clamp(0.0, 1.0)
        })
        .collect()
}

pub fn adjust(img: &mut Image, a: &Adjustments) {
    if *a == Adjustments::default() {
        return;
    }
    let lut = if a.curve != Adjustments::default().curve { Some(curve_lut(&a.curve, 1024)) } else { None };
    let gain = 2f32.powf(a.exposure);
    let (w, h) = (img.w as f32, img.h as f32);
    let width = img.w;
    let warm = [1.0 + 0.10 * a.warmth, 1.0 + 0.02 * a.warmth, 1.0 - 0.12 * a.warmth];
    img.px.par_iter_mut().enumerate().for_each(|(i, p)| {
        let mut c = [p[0], p[1], p[2]];
        if a.exposure != 0.0 || a.warmth != 0.0 {
            for k in 0..3 {
                c[k] = linear_to_srgb(srgb_to_linear(c[k]) * gain * warm[k]);
            }
        }
        if a.highlights != 0.0 || a.shadows != 0.0 {
            let l = luma3(c);
            let hw = ((l - 0.5) * 2.0).clamp(0.0, 1.0);
            let sw = ((0.5 - l) * 2.0).clamp(0.0, 1.0);
            let delta = a.highlights * 0.25 * hw * hw + a.shadows * 0.25 * sw * sw;
            for v in &mut c {
                *v += delta;
            }
        }
        for v in &mut c {
            *v = (*v + a.brightness - 0.5) * a.contrast + 0.5;
        }
        if a.saturation != 1.0 {
            let l = luma3(c);
            for v in &mut c {
                *v = l + (*v - l) * a.saturation;
            }
        }
        if let Some(lut) = &lut {
            for v in &mut c {
                let x = v.clamp(0.0, 1.0) * 1023.0;
                let i0 = x as usize;
                let f = x - i0 as f32;
                *v = lut[i0] * (1.0 - f) + lut[(i0 + 1).min(1023)] * f;
            }
        }
        if a.vignette > 0.0 {
            let (x, y) = ((i % width) as f32 / w - 0.5, (i / width) as f32 / h - 0.5);
            let r = (x * x + y * y).sqrt() / 0.7071;
            let v = 1.0 - a.vignette * (r * r).powf(1.5).min(1.0);
            for c in &mut c {
                *c *= v;
            }
        }
        p[0] = c[0].clamp(0.0, 1.0);
        p[1] = c[1].clamp(0.0, 1.0);
        p[2] = c[2].clamp(0.0, 1.0);
    });
}

// ------------------------------------------------------------ filters

pub const LOOKS: &[&str] = &["vivid", "mono", "noir", "silvertone", "chrome", "process", "transfer", "instant", "fade", "sepia", "warm", "cool", "matte"];

pub fn apply_look(img: &mut Image, look: &str, amount: f32) {
    let orig = if amount < 1.0 { Some(img.clone()) } else { None };
    match look {
        "vivid" => map_rgb(img, |c| {
            let l = luma3(c);
            let s = c.map(|v| l + (v - l) * 1.35);
            s.map(|v| (v - 0.5) * 1.08 + 0.5)
        }),
        "mono" => map_rgb(img, |c| [luma3(c); 3]),
        "noir" => {
            let lut = curve_lut(&[0.0, 0.12, 0.45, 0.85, 1.0], 256);
            map_rgb(img, |c| [lut[(luma3(c).clamp(0.0, 1.0) * 255.0) as usize]; 3])
        }
        "silvertone" => map_rgb(img, |c| {
            let l = luma3(c);
            [l * 0.98 + 0.02, l * 0.99 + 0.015, l + 0.03]
        }),
        "chrome" => map_rgb(img, |c| {
            let l = luma3(c);
            c.map(|v| ((l + (v - l) * 1.2) - 0.5) * 1.15 + 0.5)
        }),
        "process" => map_rgb(img, |c| [c[0] * 0.92 + 0.04, c[1] * 1.02, c[2] * 0.85 + 0.12]),
        "transfer" => map_rgb(img, |c| [c[0] * 1.05 + 0.02, c[1] * 0.97 + 0.03, c[2] * 0.88 + 0.02]),
        "instant" => map_rgb(img, |c| [c[0] * 0.9 + 0.1, c[1] * 0.88 + 0.08, c[2] * 0.75 + 0.1]),
        "fade" => map_rgb(img, |c| {
            let l = luma3(c);
            c.map(|v| (l + (v - l) * 0.7) * 0.82 + 0.1)
        }),
        "sepia" => map_rgb(img, |c| {
            [0.393 * c[0] + 0.769 * c[1] + 0.189 * c[2], 0.349 * c[0] + 0.686 * c[1] + 0.168 * c[2], 0.272 * c[0] + 0.534 * c[1] + 0.131 * c[2]]
        }),
        "warm" => map_rgb(img, |c| [c[0] * 1.08, c[1] * 1.02, c[2] * 0.88]),
        "cool" => map_rgb(img, |c| [c[0] * 0.9, c[1] * 1.0, c[2] * 1.1]),
        "matte" => {
            let lut = curve_lut(&[0.08, 0.28, 0.5, 0.72, 0.9], 256);
            map_rgb(img, |c| c.map(|v| lut[(v.clamp(0.0, 1.0) * 255.0) as usize]))
        }
        _ => {}
    }
    if let Some(o) = orig {
        img.px.par_iter_mut().zip(o.px.par_iter()).for_each(|(p, q)| {
            for c in 0..3 {
                p[c] = q[c] + (p[c] - q[c]) * amount;
            }
        });
    }
}

/// One-tap auto correction: per-channel levels stretch (0.5% clip) blended
/// 50/50 with a luma-only stretch to keep colour casts in check, then a
/// gentle contrast-preserving saturation lift.
pub fn auto_enhance(img: &mut Image) {
    let mut hist = [[0u32; 256]; 4];
    for p in &img.px {
        for c in 0..3 {
            hist[c][(p[c].clamp(0.0, 1.0) * 255.0) as usize] += 1;
        }
        hist[3][(luma3([p[0], p[1], p[2]]).clamp(0.0, 1.0) * 255.0) as usize] += 1;
    }
    let n = img.px.len() as u32;
    let clip = n / 200;
    let bounds = |h: &[u32; 256]| {
        let (mut lo, mut acc) = (0usize, 0u32);
        while lo < 255 && acc + h[lo] <= clip {
            acc += h[lo];
            lo += 1;
        }
        let (mut hi, mut acc) = (255usize, 0u32);
        while hi > 0 && acc + h[hi] <= clip {
            acc += h[hi];
            hi -= 1;
        }
        (lo as f32 / 255.0, (hi.max(lo + 1)) as f32 / 255.0)
    };
    let b: Vec<(f32, f32)> = (0..4).map(|c| bounds(&hist[c])).collect();
    map_rgb(img, |c| {
        let mut o = [0.0; 3];
        for k in 0..3 {
            let per = (c[k] - b[k].0) / (b[k].1 - b[k].0);
            let lum = (c[k] - b[3].0) / (b[3].1 - b[3].0);
            o[k] = 0.5 * per + 0.5 * lum;
        }
        let l = luma3(o);
        o.map(|v| l + (v - l) * 1.1)
    });
}

// ------------------------------------------------------------ geometry

pub fn rotate90(img: &Image, clockwise: bool) -> Image {
    let t = img.transpose();
    if clockwise {
        t.flip_h()
    } else {
        t.flip_v()
    }
}

pub fn crop_norm(img: &Image, x: f32, y: f32, w: f32, h: f32) -> Image {
    let x0 = ((x.clamp(0.0, 1.0)) * img.w as f32).round() as usize;
    let y0 = ((y.clamp(0.0, 1.0)) * img.h as f32).round() as usize;
    let cw = ((w * img.w as f32).round() as usize).clamp(1, img.w - x0.min(img.w - 1));
    let ch = ((h * img.h as f32).round() as usize).clamp(1, img.h - y0.min(img.h - 1));
    img.crop(x0.min(img.w - 1), y0.min(img.h - 1), cw, ch)
}

/// Arbitrary-angle straighten (degrees), cropped to the largest upright
/// rectangle with no empty corners. Bicubic sampling.
pub fn straighten(img: &Image, degrees: f32) -> Image {
    if degrees.abs() < 0.01 {
        return img.clone();
    }
    let a = degrees.to_radians();
    let (sa, ca) = (a.sin().abs(), a.cos());
    let (w, h) = (img.w as f32, img.h as f32);
    // Largest axis-aligned rectangle of the same aspect inside the rotated image.
    let k = (w * ca.abs() + h * sa).max(h * ca.abs() + w * sa);
    let s = (w.min(h * w / h) / k * w.min(h).max(1.0) / w.min(h)).min(1.0);
    let scale = (w / (w * ca.abs() + h * sa)).min(h / (h * ca.abs() + w * sa)).min(1.0);
    let _ = s;
    let (ow, oh) = ((w * scale).floor().max(1.0) as usize, (h * scale).floor().max(1.0) as usize);
    let (sn, cs) = (a.sin(), a.cos());
    let (cx, cy) = (w / 2.0, h / 2.0);
    let mut out = Image::new(ow, oh);
    out.px.par_chunks_mut(ow).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let dx = x as f32 + 0.5 - ow as f32 / 2.0;
            let dy = y as f32 + 0.5 - oh as f32 / 2.0;
            let sx = cs * dx + sn * dy + cx - 0.5;
            let sy = -sn * dx + cs * dy + cy - 0.5;
            *o = sample_bicubic(img, sx, sy);
        }
    });
    out
}

pub fn sample_bicubic(img: &Image, x: f32, y: f32) -> [f32; 4] {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let wts = |t: f32| {
        let a = -0.5;
        let w = |d: f32| {
            let d = d.abs();
            if d < 1.0 {
                (a + 2.0) * d * d * d - (a + 3.0) * d * d + 1.0
            } else if d < 2.0 {
                a * d * d * d - 5.0 * a * d * d + 8.0 * a * d - 4.0 * a
            } else {
                0.0
            }
        };
        [w(t + 1.0), w(t), w(1.0 - t), w(2.0 - t)]
    };
    let (wx, wy) = (wts(fx), wts(fy));
    let mut acc = [0.0f32; 4];
    for j in 0..4 {
        for i in 0..4 {
            let p = img.at_clamped(x0 as isize + i as isize - 1, y0 as isize + j as isize - 1);
            let wgt = wx[i] * wy[j];
            for c in 0..4 {
                acc[c] += wgt * p[c];
            }
        }
    }
    acc.map(|v| v.clamp(0.0, 1.0))
}

// ------------------------------------------------------------ LUT

pub struct Lut3d {
    pub n: usize,
    pub data: Vec<[f32; 3]>,
}

/// Parse an Adobe/Resolve `.cube` 3D LUT.
pub fn parse_cube(text: &str) -> Result<Lut3d, String> {
    let mut n = 0;
    let mut data = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some(r) = l.strip_prefix("LUT_3D_SIZE") {
            n = r.trim().parse().map_err(|_| "bad LUT_3D_SIZE")?;
            continue;
        }
        if l.chars().next().map_or(false, |c| c.is_ascii_alphabetic()) {
            continue;
        }
        let v: Vec<f32> = l.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        if v.len() == 3 {
            data.push([v[0], v[1], v[2]]);
        }
    }
    if n < 2 || data.len() != n * n * n {
        return Err(format!("expected {} LUT entries, found {}", n * n * n, data.len()));
    }
    Ok(Lut3d { n, data })
}

pub fn apply_lut(img: &mut Image, lut: &Lut3d, amount: f32) {
    let n = lut.n;
    let at = |r: usize, g: usize, b: usize| lut.data[r + g * n + b * n * n];
    map_rgb(img, |c| {
        let s = c.map(|v| v.clamp(0.0, 1.0) * (n - 1) as f32);
        let i = s.map(|v| (v as usize).min(n - 2));
        let f = [s[0] - i[0] as f32, s[1] - i[1] as f32, s[2] - i[2] as f32];
        let mut o = [0.0f32; 3];
        for dr in 0..2 {
            for dg in 0..2 {
                for db in 0..2 {
                    let w = (if dr == 1 { f[0] } else { 1.0 - f[0] }) * (if dg == 1 { f[1] } else { 1.0 - f[1] }) * (if db == 1 { f[2] } else { 1.0 - f[2] });
                    let v = at(i[0] + dr, i[1] + dg, i[2] + db);
                    for k in 0..3 {
                        o[k] += w * v[k];
                    }
                }
            }
        }
        [c[0] + (o[0] - c[0]) * amount, c[1] + (o[1] - c[1]) * amount, c[2] + (o[2] - c[2]) * amount]
    });
}

// ------------------------------------------------------------ restore

/// Edge-preserving denoise (bilateral in a 5x5 window, sigma scaled by
/// `amount` 0..1).
pub fn denoise(img: &Image, amount: f32) -> Image {
    if amount <= 0.0 {
        return img.clone();
    }
    let sr = 0.02 + 0.10 * amount;
    let inv2sr = 1.0 / (2.0 * sr * sr);
    let ss = 1.6f32;
    let mut out = img.clone();
    let w = img.w;
    out.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let c = img.at(x, y);
            let mut acc = [0.0f32; 3];
            let mut ws = 0.0;
            for dy in -2isize..=2 {
                for dx in -2isize..=2 {
                    let q = img.at_clamped(x as isize + dx, y as isize + dy);
                    let d2 = (q[0] - c[0]).powi(2) + (q[1] - c[1]).powi(2) + (q[2] - c[2]).powi(2);
                    let wt = (-(d2 * inv2sr) - ((dx * dx + dy * dy) as f32) / (2.0 * ss * ss)).exp();
                    for k in 0..3 {
                        acc[k] += wt * q[k];
                    }
                    ws += wt;
                }
            }
            o[0] = acc[0] / ws;
            o[1] = acc[1] / ws;
            o[2] = acc[2] / ws;
        }
    });
    out
}

// ------------------------------------------------------------ pixel art

/// Pixel-art conversion: downscale to `cell`-pixel blocks (area average),
/// quantise to `colors` with k-means in a perceptual space, optional
/// ordered dithering, then scale back up with hard pixel edges.
pub fn pixel_art(img: &Image, cell: usize, colors: usize, dither: bool, outline: bool) -> Image {
    let cell = cell.max(1);
    let (sw, sh) = ((img.w / cell).max(1), (img.h / cell).max(1));
    let mut small = resize(img, sw, sh, Filter::Box);
    let colors = colors.clamp(2, 256);
    // k-means++-ish init: spread over sorted luma.
    let mut pts: Vec<[f32; 3]> = small.px.iter().map(|p| [p[0], p[1], p[2]]).collect();
    pts.sort_by(|a, b| luma3(*a).partial_cmp(&luma3(*b)).unwrap());
    let mut cent: Vec<[f32; 3]> = (0..colors).map(|i| pts[(i * (pts.len() - 1)) / (colors - 1).max(1)]).collect();
    for _ in 0..12 {
        let mut sum = vec![[0.0f32; 4]; colors];
        for p in &pts {
            let k = nearest(&cent, *p);
            for c in 0..3 {
                sum[k][c] += p[c];
            }
            sum[k][3] += 1.0;
        }
        for (k, s) in sum.iter().enumerate() {
            if s[3] > 0.0 {
                cent[k] = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
            }
        }
    }
    const BAYER: [[f32; 4]; 4] = [[0.0, 8.0, 2.0, 10.0], [12.0, 4.0, 14.0, 6.0], [3.0, 11.0, 1.0, 9.0], [15.0, 7.0, 13.0, 5.0]];
    let spread = 0.6 / colors as f32;
    for y in 0..sh {
        for x in 0..sw {
            let p = &mut small.px[y * sw + x];
            let mut c = [p[0], p[1], p[2]];
            if dither {
                let d = (BAYER[y % 4][x % 4] / 16.0 - 0.5) * spread;
                c = c.map(|v| v + d);
            }
            let k = nearest(&cent, c);
            p[0] = cent[k][0];
            p[1] = cent[k][1];
            p[2] = cent[k][2];
            p[3] = if p[3] > 0.5 { 1.0 } else { 0.0 };
        }
    }
    if outline {
        let src = small.clone();
        for y in 0..sh {
            for x in 0..sw {
                if src.px[y * sw + x][3] == 0.0 {
                    continue;
                }
                let edge = [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)].iter().any(|(dx, dy)| {
                    let (xx, yy) = (x as isize + dx, y as isize + dy);
                    xx < 0 || yy < 0 || xx >= sw as isize || yy >= sh as isize || src.px[yy as usize * sw + xx as usize][3] == 0.0
                });
                if edge {
                    let p = &mut small.px[y * sw + x];
                    p[0] *= 0.25;
                    p[1] *= 0.25;
                    p[2] *= 0.25;
                }
            }
        }
    }
    let mut out = Image::new(sw * cell, sh * cell);
    out.px.par_chunks_mut(sw * cell).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            *o = small.px[(y / cell) * sw + x / cell];
        }
    });
    out
}

fn nearest(cent: &[[f32; 3]], p: [f32; 3]) -> usize {
    let mut best = (0, f32::MAX);
    for (i, c) in cent.iter().enumerate() {
        // Weighted RGB distance (cheap perceptual approximation).
        let d = 2.0 * (c[0] - p[0]).powi(2) + 4.0 * (c[1] - p[1]).powi(2) + 3.0 * (c[2] - p[2]).powi(2);
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0
}

// ------------------------------------------------------------ textures

/// Normal map from luma treated as height (Sobel), OpenGL convention.
pub fn normal_map(img: &Image, strength: f32) -> Image {
    let l: Vec<f32> = img.px.iter().map(|p| Image::luma(*p)).collect();
    let (w, h) = (img.w, img.h);
    let at = |x: isize, y: isize| l[y.clamp(0, h as isize - 1) as usize * w + x.clamp(0, w as isize - 1) as usize];
    let mut out = Image::new(w, h);
    out.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let y = y as isize;
        for (x, o) in row.iter_mut().enumerate() {
            let x = x as isize;
            let gx = (at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1)) - (at(x - 1, y - 1) + 2.0 * at(x - 1, y) + at(x - 1, y + 1));
            let gy = (at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1)) - (at(x - 1, y - 1) + 2.0 * at(x, y - 1) + at(x + 1, y - 1));
            let (nx, ny, nz) = (-gx * strength, gy * strength, 1.0);
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            *o = [nx / len * 0.5 + 0.5, ny / len * 0.5 + 0.5, nz / len * 0.5 + 0.5, 1.0];
        }
    });
    out
}

/// Make an image tile seamlessly: blend it with a half-offset copy using a
/// mask that is 0 at the centre and 1 at the borders.
pub fn seamless(img: &Image, blend: f32) -> Image {
    let (w, h) = (img.w, img.h);
    let b = blend.clamp(0.05, 0.5);
    let mut out = img.clone();
    out.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let q = img.at((x + w / 2) % w, (y + h / 2) % h);
            let dx = ((x as f32 + 0.5) / w as f32 - 0.5).abs() * 2.0;
            let dy = ((y as f32 + 0.5) / h as f32 - 0.5).abs() * 2.0;
            let edge = dx.max(dy);
            let t = ((edge - (1.0 - 2.0 * b)) / (2.0 * b)).clamp(0.0, 1.0);
            let t = t * t * (3.0 - 2.0 * t);
            for c in 0..4 {
                o[c] = o[c] * (1.0 - t) + q[c] * t;
            }
        }
    });
    out
}

/// Ambient-occlusion multiply blend (`ao` grey map over `img`).
pub fn ao_blend(img: &mut Image, ao: &Image, amount: f32) {
    let ao = if (ao.w, ao.h) != (img.w, img.h) { resize(ao, img.w, img.h, Filter::CatmullRom) } else { ao.clone() };
    img.px.par_iter_mut().zip(ao.px.par_iter()).for_each(|(p, a)| {
        let m = 1.0 - amount * (1.0 - Image::luma(*a));
        for c in 0..3 {
            p[c] *= m;
        }
    });
}

/// Depth fog: blend towards `color` by depth (luma of `depth`, 1 = far).
pub fn depth_fog(img: &mut Image, depth: &Image, color: [f32; 3], density: f32, start: f32) {
    let depth = if (depth.w, depth.h) != (img.w, img.h) { resize(depth, img.w, img.h, Filter::CatmullRom) } else { depth.clone() };
    img.px.par_iter_mut().zip(depth.px.par_iter()).for_each(|(p, d)| {
        let z = ((Image::luma(*d) - start) / (1.0 - start).max(1e-3)).clamp(0.0, 1.0);
        let f = 1.0 - (-density * 4.0 * z).exp();
        for c in 0..3 {
            p[c] = p[c] * (1.0 - f) + color[c] * f;
        }
    });
}

// ------------------------------------------------------------ masks

/// Blend `edited` over `orig` through a mask (alpha of `mask` resized).
pub fn masked_blend(orig: &Image, edited: &Image, mask: &[f32]) -> Image {
    let mut out = orig.clone();
    out.px.par_iter_mut().zip(edited.px.par_iter()).zip(mask.par_iter()).for_each(|((o, e), m)| {
        for c in 0..4 {
            o[c] += (e[c] - o[c]) * m;
        }
    });
    out
}

/// Clone stamp: copy a soft circular patch from (sx,sy) to (dx,dy).
pub fn clone_stamp(img: &mut Image, sx: f32, sy: f32, dx: f32, dy: f32, radius: f32, hardness: f32) {
    let r = radius.max(1.0);
    let src = img.clone();
    let (x0, x1) = ((dx - r).floor().max(0.0) as usize, ((dx + r).ceil() as usize).min(img.w - 1));
    let (y0, y1) = ((dy - r).floor().max(0.0) as usize, ((dy + r).ceil() as usize).min(img.h - 1));
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = ((x as f32 - dx).powi(2) + (y as f32 - dy).powi(2)).sqrt() / r;
            if d >= 1.0 {
                continue;
            }
            let t = if d < hardness { 1.0 } else { 1.0 - (d - hardness) / (1.0 - hardness).max(1e-3) };
            let q = sample_bicubic(&src, x as f32 + sx - dx, y as f32 + sy - dy);
            let p = &mut img.px[y * img.w + x];
            for c in 0..4 {
                p[c] += (q[c] - p[c]) * t;
            }
        }
    }
}

/// Fill masked pixels (mask > 0.5) from their surroundings: coarse-to-fine
/// diffusion (pyramid), which handles both thin scratches and larger holes
/// without smearing a single colour across them.
pub fn inpaint(img: &Image, mask: &[f32]) -> Image {
    fn solve(img: &Image, known: &[bool]) -> Image {
        let (w, h) = (img.w, img.h);
        let mut cur = img.clone();
        if w > 8 && h > 8 && known.iter().any(|k| !*k) {
            // Solve at half resolution first for a good initial guess.
            let (hw, hh) = (w / 2, h / 2);
            let mut half = Image::new(hw, hh);
            let mut hk = vec![false; hw * hh];
            for y in 0..hh {
                for x in 0..hw {
                    let mut acc = [0.0f32; 4];
                    let mut n = 0.0;
                    for (xx, yy) in [(2 * x, 2 * y), (2 * x + 1, 2 * y), (2 * x, 2 * y + 1), (2 * x + 1, 2 * y + 1)] {
                        if known[yy * w + xx] {
                            let p = img.at(xx, yy);
                            for c in 0..4 {
                                acc[c] += p[c];
                            }
                            n += 1.0;
                        }
                    }
                    if n > 0.0 {
                        half.px[y * hw + x] = acc.map(|v| v / n);
                        hk[y * hw + x] = true;
                    }
                }
            }
            let hs = solve(&half, &hk);
            for y in 0..h {
                for x in 0..w {
                    if !known[y * w + x] {
                        cur.px[y * w + x] = hs.at((x / 2).min(hw - 1), (y / 2).min(hh - 1));
                    }
                }
            }
        }
        for _ in 0..40 {
            let prev = cur.clone();
            for y in 0..h {
                for x in 0..w {
                    if known[y * w + x] {
                        continue;
                    }
                    let mut acc = [0.0f32; 4];
                    for (dx, dy) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                        let q = prev.at_clamped(x as isize + dx, y as isize + dy);
                        for c in 0..4 {
                            acc[c] += q[c];
                        }
                    }
                    cur.px[y * w + x] = acc.map(|v| v / 4.0);
                }
            }
        }
        cur
    }
    let known: Vec<bool> = mask.iter().map(|m| *m <= 0.5).collect();
    solve(img, &known)
}

/// Halo-free sharpen for the editor (same operator as the upscaler's detail
/// pass).
pub fn sharpen(img: &mut Image, amount: f32, radius: f32) {
    crate::enhance::sharpen_detail(img, amount, radius);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_curve_is_identity() {
        let l = curve_lut(&[0.0, 0.25, 0.5, 0.75, 1.0], 101);
        for (i, v) in l.iter().enumerate() {
            assert!((v - i as f32 / 100.0).abs() < 1e-4);
        }
    }
    #[test]
    fn rotate_four_times() {
        let mut img = Image::new(5, 3);
        for (i, p) in img.px.iter_mut().enumerate() {
            p[0] = i as f32 / 15.0;
        }
        let mut r = img.clone();
        for _ in 0..4 {
            r = rotate90(&r, true);
        }
        assert_eq!(r.px, img.px);
        let c = rotate90(&img, true);
        assert_eq!((c.w, c.h), (3, 5));
        // Top-left after a clockwise turn is the old bottom-left.
        assert_eq!(c.at(0, 0), img.at(0, 2));
    }
    #[test]
    fn inpaint_fills_hole_smoothly() {
        let mut img = Image::new(32, 32);
        for (i, p) in img.px.iter_mut().enumerate() {
            let x = (i % 32) as f32 / 31.0;
            *p = [x, x, x, 1.0];
        }
        let mut mask = vec![0.0; 32 * 32];
        for y in 10..20 {
            for x in 10..20 {
                mask[y * 32 + x] = 1.0;
                img.px[y * 32 + x] = [1.0, 0.0, 0.0, 1.0];
            }
        }
        let out = inpaint(&img, &mask);
        let p = out.at(15, 15);
        assert!((p[0] - 15.0 / 31.0).abs() < 0.08 && p[1] > 0.3, "{p:?}");
    }
}
