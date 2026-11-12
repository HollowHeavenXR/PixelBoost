//! Compositing and canvas tools: image overlays / stickers / double
//! exposure with blend modes, frames and borders, padding to an aspect
//! ratio, rounded corners, and perspective (keystone) correction.

use crate::edit::sample_bicubic;
use crate::filters::{gauss_plane, mix, sample_bilinear};
use crate::image::Image;
use rayon::prelude::*;

pub const BLEND_MODES: &[&str] = &["normal", "multiply", "screen", "overlay", "soft_light", "hard_light", "lighten", "darken", "add", "difference", "color", "luminosity"];

#[inline(always)]
fn lum(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    let mut o = [c[0] + d, c[1] + d, c[2] + d];
    let l2 = lum(o);
    let mn = o[0].min(o[1]).min(o[2]);
    let mx = o[0].max(o[1]).max(o[2]);
    if mn < 0.0 {
        for v in &mut o {
            *v = l2 + (*v - l2) * l2 / (l2 - mn).max(1e-6);
        }
    }
    if mx > 1.0 {
        for v in &mut o {
            *v = l2 + (*v - l2) * (1.0 - l2) / (mx - l2).max(1e-6);
        }
    }
    o
}

/// Blend `top` onto `base` (both encoded RGB) with a named mode.
pub fn blend(mode: &str, b: [f32; 3], t: [f32; 3]) -> [f32; 3] {
    let per = |f: &dyn Fn(f32, f32) -> f32| [f(b[0], t[0]), f(b[1], t[1]), f(b[2], t[2])];
    match mode {
        "multiply" => per(&|b, t| b * t),
        "screen" => per(&|b, t| 1.0 - (1.0 - b) * (1.0 - t)),
        "overlay" => per(&|b, t| if b < 0.5 { 2.0 * b * t } else { 1.0 - 2.0 * (1.0 - b) * (1.0 - t) }),
        "hard_light" => per(&|b, t| if t < 0.5 { 2.0 * b * t } else { 1.0 - 2.0 * (1.0 - b) * (1.0 - t) }),
        "soft_light" => per(&|b, t| {
            if t <= 0.5 {
                b - (1.0 - 2.0 * t) * b * (1.0 - b)
            } else {
                let d = if b <= 0.25 { ((16.0 * b - 12.0) * b + 4.0) * b } else { b.sqrt() };
                b + (2.0 * t - 1.0) * (d - b)
            }
        }),
        "lighten" => per(&|b, t| b.max(t)),
        "darken" => per(&|b, t| b.min(t)),
        "add" => per(&|b, t| (b + t).min(1.0)),
        "difference" => per(&|b, t| (b - t).abs()),
        "color" => set_lum(t, lum(b)),
        "luminosity" => set_lum(b, lum(t)),
        _ => t,
    }
}

