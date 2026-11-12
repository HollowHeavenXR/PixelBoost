//! "Develop" tools of a photo editor beyond the basic sliders: dehaze,
//! clarity/texture (local contrast), film grain, HSL colour mixer, colour
//! grading wheels and per-channel RGB curves.

use crate::edit::{curve_lut, srgb_to_linear};
use crate::filters::{gauss_plane, hash_noise, luma_plane, sample_plane, smoothstep};
use crate::image::Image;
use crate::resample::{resize, Filter};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[inline(always)]
fn luma3(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

// ------------------------------------------------------------ dehaze

/// Dark-channel-prior dehaze, computed on a small copy (the haze map is
/// smooth) and applied at full size. Negative `amount` adds haze.
pub fn dehaze(img: &mut Image, amount: f32) {
    let amount = amount.clamp(-1.0, 1.0);
    let f = (img.w.max(img.h) as f32 / 256.0).max(1.0);
    let (sw, sh) = (((img.w as f32 / f) as usize).max(1), ((img.h as f32 / f) as usize).max(1));
    let small = resize(img, sw, sh, Filter::Box);
    let dark: Vec<f32> = small.px.iter().map(|p| p[0].min(p[1]).min(p[2])).collect();
    // Min filter approximated by erosion-ish blur of the dark channel.
    let r = ((sw.min(sh) as f32) * 0.03).max(1.0);
    let dark = gauss_plane(&dark, sw, sh, r);
    // Airlight: mean colour of the brightest 0.5% of the dark channel.
    let mut idx: Vec<usize> = (0..dark.len()).collect();
    idx.sort_unstable_by(|a, b| dark[*b].partial_cmp(&dark[*a]).unwrap_or(std::cmp::Ordering::Equal));
    let n = (dark.len() / 200).max(1);
    let mut air = [0.0f32; 3];
    for &i in &idx[..n] {
        for c in 0..3 {
            air[c] += small.px[i][c] / n as f32;
        }
    }
    let air_l = luma3(air).max(0.3);
    let (w, h) = (img.w, img.h);
    let (kx, ky) = (sw as f32 / w as f32, sh as f32 / h as f32);
    img.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let d = sample_plane(&dark, sw, sh, (x as f32 + 0.5) * kx - 0.5, (y as f32 + 0.5) * ky - 0.5);
            if amount > 0.0 {
                let t = (1.0 - 0.92 * amount * d / air_l).max(0.22);
                for c in 0..3 {
                    p[c] = ((p[c] - air[c]) / t + air[c]).clamp(0.0, 1.0);
                }
            } else {
                let k = -amount * 0.55 * (0.4 + 0.6 * smoothstep(0.0, 0.8, 1.0 - d));
                for c in 0..3 {
                    p[c] += (air[c].max(0.75) - p[c]) * k;
                }
            }
        }
    });
}

// ------------------------------------------------------------ local contrast

/// Clarity (radius ~1.5% of the short side, midtones only) and texture
/// (~2 px detail band), applied to luma so colours don't shift.
pub fn local_contrast(img: &mut Image, clarity: f32, texture: f32) {
    let (w, h) = (img.w, img.h);
    let l = luma_plane(img);
    let short = w.min(h) as f32;
    let big = if clarity != 0.0 { Some(gauss_plane(&l, w, h, (short * 0.015).max(2.0))) } else { None };
    let (fine_a, fine_b) = if texture != 0.0 {
        let s = (short / 700.0).max(0.8);
        (Some(gauss_plane(&l, w, h, s)), Some(gauss_plane(&l, w, h, s * 4.0)))
    } else {
        (None, None)
    };
    img.px.par_iter_mut().enumerate().for_each(|(i, p)| {
        let li = l[i];
        let mut nl = li;
        if let Some(b) = &big {
            // Midtone weight keeps clipped areas from haloing.
            let m = 1.0 - (2.0 * li - 1.0).powi(2);
            nl += clarity * 0.9 * (li - b[i]) * (0.25 + 0.75 * m);
        }
        if let (Some(a), Some(b)) = (&fine_a, &fine_b) {
            nl += texture * 1.2 * (a[i] - b[i]);
        }
        let d = nl - li;
        for c in 0..3 {
            p[c] = (p[c] + d).clamp(0.0, 1.0);
        }
    });
}

