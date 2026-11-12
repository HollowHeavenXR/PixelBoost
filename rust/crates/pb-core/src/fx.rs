//! Blur family (Gaussian, tilt-shift, zoom, motion, spin, lens/portrait,
//! bloom) and creative effects (chromatic aberration, halftone, duotone,
//! posterize, sketch, glitch, oil paint, mosaic, invert, solarize,
//! threshold, emboss, light leak). Sizes are fractions of the image width so
//! a preview matches the full-size export.

use crate::edit::{linear_to_srgb, srgb_to_linear};
use crate::filters::{gauss, gauss_plane, hash_noise, luma_plane, mix, sample_bilinear, smoothstep};
use crate::image::Image;
use crate::resample::{resize, Filter};
use rayon::prelude::*;

#[inline(always)]
fn luma(p: [f32; 4]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// Blend `fx` over `orig` by `amount` (0..1), keeping orig's alpha.
fn mix_into(orig: &Image, mut fx: Image, amount: f32) -> Image {
    let a = amount.clamp(0.0, 1.0);
    if a >= 0.999 {
        return fx;
    }
    fx.px.par_iter_mut().zip(orig.px.par_iter()).for_each(|(f, o)| {
        for c in 0..3 {
            f[c] = mix(o[c], f[c], a);
        }
        f[3] = o[3];
    });
    fx
}

// ------------------------------------------------------------ blurs

pub fn blur(img: &Image, radius: f32) -> Image {
    gauss(img, radius.max(0.0) * img.w as f32)
}

/// Tilt-shift: a sharp band through (0.5, `y`) at `angle` degrees, `band`
/// and `feather` as fractions of the height; blur grows away from it.
pub fn tilt_shift(img: &Image, y: f32, angle: f32, band: f32, feather: f32, radius: f32) -> Image {
    let sigma = radius.max(0.0) * img.w as f32;
    let b1 = gauss(img, sigma * 0.45);
    let b2 = gauss(img, sigma);
    let (w, h) = (img.w as f32, img.h as f32);
    let (s, c) = angle.to_radians().sin_cos();
    let (cx, cy) = (0.5 * w, y * h);
    let half = band * h * 0.5;
    let fe = (feather * h).max(1.0);
    let mut out = img.clone();
    out.px.par_iter_mut().enumerate().for_each(|(i, p)| {
        let (x, yy) = ((i % img.w) as f32 + 0.5 - cx, (i / img.w) as f32 + 0.5 - cy);
        // Distance from the line (direction (c, s)).
        let d = (-x * s + yy * c).abs();
        let t = smoothstep(half, half + fe, d);
        let (a, b) = (b1.px[i], b2.px[i]);
        for k in 0..4 {
            p[k] = if t < 0.5 { mix(p[k], a[k], t * 2.0) } else { mix(a[k], b[k], t * 2.0 - 1.0) };
        }
    });
    out
}

/// Radial zoom blur towards (cx, cy); `amount` 0..1 = streak length as a
/// fraction of the distance to the centre.
pub fn zoom_blur(img: &Image, cx: f32, cy: f32, amount: f32) -> Image {
    let (w, h) = (img.w as f32, img.h as f32);
    let (cxp, cyp) = (cx * w, cy * h);
    let k = amount.clamp(0.0, 1.0) * 0.35;
    let n = 28;
    let mut out = img.clone();
    out.px.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let (dx, dy) = (cxp - x as f32, cyp - y as f32);
            let mut acc = [0.0f32; 4];
            for j in 0..n {
                let t = k * j as f32 / (n - 1) as f32;
                let s = sample_bilinear(img, x as f32 + dx * t, y as f32 + dy * t);
                for c in 0..4 {
                    acc[c] += s[c];
                }
            }
            *p = acc.map(|v| v / n as f32);
        }
    });
    out
}