/// Place `top` centred at (x, y) (normalised), `w` = its width as a
/// fraction of the image width (height keeps its aspect), rotated by
/// `rotation` degrees. `fit` = "cover" / "contain" ignores x/y/w and fills
/// the frame instead (double exposure, textures).
#[allow(clippy::too_many_arguments)]
pub fn overlay(img: &Image, top: &Image, x: f32, y: f32, w: f32, rotation: f32, opacity: f32, mode: &str, fit: Option<&str>) -> Image {
    let (iw, ih) = (img.w as f32, img.h as f32);
    let (tw, th) = (top.w as f32, top.h as f32);
    let (cx, cy, scale, rot) = match fit {
        Some("cover") => (iw / 2.0, ih / 2.0, (iw / tw).max(ih / th), 0.0f32),
        Some("contain") => (iw / 2.0, ih / 2.0, (iw / tw).min(ih / th), 0.0),
        _ => (x * iw, y * ih, (w * iw / tw).max(1e-4), rotation),
    };
    let (s, c) = rot.to_radians().sin_cos();
    let (hw, hh) = (tw * scale / 2.0, th * scale / 2.0);
    let ex = hw * c.abs() + hh * s.abs();
    let ey = hw * s.abs() + hh * c.abs();
    let x0 = (cx - ex).floor().max(0.0) as usize;
    let y0 = (cy - ey).floor().max(0.0) as usize;
    let x1 = ((cx + ex).ceil().max(0.0) as usize).min(img.w);
    let y1 = ((cy + ey).ceil().max(0.0) as usize).min(img.h);
    let mut out = img.clone();
    if x1 <= x0 || y1 <= y0 {
        return out;
    }
    let op = opacity.clamp(0.0, 1.0);
    let stride = img.w;
    out.px[y0 * stride..y1 * stride].par_chunks_mut(stride).enumerate().for_each(|(j, row)| {
        let py = (y0 + j) as f32 + 0.5 - cy;
        for xi in x0..x1 {
            let px = xi as f32 + 0.5 - cx;
            let u = (px * c + py * s) / scale + tw / 2.0 - 0.5;
            let v = (-px * s + py * c) / scale + th / 2.0 - 0.5;
            if u < -0.5 || v < -0.5 || u > tw - 0.5 || v > th - 0.5 {
                continue;
            }
            let t = sample_bilinear(top, u, v);
            let a = t[3] * op;
            if a <= 0.0 {
                continue;
            }
            let p = &mut row[xi];
            let bl = blend(mode, [p[0], p[1], p[2]], [t[0], t[1], t[2]]);
            for k in 0..3 {
                p[k] = mix(p[k], bl[k], a);
            }
            p[3] = p[3] + (1.0 - p[3]) * a;
        }
    });
    out
}

// ------------------------------------------------------------ frames

/// Frames. `width` is a fraction of the shorter side; `ratio` (w/h) is
/// used by "pad"; `radius` by "rounded" (fraction of the shorter side).
pub fn frame(img: &Image, style: &str, width: f32, color: [f32; 4], ratio: f32, radius: f32) -> Result<Image, String> {
    let short = img.w.min(img.h) as f32;
    let bw = (width.max(0.0) * short).round() as usize;
    Ok(match style {
        "border" => canvas(img, bw, bw, bw, bw, color),
        "polaroid" => {
            let b = bw.max(1);
            canvas(img, b, b, b, (b as f32 * 3.6).round() as usize, color)
        }
        "film" => {
            // Black border with a thin light inner rule.
            let b = bw.max(2);
            let mut c = canvas(img, b, b, b, b, [0.03, 0.03, 0.03, 1.0]);
            let rule = (b / 8).max(1);
            let (w, h) = (c.w, c.h);
            for y in 0..h {
                for x in 0..w {
                    let inx = x + rule >= b && x < w - b + rule && y + rule >= b && y < h - b + rule;
                    let inner = x >= b && x < w - b && y >= b && y < h - b;
                    if inx && !inner {
                        c.px[y * w + x] = [0.85, 0.85, 0.8, 1.0];
                    }
                }
            }
            c
        }
        "pad" => {
            let r = if ratio > 0.0 { ratio } else { 1.0 };
            let (w, h) = (img.w as f32, img.h as f32);
            let (nw, nh) = if w / h > r { (w, w / r) } else { (h * r, h) };
            let px = ((nw - w) / 2.0).round() as usize;
            let py = ((nh - h) / 2.0).round() as usize;
            let m = bw;
            canvas(img, px + m, py + m, px + m, py + m, color)
        }
        "rounded" => {
            let r = radius.clamp(0.0, 0.5) * short;
            let mut out = img.clone();
            let (w, h) = (img.w as f32, img.h as f32);
            out.px.par_iter_mut().enumerate().for_each(|(i, p)| {
                let (x, y) = ((i % img.w) as f32 + 0.5, (i / img.w) as f32 + 0.5);
                let qx = (x - w / 2.0).abs() - w / 2.0 + r;
                let qy = (y - h / 2.0).abs() - h / 2.0 + r;
                let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
                p[3] *= (0.5 - d).clamp(0.0, 1.0);
            });
            out
        }
        "shadow" => {
            // Photo floating on a coloured card with a soft drop shadow.
            let b = bw.max(4);
            let mut out = canvas(img, b, b, b, b, color);
            let (w, h) = (out.w, out.h);
            let mut m = vec![0.0f32; w * h];
            let off = (b as f32 * 0.25) as usize;
            for y in 0..img.h {
                let yy = y + b + off;
                if yy >= h {
                    break;
                }
                for x in 0..img.w {
                    let xx = x + b + off / 2;
                    if xx < w {
                        m[yy * w + xx] = img.px[y * img.w + x][3];
                    }
                }
            }
            let sh = gauss_plane(&m, w, h, b as f32 * 0.35);
            out.px.par_iter_mut().enumerate().for_each(|(i, p)| {
                let (x, y) = (i % w, i / w);
                let inside = x >= b && x < b + img.w && y >= b && y < b + img.h;
                let k = sh[i] * 0.55;
                if !inside {
                    for c in 0..3 {
                        p[c] *= 1.0 - k;
                    }
                } else {
                    let s = img.px[(y - b) * img.w + (x - b)];
                    let a = s[3];
                    for c in 0..3 {
                        p[c] = mix(p[c] * (1.0 - k), s[c], a);
                    }
                }
            });
            out
        }
        other => return Err(format!("unknown frame {other}")),
    })
}