// ------------------------------------------------------------ grain

/// Film grain: smooth value noise at `size` (0..1 -> ~1..4 px at 1000 px
/// width, scaled with the image so previews match the export), strongest in
/// the midtones.
pub fn grain(img: &mut Image, amount: f32, size: f32) {
    let (w, h) = (img.w, img.h);
    let cell = ((0.8 + size.clamp(0.0, 1.0) * 3.2) * w as f32 / 1000.0).max(0.6);
    let gw = ((w as f32 / cell).ceil() as usize + 2).max(2);
    let gh = ((h as f32 / cell).ceil() as usize + 2).max(2);
    let field: Vec<f32> = (0..gw * gh).map(|i| hash_noise((i % gw) as u32, (i / gw) as u32, 7)).collect();
    img.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let n = sample_plane(&field, gw, gh, x as f32 / cell, y as f32 / cell);
            let l = luma3([p[0], p[1], p[2]]);
            let m = 0.35 + 0.65 * (1.0 - (2.0 * l - 1.0).powi(2));
            let d = n * amount * 0.16 * m;
            for c in 0..3 {
                p[c] = (p[c] + d).clamp(0.0, 1.0);
            }
        }
    });
}

// ------------------------------------------------------------ HSL mixer

/// Eight colour bands (red, orange, yellow, green, aqua, blue, purple,
/// magenta): hue shift, saturation and luminance, each -1..1.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Hsl {
    pub hue: [f32; 8],
    pub sat: [f32; 8],
    pub lum: [f32; 8],
}

const BAND_CENTRES: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 275.0, 315.0];