/// Rotational blur around (cx, cy) by `degrees`.
pub fn spin_blur(img: &Image, cx: f32, cy: f32, degrees: f32) -> Image {
    let (w, h) = (img.w as f32, img.h as f32);
    let (cxp, cyp) = (cx * w, cy * h);
    let span = degrees.to_radians();
    let n = 32;
    let mut out = img.clone();
    out.px.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let (dx, dy) = (x as f32 - cxp, y as f32 - cyp);
            let mut acc = [0.0f32; 4];
            for j in 0..n {
                let a = span * (j as f32 / (n - 1) as f32 - 0.5);
                let (s, c) = a.sin_cos();
                let q = sample_bilinear(img, cxp + dx * c - dy * s, cyp + dx * s + dy * c);
                for k in 0..4 {
                    acc[k] += q[k];
                }
            }
            *p = acc.map(|v| v / n as f32);
        }
    });
    out
}

/// Linear motion blur: `distance` (fraction of the width) along `angle`.
pub fn motion_blur(img: &Image, angle: f32, distance: f32) -> Image {
    let len = distance.max(0.0) * img.w as f32;
    if len < 1.0 {
        return img.clone();
    }
    let (s, c) = angle.to_radians().sin_cos();
    let n = ((len / 1.5) as usize).clamp(8, 64);
    let mut out = img.clone();
    out.px.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 4];
            for j in 0..n {
                let t = len * (j as f32 / (n - 1) as f32 - 0.5);
                let q = sample_bilinear(img, x as f32 + c * t, y as f32 + s * t);
                for k in 0..4 {
                    acc[k] += q[k];
                }
            }
            *p = acc.map(|v| v / n as f32);
        }
    });
    out
}

/// Lens (bokeh) blur: disc kernel in linear light with highlight boost, so
/// bright points bloom into discs. `keep` (if any) marks what stays sharp —
/// its pixels don't bleed into the blurred background.
pub fn lens_blur(img: &Image, radius: f32, bokeh: f32, keep: Option<&[f32]>) -> Image {
    let rpx = radius.max(0.0) * img.w as f32;
    if rpx < 0.5 {
        return img.clone();
    }
    // Work at a scale where the disc is at most ~10 px across.
    let f = (rpx / 10.0).max(1.0);
    let (sw, sh) = (((img.w as f32 / f).round() as usize).max(1), ((img.h as f32 / f).round() as usize).max(1));
    let small = if f > 1.0 { resize(img, sw, sh, Filter::Box) } else { img.clone() };
    let keep_small: Option<Vec<f32>> = keep.map(|k| {
        let mut m = Image::new(img.w, img.h);
        for (p, v) in m.px.iter_mut().zip(k) {
            *p = [*v, *v, *v, 1.0];
        }
        let m = if f > 1.0 { resize(&m, sw, sh, Filter::Box) } else { m };
        m.px.iter().map(|p| p[0]).collect()
    });
    // Blur in a highlight-expanded space f(x) = x (1 + B x^4) and map back
    // with f^-1: flat bright areas come back unchanged, small bright points
    // keep their energy and spread into bokeh discs.
    let boost = bokeh.clamp(0.0, 1.0) * 12.0;
    let fwd = |x: f32| x * (1.0 + boost * x * x * x * x);
    let lin: Vec<[f32; 4]> = small
        .px
        .par_iter()
        .map(|p| [fwd(srgb_to_linear(p[0])), fwd(srgb_to_linear(p[1])), fwd(srgb_to_linear(p[2])), p[3]])
        .collect();
    let r = rpx / f;
    // Vogel spiral samples cover the disc evenly.
    let ns = 96usize;
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    let taps: Vec<(f32, f32)> = (0..ns)
        .map(|i| {
            let rr = r * ((i as f32 + 0.5) / ns as f32).sqrt();
            let a = i as f32 * golden;
            (rr * a.cos(), rr * a.sin())
        })
        .collect();
    let mut blurred = vec![[0.0f32; 4]; sw * sh];
    blurred.par_chunks_mut(sw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 4];
            let mut wsum = 0.0f32;
            for &(dx, dy) in &taps {
                // Bilinear taps: nearest-pixel taps alias into streaks.
                let fx = (x as f32 + dx).clamp(0.0, (sw - 1) as f32);
                let fy = (y as f32 + dy).clamp(0.0, (sh - 1) as f32);
                let (x0, y0) = (fx as usize, fy as usize);
                let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
                let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                for (i, wb) in [(y0 * sw + x0, (1.0 - tx) * (1.0 - ty)), (y0 * sw + x1, tx * (1.0 - ty)), (y1 * sw + x0, (1.0 - tx) * ty), (y1 * sw + x1, tx * ty)] {
                    if wb <= 0.0 {
                        continue;
                    }
                    let wt = wb * keep_small.as_ref().map(|k| (1.0 - k[i]).max(0.02)).unwrap_or(1.0);
                    for c in 0..4 {
                        acc[c] += lin[i][c] * wt;
                    }
                    wsum += wt;
                }
            }
            *o = acc.map(|v| v / wsum);
        }
    });
    let inv = |y: f32| {
        // Newton on f(x) - y = 0, f' = 1 + 5 B x^4.
        let mut x = y.clamp(0.0, 1.0);
        for _ in 0..5 {
            let x4 = x * x * x * x;
            x -= (x * (1.0 + boost * x4) - y) / (1.0 + 5.0 * boost * x4);
            x = x.clamp(0.0, 1.0);
        }
        x
    };
    let mut bimg = Image { w: sw, h: sh, px: blurred.into_par_iter().map(|p| [linear_to_srgb(inv(p[0])), linear_to_srgb(inv(p[1])), linear_to_srgb(inv(p[2])), p[3]]).collect() };
    if (sw, sh) != (img.w, img.h) {
        bimg = resize(&bimg, img.w, img.h, Filter::CatmullRom);
    }
    if let Some(k) = keep {
        bimg.px.par_iter_mut().zip(img.px.par_iter()).zip(k.par_iter()).for_each(|((b, o), m)| {
            for c in 0..4 {
                b[c] = mix(b[c], o[c], m.clamp(0.0, 1.0));
            }
        });
    }
    bimg
}