/// New canvas with margins (left, top, right, bottom) in `color`; the image
/// composited on top (so transparency shows the colour).
fn canvas(img: &Image, l: usize, t: usize, r: usize, b: usize, color: [f32; 4]) -> Image {
    let (w, h) = (img.w + l + r, img.h + t + b);
    let mut out = Image { w, h, px: vec![color; w * h] };
    out.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        if y < t || y >= t + img.h {
            return;
        }
        let src = &img.px[(y - t) * img.w..(y - t + 1) * img.w];
        for (x, s) in src.iter().enumerate() {
            let p = &mut row[l + x];
            let a = s[3];
            for c in 0..3 {
                p[c] = mix(p[c], s[c], a);
            }
            p[3] = p[3] + (1.0 - p[3]) * a;
        }
    });
    out
}

// ------------------------------------------------------------ perspective

/// Homography mapping the unit square corners (0,0),(1,0),(1,1),(0,1) to
/// the quad `q`.
fn square_to_quad(q: [(f32, f32); 4]) -> [f64; 9] {
    let (x0, y0) = (q[0].0 as f64, q[0].1 as f64);
    let (x1, y1) = (q[1].0 as f64, q[1].1 as f64);
    let (x2, y2) = (q[2].0 as f64, q[2].1 as f64);
    let (x3, y3) = (q[3].0 as f64, q[3].1 as f64);
    let dx1 = x1 - x2;
    let dx2 = x3 - x2;
    let dy1 = y1 - y2;
    let dy2 = y3 - y2;
    let sx = x0 - x1 + x2 - x3;
    let sy = y0 - y1 + y2 - y3;
    let (g, h) = if sx.abs() < 1e-12 && sy.abs() < 1e-12 {
        (0.0, 0.0)
    } else {
        let den = dx1 * dy2 - dx2 * dy1;
        ((sx * dy2 - dx2 * sy) / den, (dx1 * sy - sx * dy1) / den)
    };
    [x1 - x0 + g * x1, x3 - x0 + h * x3, x0, y1 - y0 + g * y1, y3 - y0 + h * y3, y0, g, h, 1.0]
}