fn rgb_to_hsv(c: [f32; 3]) -> (f32, f32, f32) {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    let d = mx - mn;
    let h = if d < 1e-6 {
        0.0
    } else if mx == c[0] {
        60.0 * (((c[1] - c[2]) / d).rem_euclid(6.0))
    } else if mx == c[1] {
        60.0 * ((c[2] - c[0]) / d + 2.0)
    } else {
        60.0 * ((c[0] - c[1]) / d + 4.0)
    };
    let s = if mx > 1e-6 { d / mx } else { 0.0 };
    (h, s, mx)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

/// Weights of the 8 bands for a hue (triangular between neighbouring
/// centres, so they sum to 1).
fn band_weights(h: f32) -> [f32; 8] {
    let mut w = [0.0f32; 8];
    for i in 0..8 {
        let a = BAND_CENTRES[i];
        let b = if i == 7 { 360.0 } else { BAND_CENTRES[i + 1] };
        let hh = if h < a && i == 7 { h + 360.0 } else { h };
        if hh >= a && hh < b {
            let t = (hh - a) / (b - a);
            let t = t * t * (3.0 - 2.0 * t);
            w[i] += 1.0 - t;
            w[(i + 1) % 8] += t;
        }
    }
    w
}

pub fn hsl(img: &mut Image, p: &Hsl) {
    if *p == Hsl::default() {
        return;
    }
    img.px.par_iter_mut().for_each(|px| {
        let c = [px[0], px[1], px[2]];
        let (h, s, v) = rgb_to_hsv(c);
        if s < 1e-4 {
            return;
        }
        let w = band_weights(h);
        let (mut dh, mut ds, mut dl) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..8 {
            dh += w[i] * p.hue[i];
            ds += w[i] * p.sat[i];
            dl += w[i] * p.lum[i];
        }
        // Grey-ish pixels move less.
        let k = smoothstep(0.0, 0.25, s);
        let nh = h + dh * 30.0 * k;
        let ns = (s * (1.0 + ds * k)).clamp(0.0, 1.0);
        let mut o = hsv_to_rgb(nh, ns, v);
        if dl != 0.0 {
            let l0 = luma3(o);
            let target = (l0 * (1.0 + dl * 0.45 * k)).clamp(0.0, 1.0);
            let d = target - l0;
            for x in &mut o {
                *x += d;
            }
        }
        px[0] = o[0].clamp(0.0, 1.0);
        px[1] = o[1].clamp(0.0, 1.0);
        px[2] = o[2].clamp(0.0, 1.0);
    });
}

// ------------------------------------------------------------ colour grading

/// One grading wheel: hue in degrees, saturation 0..1, luminance -1..1.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Wheel {
    pub hue: f32,
    pub sat: f32,
    pub lum: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Grade {
    pub shadows: Wheel,
    pub midtones: Wheel,
    pub highlights: Wheel,
    pub global: Wheel,
    /// Moves the shadow/highlight split, -1..1.
    pub balance: f32,
}

fn wheel_offset(wh: &Wheel) -> [f32; 3] {
    let c = hsv_to_rgb(wh.hue, 1.0, 1.0);
    let l = luma3(c);
    // Zero-luma chroma offset.
    [(c[0] - l) * wh.sat, (c[1] - l) * wh.sat, (c[2] - l) * wh.sat]
}

pub fn grade(img: &mut Image, g: &Grade) {
    if *g == Grade::default() {
        return;
    }
    let (os, om, oh, og) = (wheel_offset(&g.shadows), wheel_offset(&g.midtones), wheel_offset(&g.highlights), wheel_offset(&g.global));
    let pivot = 0.5 + g.balance.clamp(-1.0, 1.0) * 0.3;
    img.px.par_iter_mut().for_each(|p| {
        let c = [p[0], p[1], p[2]];
        let l = luma3(c);
        let ws = 1.0 - smoothstep(0.0, pivot + 0.15, l);
        let wh = smoothstep(pivot - 0.15, 1.0, l);
        let wm = (1.0 - ws - wh).max(0.0);
        let mut o = c;
        for k in 0..3 {
            let chroma = 0.30 * (os[k] * ws + om[k] * wm + oh[k] * wh) + 0.22 * og[k];
            o[k] += chroma;
        }
        // Lift / gamma / gain style luminance per range.
        let dl = g.shadows.lum * 0.25 * ws + g.midtones.lum * 0.2 * wm + g.highlights.lum * 0.25 * wh + g.global.lum * 0.2;
        for x in &mut o {
            *x = (*x + dl).clamp(0.0, 1.0);
        }
        p[0] = o[0];
        p[1] = o[1];
        p[2] = o[2];
    });
}

// ------------------------------------------------------------ RGB curves

/// Tone curves: `master` then per channel. Each is a list of outputs at
/// evenly spaced inputs (2..=17 points); empty = identity.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Curves {
    pub master: Vec<f32>,
    pub red: Vec<f32>,
    pub green: Vec<f32>,
    pub blue: Vec<f32>,
}

fn lut_of(pts: &[f32]) -> Option<Vec<f32>> {
    if pts.len() < 2 || pts.len() > 17 {
        return None;
    }
    let n = pts.len();
    let identity = pts.iter().enumerate().all(|(i, v)| (v - i as f32 / (n - 1) as f32).abs() < 1e-4);
    if identity {
        None
    } else {
        Some(curve_lut(pts, 1024))
    }
}

#[inline(always)]
fn look(lut: &[f32], v: f32) -> f32 {
    let x = v.clamp(0.0, 1.0) * 1023.0;
    let i0 = x as usize;
    let f = x - i0 as f32;
    lut[i0] * (1.0 - f) + lut[(i0 + 1).min(1023)] * f
}