/// Bloom / glow: blurred bright parts added back (screen blend).
pub fn bloom(img: &Image, amount: f32, radius: f32, threshold: f32) -> Image {
    let (w, h) = (img.w, img.h);
    let sigma = (radius.max(0.001) * w as f32).max(1.0);
    let mut bright = img.clone();
    bright.px.par_iter_mut().for_each(|p| {
        let k = smoothstep(threshold, (threshold + 0.25).min(1.0), luma(*p));
        for c in 0..3 {
            p[c] *= k;
        }
    });
    let g = gauss(&bright, sigma);
    let _ = h;
    let mut out = img.clone();
    out.px.par_iter_mut().zip(g.px.par_iter()).for_each(|(o, b)| {
        for c in 0..3 {
            let s = b[c] * amount.clamp(0.0, 2.0);
            o[c] = 1.0 - (1.0 - o[c]) * (1.0 - s.min(1.0));
        }
    });
    out
}

// ------------------------------------------------------------ effects

pub const EFFECTS: &[&str] = &["chromatic", "halftone", "duotone", "posterize", "sketch", "glitch", "oil", "mosaic", "invert", "solarize", "threshold", "emboss", "light_leak"];

/// One creative effect. `size` 0..1 is the effect-specific scale, `colors`
/// two colours used by duotone / halftone / light leak.
pub fn effect(img: &Image, name: &str, amount: f32, size: f32, colors: [[f32; 3]; 2]) -> Result<Image, String> {
    let (w, h) = (img.w, img.h);
    let scale = w as f32 / 1000.0;
    let size = size.clamp(0.0, 1.0);
    let fx = match name {
        "chromatic" => {
            let k = 0.002 + size * 0.02;
            let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
            let mut out = img.clone();
            out.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                for (x, p) in row.iter_mut().enumerate() {
                    let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                    p[0] = sample_bilinear(img, cx + dx * (1.0 + k), cy + dy * (1.0 + k))[0];
                    p[2] = sample_bilinear(img, cx + dx * (1.0 - k), cy + dy * (1.0 - k))[2];
                }
            });
            out
        }
        "halftone" => {
            let cell = ((4.0 + size * 20.0) * scale).max(3.0);
            let (s, c) = (45f32.to_radians().sin(), 45f32.to_radians().cos());
            let blurred = gauss(img, cell * 0.35);
            let (ink, paper) = (colors[0], colors[1]);
            let mut out = img.clone();
            out.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                for (x, p) in row.iter_mut().enumerate() {
                    let (u, v) = ((x as f32 * c - y as f32 * s) / cell, (x as f32 * s + y as f32 * c) / cell);
                    let (fu, fv) = (u - u.floor() - 0.5, v - v.floor() - 0.5);
                    let d = (fu * fu + fv * fv).sqrt();
                    let dark = 1.0 - luma(blurred.px[y * w + x]);
                    let rad = dark.sqrt() * std::f32::consts::FRAC_1_SQRT_2;
                    let aa = 1.0 / cell;
                    let t = 1.0 - smoothstep(rad - aa, rad + aa, d);
                    for k in 0..3 {
                        p[k] = mix(paper[k], ink[k], t);
                    }
                }
            });
            out
        }
        "duotone" => {
            let mut out = img.clone();
            out.px.par_iter_mut().for_each(|p| {
                let l = smoothstep(0.0, 1.0, luma(*p));
                for k in 0..3 {
                    p[k] = mix(colors[0][k], colors[1][k], l);
                }
            });
            out
        }
        "posterize" => {
            let levels = (2.0 + (1.0 - size) * 10.0).round();
            let mut out = img.clone();
            out.px.par_iter_mut().for_each(|p| {
                for k in 0..3 {
                    p[k] = (p[k] * (levels - 1.0)).round() / (levels - 1.0);
                }
            });
            out
        }
        "sketch" => {
            let l = luma_plane(img);
            let inv: Vec<f32> = l.iter().map(|v| 1.0 - v).collect();
            let b = gauss_plane(&inv, w, h, ((2.0 + size * 10.0) * scale).max(1.0));
            let mut out = img.clone();
            out.px.par_iter_mut().enumerate().for_each(|(i, p)| {
                let dodge = (l[i] / (1.0 - b[i]).max(1e-3)).min(1.0);
                let v = dodge.powf(1.6);
                p[0] = v;
                p[1] = v;
                p[2] = v;
            });
            out
        }
        "glitch" => {
            let band = ((6.0 + size * 30.0) * scale).max(2.0) as usize;
            let shift_max = (0.02 + size * 0.08) * w as f32;
            let split = (2.0 + size * 10.0) * scale;
            let mut out = img.clone();
            out.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                let b = (y / band) as u32;
                let r = hash_noise(b, 17, 3);
                let shift = if r.abs() > 0.55 { hash_noise(b, 5, 9) * shift_max } else { 0.0 };
                for (x, p) in row.iter_mut().enumerate() {
                    let sx = x as f32 + shift;
                    let q = sample_bilinear(img, sx, y as f32);
                    p[0] = sample_bilinear(img, sx + split, y as f32)[0];
                    p[1] = q[1];
                    p[2] = sample_bilinear(img, sx - split, y as f32)[2];
                }
            });
            out
        }
        "oil" => kuwahara(img, ((2.0 + size * 8.0) * scale).max(1.0).round() as usize),
        "mosaic" => {
            let cell = ((6.0 + size * 40.0) * scale).max(2.0) as usize;
            let (cw, ch) = (w.div_ceil(cell), h.div_ceil(cell));
            let small = resize(img, cw, ch, Filter::Box);
            let mut out = img.clone();
            out.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                for (x, p) in row.iter_mut().enumerate() {
                    *p = small.px[(y / cell).min(ch - 1) * cw + (x / cell).min(cw - 1)];
                }
            });
            out
        }
        "invert" => {
            let mut out = img.clone();
            out.px.par_iter_mut().for_each(|p| {
                for k in 0..3 {
                    p[k] = 1.0 - p[k];
                }
            });
            out
        }
        "solarize" => {
            let t = 0.3 + size * 0.6;
            let mut out = img.clone();
            out.px.par_iter_mut().for_each(|p| {
                for k in 0..3 {
                    if p[k] > t {
                        p[k] = 1.0 - p[k];
                    }
                }
            });
            out
        }
        "threshold" => {
            let t = 0.2 + size * 0.6;
            let mut out = img.clone();
            out.px.par_iter_mut().for_each(|p| {
                let v = smoothstep(t - 0.01, t + 0.01, luma(*p));
                p[0] = v;
                p[1] = v;
                p[2] = v;
            });
            out
        }
        "emboss" => {
            let l = luma_plane(img);
            let d = ((1.0 + size * 4.0) * scale).max(1.0);
            let mut out = img.clone();
            out.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                for (x, p) in row.iter_mut().enumerate() {
                    let a = crate::filters::sample_plane(&l, w, h, x as f32 - d, y as f32 - d);
                    let b = crate::filters::sample_plane(&l, w, h, x as f32 + d, y as f32 + d);
                    let v = (0.5 + (b - a) * 2.0).clamp(0.0, 1.0);
                    p[0] = v;
                    p[1] = v;
                    p[2] = v;
                }
            });
            out
        }
        "light_leak" => {
            // Two soft colour blobs from the corners, screen-blended.
            let (fw, fh) = (w as f32, h as f32);
            let r = 0.45 + size * 0.6;
            let mut out = img.clone();
            out.px.par_iter_mut().enumerate().for_each(|(i, p)| {
                let (x, y) = ((i % w) as f32 / fw, (i / w) as f32 / fh);
                let a = (1.0 - ((x * x + (y - 0.1) * (y - 0.1)).sqrt() / r)).clamp(0.0, 1.0).powf(1.5);
                let bx = 1.0 - x;
                let b = (1.0 - ((bx * bx + (1.0 - y) * (1.0 - y)).sqrt() / (r * 0.8))).clamp(0.0, 1.0).powf(1.5);
                for k in 0..3 {
                    let s = (colors[0][k] * a + colors[1][k] * b * 0.8).min(1.0);
                    p[k] = 1.0 - (1.0 - p[k]) * (1.0 - s);
                }
            });
            out
        }
        other => return Err(format!("unknown effect {other}")),
    };
    Ok(mix_into(img, fx, amount))
}