/// Keystone correction. `vertical` > 0 widens the top (fixes buildings
/// leaning in), < 0 widens the bottom; `horizontal` does the same for the
/// sides. Output keeps the image size and is always fully covered.
pub fn perspective(img: &Image, vertical: f32, horizontal: f32) -> Image {
    let v = vertical.clamp(-1.0, 1.0) * 0.3;
    let hz = horizontal.clamp(-1.0, 1.0) * 0.3;
    // Source quad (normalised) that gets stretched onto the full output.
    let mut q = [(0.0f32, 0.0f32), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
    if v > 0.0 {
        q[0].0 += v / 2.0;
        q[1].0 -= v / 2.0;
    } else {
        q[3].0 += -v / 2.0;
        q[2].0 -= -v / 2.0;
    }
    if hz > 0.0 {
        q[0].1 += hz / 2.0;
        q[3].1 -= hz / 2.0;
    } else {
        q[1].1 += -hz / 2.0;
        q[2].1 -= -hz / 2.0;
    }
    let m = square_to_quad(q);
    let (w, h) = (img.w as f64, img.h as f64);
    let mut out = img.clone();
    out.px.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
        let vv = (y as f64 + 0.5) / h;
        for (x, p) in row.iter_mut().enumerate() {
            let u = (x as f64 + 0.5) / w;
            let den = m[6] * u + m[7] * vv + m[8];
            let sx = (m[0] * u + m[1] * vv + m[2]) / den;
            let sy = (m[3] * u + m[4] * vv + m[5]) / den;
            *p = sample_bicubic(img, (sx * w - 0.5) as f32, (sy * h - 0.5) as f32);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn grad(w: usize, h: usize) -> Image {
        let mut img = Image::new(w, h);
        for (i, p) in img.px.iter_mut().enumerate() {
            *p = [(i % w) as f32 / (w - 1) as f32, (i / w) as f32 / (h - 1) as f32, 0.5, 1.0];
        }
        img
    }
    #[test]
    fn identity_perspective() {
        let img = grad(30, 20);
        let o = perspective(&img, 0.0, 0.0);
        for (a, b) in o.px.iter().zip(&img.px) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 0.02);
            }
        }
        let k = perspective(&img, 0.8, -0.5);
        assert!(k.px.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }
    #[test]
    fn frames_grow_canvas() {
        let img = grad(40, 20);
        assert_eq!(frame(&img, "border", 0.1, [1.0; 4], 0.0, 0.0).unwrap().w, 44);
        let p = frame(&img, "pad", 0.0, [1.0; 4], 1.0, 0.0).unwrap();
        assert_eq!((p.w, p.h), (40, 40));
        let r = frame(&img, "rounded", 0.0, [1.0; 4], 0.0, 0.25).unwrap();
        assert!(r.px[0][3] < 0.1 && r.px[20 * 10][3] > 0.9);
        for s in ["polaroid", "film", "shadow"] {
            let f = frame(&img, s, 0.1, [0.9, 0.9, 0.9, 1.0], 0.0, 0.0).unwrap();
            assert!(f.w > 40 && f.px.iter().all(|p| p.iter().all(|v| v.is_finite())), "{s}");
        }
    }
    #[test]
    fn blend_modes_in_range() {
        for m in BLEND_MODES {
            let o = blend(m, [0.2, 0.5, 0.9], [0.7, 0.1, 0.4]);
            assert!(o.iter().all(|v| (-1e-4..=1.0001).contains(v)), "{m} {o:?}");
        }
        let img = grad(20, 20);
        let top = grad(10, 5);
        let o = overlay(&img, &top, 0.5, 0.5, 0.5, 30.0, 0.8, "screen", None);
        assert_eq!((o.w, o.h), (20, 20));
        let c = overlay(&img, &top, 0.0, 0.0, 0.0, 0.0, 1.0, "normal", Some("cover"));
        // cover: 10x5 scaled 4x to 40x20, centred -> pixel 10 samples u = 4.625.
        assert!((c.at(10, 10)[0] - 4.625 / 9.0).abs() < 0.03, "{:?}", c.at(10, 10));
    }
}