pub fn curves(img: &mut Image, c: &Curves) {
    let m = lut_of(&c.master);
    let ch = [lut_of(&c.red), lut_of(&c.green), lut_of(&c.blue)];
    if m.is_none() && ch.iter().all(|x| x.is_none()) {
        return;
    }
    img.px.par_iter_mut().for_each(|p| {
        for k in 0..3 {
            let mut v = p[k];
            if let Some(l) = &m {
                v = look(l, v);
            }
            if let Some(l) = &ch[k] {
                v = look(l, v);
            }
            p[k] = v;
        }
    });
}

// ------------------------------------------------------------ white balance picker

/// Warmth/tint that make the colour at `rgb` neutral grey.
pub fn neutral_balance(rgb: [f32; 3]) -> (f32, f32) {
    let l = [srgb_to_linear(rgb[0]), srgb_to_linear(rgb[1]), srgb_to_linear(rgb[2])];
    // Solve the warm/tint gains of `adjust` approximately: blue-red axis
    // for warmth, green for tint.
    let rb = (l[2].max(1e-4) / l[0].max(1e-4)).ln();
    let warmth = (rb / 0.22).clamp(-1.0, 1.0);
    let g = l[1].max(1e-4) / ((l[0] + l[2]) * 0.5).max(1e-4);
    let tint = (g.ln() / 0.12).clamp(-1.0, 1.0);
    (warmth, tint)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ramp() -> Image {
        let mut img = Image::new(64, 48);
        for (i, p) in img.px.iter_mut().enumerate() {
            let x = (i % 64) as f32 / 63.0;
            let y = (i / 64) as f32 / 47.0;
            *p = [x, y, 1.0 - x, 1.0];
        }
        img
    }
    #[test]
    fn hsl_bands_partition() {
        for h in 0..360 {
            let w = band_weights(h as f32);
            let s: f32 = w.iter().sum();
            assert!((s - 1.0).abs() < 1e-4, "{h} {s}");
        }
        let c = [0.8, 0.3, 0.1];
        let (h, s, v) = rgb_to_hsv(c);
        let r = hsv_to_rgb(h, s, v);
        for k in 0..3 {
            assert!((r[k] - c[k]).abs() < 1e-4);
        }
    }
    #[test]
    fn neutral_settings_are_identity() {
        let img = ramp();
        let mut a = img.clone();
        hsl(&mut a, &Hsl::default());
        grade(&mut a, &Grade::default());
        curves(&mut a, &Curves { master: vec![0.0, 0.5, 1.0], ..Default::default() });
        assert_eq!(a.px, img.px);
    }
    #[test]
    fn tools_stay_finite() {
        let mut img = ramp();
        dehaze(&mut img, 0.7);
        dehaze(&mut img, -0.5);
        local_contrast(&mut img, 0.8, -0.6);
        grain(&mut img, 0.5, 0.5);
        hsl(&mut img, &Hsl { hue: [0.5; 8], sat: [-0.3; 8], lum: [0.2; 8] });
        let wheel = Wheel { hue: 200.0, sat: 0.5, lum: 0.1 };
        grade(&mut img, &Grade { shadows: wheel, highlights: wheel, balance: 0.2, ..Default::default() });
        curves(&mut img, &Curves { red: vec![0.0, 0.6, 1.0], ..Default::default() });
        assert!(img.px.iter().all(|p| p.iter().all(|v| v.is_finite() && *v >= 0.0 && *v <= 1.0)));
    }
    #[test]
    fn hsl_sat_minus_one_desaturates_reds() {
        let mut img = Image::new(1, 1);
        img.px[0] = [0.9, 0.1, 0.1, 1.0];
        hsl(&mut img, &Hsl { sat: [-1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], ..Default::default() });
        let p = img.px[0];
        assert!((p[0] - p[1]).abs() < 0.05, "{p:?}");
    }
}