/// Kuwahara filter (oil paint): each pixel takes the mean of the least
/// varied of its four (r+1)x(r+1) quadrants. Quadrant means are centred box
/// means sampled at diagonal offsets, so the cost is O(1) per pixel and the
/// memory is a few f32 planes.
pub fn kuwahara(img: &Image, r: usize) -> Image {
    let (w, h) = (img.w, img.h);
    let half = (r.max(2) / 2).max(1);
    let p = crate::filters::planes(img);
    let l = luma_plane(img);
    let l2: Vec<f32> = l.par_iter().map(|v| v * v).collect();
    let bl = crate::filters::box_blur_plane(&l, w, h, half);
    let bl2 = crate::filters::box_blur_plane(&l2, w, h, half);
    drop(l2);
    let bc: Vec<Vec<f32>> = (0..3).map(|c| crate::filters::box_blur_plane(&p[c], w, h, half)).collect();
    drop(p);
    let hh = half as isize;
    let offs = [(-hh, -hh), (hh, -hh), (-hh, hh), (hh, hh)];
    let mut out = img.clone();
    out.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let mut best = f32::MAX;
            let mut bi = y * w + x;
            for &(dx, dy) in &offs {
                let xx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                let yy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                let i = yy * w + xx;
                let var = bl2[i] - bl[i] * bl[i];
                if var < best {
                    best = var;
                    bi = i;
                }
            }
            px[0] = bc[0][bi];
            px[1] = bc[1][bi];
            px[2] = bc[2][bi];
        }
    });
    out
}

/// Edge-preserving smoothing (guided filter, image as its own guide) for
/// skin and surfaces. `radius` is a fraction of the width.
pub fn smooth(img: &Image, amount: f32, radius: f32) -> Image {
    let (w, h) = (img.w, img.h);
    let r = ((radius.max(0.0) * w as f32).round() as usize).max(1);
    let eps = 0.0008 + 0.004 * amount.clamp(0.0, 1.0);
    let mut out = img.clone();
    let p = crate::filters::planes(img);
    let mut res: Vec<Vec<f32>> = Vec::with_capacity(3);
    for c in 0..3 {
        let i = &p[c];
        let mean_i = crate::filters::box_blur_plane(i, w, h, r);
        let sq: Vec<f32> = i.par_iter().map(|v| v * v).collect();
        let mean_ii = crate::filters::box_blur_plane(&sq, w, h, r);
        let a: Vec<f32> = mean_ii.par_iter().zip(mean_i.par_iter()).map(|(ii, m)| {
            let var = ii - m * m;
            var / (var + eps)
        }).collect();
        let b: Vec<f32> = a.par_iter().zip(mean_i.par_iter()).map(|(a, m)| m - a * m).collect();
        let ma = crate::filters::box_blur_plane(&a, w, h, r);
        let mb = crate::filters::box_blur_plane(&b, w, h, r);
        res.push(i.par_iter().zip(ma.par_iter().zip(mb.par_iter())).map(|(v, (a, b))| a * v + b).collect());
    }
    let k = amount.clamp(0.0, 1.0);
    out.px.par_iter_mut().enumerate().for_each(|(i, px)| {
        for c in 0..3 {
            px[c] = mix(px[c], res[c][i], k).clamp(0.0, 1.0);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_img() -> Image {
        let mut img = Image::new(48, 32);
        for (i, p) in img.px.iter_mut().enumerate() {
            let (x, y) = (i % 48, i / 48);
            let v = if (x / 6 + y / 6) % 2 == 0 { 0.9 } else { 0.1 };
            *p = [v, (x as f32) / 47.0, (y as f32) / 31.0, 1.0];
        }
        img
    }
    fn ok(i: &Image) -> bool {
        i.px.iter().all(|p| p.iter().all(|v| v.is_finite() && *v >= -1e-4 && *v <= 1.0001))
    }
    #[test]
    fn blurs_are_finite_and_keep_size() {
        let img = test_img();
        for o in [
            blur(&img, 0.02),
            tilt_shift(&img, 0.5, 10.0, 0.2, 0.2, 0.03),
            zoom_blur(&img, 0.5, 0.5, 0.5),
            spin_blur(&img, 0.5, 0.5, 20.0),
            motion_blur(&img, 30.0, 0.1),
            lens_blur(&img, 0.05, 0.6, None),
            lens_blur(&img, 0.05, 0.6, Some(&vec![0.5; 48 * 32])),
            bloom(&img, 0.8, 0.05, 0.6),
            smooth(&img, 0.8, 0.02),
            kuwahara(&img, 3),
        ] {
            assert_eq!((o.w, o.h), (48, 32));
            assert!(ok(&o));
        }
    }
    #[test]
    fn every_effect_runs() {
        let img = test_img();
        for e in EFFECTS {
            let o = effect(&img, e, 0.8, 0.5, [[0.1, 0.1, 0.3], [1.0, 0.8, 0.5]]).unwrap();
            assert!(ok(&o), "{e}");
        }
        assert!(effect(&img, "nope", 1.0, 0.5, [[0.0; 3]; 2]).is_err());
    }
    #[test]
    fn lens_blur_keeps_flat_highlights() {
        let mut img = Image::new(40, 40);
        for p in img.px.iter_mut() {
            *p = [0.92, 0.9, 0.88, 1.0];
        }
        let o = lens_blur(&img, 0.1, 1.0, None);
        assert!(o.px.iter().all(|p| (p[0] - 0.92).abs() < 0.01), "{:?}", o.px[820]);
    }
    #[test]
    fn kuwahara_flat_is_flat() {
        let mut img = Image::new(10, 10);
        for p in img.px.iter_mut() {
            *p = [0.3, 0.4, 0.5, 1.0];
        }
        let o = kuwahara(&img, 2);
        assert!(o.px.iter().all(|p| (p[0] - 0.3).abs() < 1e-5));
    }
}
