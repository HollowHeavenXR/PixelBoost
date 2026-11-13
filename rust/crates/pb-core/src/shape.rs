//! Shape crops (circle, hexagon, heart, star...) with stacked, coloured
//! rings around them — profile pictures and stickers.
//!
//! Every shape is a signed distance function (negative inside, in pixels),
//! so the edge is anti-aliased exactly and a ring at any distance from the
//! edge follows the shape: `d in [start, end]` is the ring's band. The same
//! functions trace outlines for the app's on-canvas preview
//! ([`outline`]), so what the overlay shows is what gets rendered.

use crate::image::Image;
use rayon::prelude::*;
use serde::Deserialize;
use std::f32::consts::{PI, TAU};

/// Shape ids, in the order the app shows them.
pub const SHAPES: &[(&str, &str)] = &[
    ("circle", "Circle"),
    ("ellipse", "Oval"),
    ("squircle", "Squircle"),
    ("rounded", "Rounded"),
    ("arch", "Arch"),
    ("hexagon", "Hexagon"),
    ("octagon", "Octagon"),
    ("pentagon", "Pentagon"),
    ("diamond", "Diamond"),
    ("triangle", "Triangle"),
    ("star", "Star"),
    ("heart", "Heart"),
    ("flower", "Flower"),
    ("badge", "Badge"),
    ("burst", "Burst"),
    ("shield", "Shield"),
];

/// Ring styles, in display order.
pub const RING_STYLES: &[(&str, &str)] = &[
    ("solid", "Solid"),
    ("gradient", "Gradient"),
    ("rainbow", "Rainbow"),
    ("neon", "Neon"),
    ("double", "Double"),
    ("dashed", "Dashed"),
    ("dotted", "Dotted"),
    ("story", "Story"),
];

/// Shapes that stretch to the crop rectangle; the rest keep their own
/// proportions (centred, fitted to the shorter side).
pub fn stretches(shape: &str) -> bool {
    matches!(shape, "ellipse" | "squircle" | "rounded" | "arch")
}

#[derive(Deserialize, Debug, Clone)]
pub struct Ring {
    /// Thickness as a fraction of the shape's size (shorter side).
    #[serde(default = "ring_w")]
    pub width: f32,
    /// Space between the previous edge (photo or inner ring) and this ring,
    /// as a fraction of the size. Negative overlaps the photo.
    #[serde(default)]
    pub gap: f32,
    #[serde(default = "white")]
    pub color: [f32; 4],
    /// Second colour (gradient end, neon core, story ring).
    #[serde(default)]
    pub color2: Option<[f32; 4]>,
    #[serde(default = "solid")]
    pub style: String,
    /// Dashes / dots around the shape.
    #[serde(default = "dashes")]
    pub count: f32,
    /// Gradient / rainbow start angle, degrees.
    #[serde(default)]
    pub angle: f32,
}

fn ring_w() -> f32 {
    0.04
}
fn white() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}
fn solid() -> String {
    "solid".into()
}
fn dashes() -> f32 {
    24.0
}

#[derive(Deserialize, Debug, Clone)]
pub struct ShapeCrop {
    pub shape: String,
    /// Crop rectangle, normalised (defaults: the largest centred square).
    #[serde(default)]
    pub x: Option<f32>,
    #[serde(default)]
    pub y: Option<f32>,
    #[serde(default)]
    pub w: Option<f32>,
    #[serde(default)]
    pub h: Option<f32>,
    /// Degrees, clockwise.
    #[serde(default)]
    pub rotation: f32,
    /// Shape parameter: corner radius (rounded), star inner radius, lobe
    /// depth (flower / badge / burst). 0 = the shape's default.
    #[serde(default)]
    pub param: f32,
    /// Soft edge, fraction of the size.
    #[serde(default)]
    pub feather: f32,
    #[serde(default)]
    pub rings: Vec<Ring>,
    /// Fill behind everything (None = transparent).
    #[serde(default)]
    pub background: Option<[f32; 4]>,
}

// ------------------------------------------------------------ geometry

/// A prepared shape: SDF in pixels around the crop centre.
pub struct Sdf {
    kind: Kind,
    /// Half extents of the crop box (pixels).
    hw: f32,
    hh: f32,
    /// Uniform scale for fixed-proportion shapes (pixels per unit).
    s: f32,
    cos: f32,
    sin: f32,
    param: f32,
    poly: Vec<(f32, f32)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Circle,
    Ellipse,
    Squircle,
    Rounded,
    Arch,
    Poly,
    Heart,
    Lobes(u32),
    Rect,
}

fn regular(n: usize, start_deg: f32) -> Vec<(f32, f32)> {
    (0..n)
        .map(|i| {
            let a = (start_deg + 360.0 * i as f32 / n as f32).to_radians();
            (a.cos(), a.sin())
        })
        .collect()
}

/// Scale a polygon so its bounding box fills [-1,1] on the longer side,
/// centred.
fn fit(mut p: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for &(x, y) in &p {
        x0 = x0.min(x);
        x1 = x1.max(x);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let k = 2.0 / (x1 - x0).max(y1 - y0);
    for v in &mut p {
        *v = ((v.0 - cx) * k, (v.1 - cy) * k);
    }
    p
}

impl Sdf {
    /// `w`, `h`: the crop box in pixels.
    pub fn new(shape: &str, w: f32, h: f32, rotation: f32, param: f32) -> Result<Sdf, String> {
        let (hw, hh) = (w / 2.0, h / 2.0);
        let r = rotation.to_radians();
        let mut poly = Vec::new();
        let kind = match shape {
            "circle" => Kind::Circle,
            "ellipse" | "oval" => Kind::Ellipse,
            "squircle" => Kind::Squircle,
            "rounded" => Kind::Rounded,
            "arch" => Kind::Arch,
            "rect" | "square" => Kind::Rect,
            "hexagon" => {
                poly = fit(regular(6, 0.0));
                Kind::Poly
            }
            "octagon" => {
                poly = fit(regular(8, 22.5));
                Kind::Poly
            }
            "pentagon" => {
                poly = fit(regular(5, -90.0));
                Kind::Poly
            }
            "diamond" => {
                poly = vec![(0.0, -1.0), (0.8, 0.0), (0.0, 1.0), (-0.8, 0.0)];
                Kind::Poly
            }
            "triangle" => {
                poly = fit(regular(3, -90.0));
                Kind::Poly
            }
            "star" => {
                let inner = if param > 0.0 { param.clamp(0.2, 0.9) } else { 0.5 };
                poly = fit((0..10)
                    .map(|i| {
                        let a = (-90.0 + 36.0 * i as f32).to_radians();
                        let rr = if i % 2 == 0 { 1.0 } else { inner };
                        (a.cos() * rr, a.sin() * rr)
                    })
                    .collect());
                Kind::Poly
            }
            "burst" => {
                let inner = if param > 0.0 { 1.0 - param.clamp(0.05, 0.6) } else { 0.82 };
                poly = fit((0..32)
                    .map(|i| {
                        let a = (-90.0 + 360.0 / 32.0 * i as f32).to_radians();
                        let rr = if i % 2 == 0 { 1.0 } else { inner };
                        (a.cos() * rr, a.sin() * rr)
                    })
                    .collect());
                Kind::Poly
            }
            "shield" => {
                let mut p = vec![(-0.85, -1.0), (0.85, -1.0), (0.85, -0.15)];
                // Curved lower sides meeting at the point.
                for i in 1..12 {
                    let t = i as f32 / 12.0;
                    p.push((0.85 * (1.0 - t * t), -0.15 + 1.15 * t));
                }
                p.push((0.0, 1.0));
                for i in (1..12).rev() {
                    let t = i as f32 / 12.0;
                    p.push((-0.85 * (1.0 - t * t), -0.15 + 1.15 * t));
                }
                p.push((-0.85, -0.15));
                poly = fit(p);
                Kind::Poly
            }
            "heart" => Kind::Heart,
            "flower" => Kind::Lobes(6),
            "badge" => Kind::Lobes(16),
            other => return Err(format!("unknown shape {other}")),
        };
        Ok(Sdf { kind, hw, hh, s: hw.min(hh), cos: r.cos(), sin: r.sin(), param, poly })
    }

    /// Signed distance (pixels) of the point (x, y) relative to the crop
    /// centre; negative inside.
    #[inline]
    pub fn dist(&self, x: f32, y: f32) -> f32 {
        // Rotate the point the other way (shape turns clockwise).
        let (x, y) = (x * self.cos + y * self.sin, -x * self.sin + y * self.cos);
        let s = self.s;
        match self.kind {
            Kind::Circle => (x * x + y * y).sqrt() - s,
            Kind::Rect => {
                let (qx, qy) = (x.abs() - self.hw, y.abs() - self.hh);
                qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0)
            }
            Kind::Ellipse => ellipse(x, y, self.hw, self.hh),
            Kind::Rounded => {
                let r = (if self.param > 0.0 { self.param } else { 0.18 }).clamp(0.0, 0.5) * 2.0 * s;
                let (qx, qy) = (x.abs() - self.hw + r, y.abs() - self.hh + r);
                qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
            }
            Kind::Squircle => {
                let n = if self.param > 0.0 { 2.0 + self.param * 8.0 } else { 4.5 };
                superellipse(x, y, self.hw, self.hh, n)
            }
            Kind::Arch => {
                // Box whose top corners are rounded to half the width.
                let r = if y < 0.0 { self.hw.min(self.hh) } else { 0.0 };
                let (qx, qy) = (x.abs() - self.hw + r, y.abs() - self.hh + r);
                qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
            }
            Kind::Poly => poly_dist(&self.poly, x / s, y / s) * s,
            Kind::Heart => {
                // iq's exact heart: y up, tip at 0, lobes reach y = 1.104,
                // x = ±0.604. Fit width 1.207 into 2 units.
                let k = 2.0 / 1.207;
                let (hx, hy) = (x / s / k, -(y / s) / k + 0.552);
                heart(hx, hy) * k * s
            }
            Kind::Lobes(n) => {
                let depth = if self.param > 0.0 { self.param.clamp(0.02, 0.4) } else if n <= 8 { 0.16 } else { 0.06 };
                let (px, py) = (x / s, y / s);
                let rr = (px * px + py * py).sqrt().max(1e-6);
                let th = py.atan2(px) + PI / 2.0;
                // Rounded petals: |cos| keeps notches sharp, lobes round.
                let c = (th * n as f32 / 2.0).cos();
                let r0 = 1.0 - depth + depth * c.abs();
                let dr = -depth * (n as f32 / 2.0) * (th * n as f32 / 2.0).sin() * c.signum();
                (rr - r0) / (1.0 + (dr / rr.max(0.3)).powi(2)).sqrt() * s
            }
        }
    }
}

fn ellipse(x: f32, y: f32, a: f32, b: f32) -> f32 {
    if (a - b).abs() < 1e-3 {
        return x.hypot(y) - a;
    }
    let k0 = (x / a).hypot(y / b);
    let k1 = (x / (a * a)).hypot(y / (b * b));
    if k1 < 1e-9 {
        return -a.min(b);
    }
    k0 * (k0 - 1.0) / k1
}

fn superellipse(x: f32, y: f32, a: f32, b: f32, n: f32) -> f32 {
    let (u, v) = ((x / a).abs().max(1e-9), (y / b).abs().max(1e-9));
    let f = (u.powf(n) + v.powf(n)).powf(1.0 / n);
    if f < 0.5 {
        // Deep inside: the gradient form is unstable near the centre.
        return (f - 1.0) * a.min(b);
    }
    // |∇f| = f^(1-n) * sqrt((u^(n-1)/a)^2 + (v^(n-1)/b)^2)
    let g = f.powf(1.0 - n) * ((u.powf(n - 1.0) / a).powi(2) + (v.powf(n - 1.0) / b).powi(2)).sqrt();
    (f - 1.0) / g.max(1e-9)
}

fn heart(x: f32, y: f32) -> f32 {
    let x = x.abs();
    if y + x > 1.0 {
        let (dx, dy) = (x - 0.25, y - 0.75);
        return (dx * dx + dy * dy).sqrt() - 2f32.sqrt() / 4.0;
    }
    let a = x * x + (y - 1.0) * (y - 1.0);
    let m = 0.5 * (x + y).max(0.0);
    let b = (x - m) * (x - m) + (y - m) * (y - m);
    a.min(b).sqrt() * (x - y).signum()
}

/// Exact distance to a closed polygon (iq), sign from the winding.
fn poly_dist(v: &[(f32, f32)], px: f32, py: f32) -> f32 {
    let n = v.len();
    let mut d = (px - v[0].0).powi(2) + (py - v[0].1).powi(2);
    let mut s = 1.0f32;
    let mut j = n - 1;
    for i in 0..n {
        let (ex, ey) = (v[j].0 - v[i].0, v[j].1 - v[i].1);
        let (wx, wy) = (px - v[i].0, py - v[i].1);
        let t = ((wx * ex + wy * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
        let (bx, by) = (wx - ex * t, wy - ey * t);
        d = d.min(bx * bx + by * by);
        let c1 = py >= v[i].1;
        let c2 = py < v[j].1;
        let c3 = ex * wy > ey * wx;
        if (c1 && c2 && c3) || (!c1 && !c2 && !c3) {
            s = -s;
        }
        j = i;
    }
    s * d.sqrt()
}

/// Points of the iso-line `d = offset` (pixels) around the centre of a
/// `w` x `h` box, `n` samples, normalised to the box (0..1, may go beyond
/// for offsets outside). Every bundled shape is star-shaped about its
/// centre, so marching outwards along each ray finds the line.
pub fn outline(shape: &str, w: f32, h: f32, rotation: f32, param: f32, offset: f32, n: usize) -> Result<Vec<[f32; 2]>, String> {
    let sdf = Sdf::new(shape, w, h, rotation, param)?;
    let (ox, oy) = centre_offset(shape, &sdf);
    let n = n.clamp(8, 2048);
    let max_r = (w + h) * 2.0 + offset.abs() * 2.0;
    Ok((0..n)
        .map(|i| {
            let a = TAU * i as f32 / n as f32;
            let (dx, dy) = (a.cos(), a.sin());
            let mut r = 0.0f32;
            for _ in 0..200 {
                let d = sdf.dist(ox + dx * r, oy + dy * r);
                let step = offset - d;
                if step.abs() < 0.02 || r > max_r {
                    break;
                }
                // Lipschitz-1 SDFs never overshoot; damp the approximate ones.
                r = (r + step * 0.9).max(0.0);
            }
            [(ox + dx * r + w / 2.0) / w, (oy + dy * r + h / 2.0) / h]
        })
        .collect())
}

/// Marching origin: a point well inside the shape (the heart's box centre
/// sits close to its notch).
fn centre_offset(shape: &str, sdf: &Sdf) -> (f32, f32) {
    if shape == "heart" {
        // Shift towards the tip, rotated with the shape.
        let (x, y) = (0.0, 0.12 * sdf.s);
        (x * sdf.cos - y * sdf.sin, x * sdf.sin + y * sdf.cos)
    } else {
        (0.0, 0.0)
    }
}

// ------------------------------------------------------------ render

fn hue_rgb(h: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let f = |n: f32| {
        let k = (n + h).rem_euclid(6.0);
        1.0 - 0.9 * (k.min(4.0 - k).clamp(0.0, 1.0))
    };
    [f(5.0), f(3.0), f(1.0)]
}

fn mix4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

/// Straight-alpha "over".
#[inline]
fn over(dst: &mut [f32; 4], c: [f32; 3], a: f32) {
    if a <= 0.0 {
        return;
    }
    let a = a.min(1.0);
    let oa = a + dst[3] * (1.0 - a);
    if oa <= 1e-6 {
        return;
    }
    for k in 0..3 {
        dst[k] = (c[k] * a + dst[k] * dst[3] * (1.0 - a)) / oa;
    }
    dst[3] = oa;
}

struct Band {
    start: f32,
    end: f32,
    ring: Ring,
}

impl Band {
    /// Ring colour and coverage at a pixel. `d` = shape distance, `t` =
    /// angle around the centre (0..1), `rad` = distance from the centre.
    fn shade(&self, d: f32, t: f32, rad: f32) -> ([f32; 3], f32, Option<([f32; 3], f32)>) {
        let r = &self.ring;
        let bw = self.end - self.start;
        let band = |s: f32, e: f32| ((d - s + 0.5).min(e - d + 0.5)).clamp(0.0, 1.0);
        let c1 = r.color;
        let c2 = r.color2.unwrap_or(c1);
        let tt = (t + r.angle / 360.0).rem_euclid(1.0);
        let mut glow = None;
        let (col, cov) = match r.style.as_str() {
            "gradient" => {
                let m = 0.5 - 0.5 * (tt * TAU).cos();
                (mix4(c1, c2, m), band(self.start, self.end))
            }
            "story" => {
                // Warm three-stop sweep (yellow → orange → magenta → purple).
                let stops = [[1.0, 0.83, 0.25, 1.0], [0.98, 0.45, 0.18, 1.0], [0.86, 0.15, 0.48, 1.0], [0.55, 0.2, 0.85, 1.0]];
                let u = (0.5 - 0.5 * (tt * TAU).cos()) * 3.0;
                let i = (u as usize).min(2);
                (mix4(stops[i], stops[i + 1], u - i as f32), band(self.start, self.end))
            }
            "rainbow" => {
                let c = hue_rgb(tt);
                ([c[0], c[1], c[2], c1[3]], band(self.start, self.end))
            }
            "double" => {
                let line = (bw / 3.0).max(1.0);
                let a = band(self.start, self.start + line).max(band(self.end - line, self.end));
                (c1, a)
            }
            "dashed" | "dotted" => {
                let n = r.count.max(3.0).round();
                let seg = TAU * rad.max(1.0) / n;
                let u = tt * n;
                let f = u - u.floor();
                if r.style == "dashed" {
                    let on = 0.62;
                    let along = ((f * seg + 0.5).min((on - f) * seg + 0.5)).clamp(0.0, 1.0);
                    (c1, band(self.start, self.end) * along)
                } else {
                    let a = (f - 0.5) * seg;
                    let b = d - (self.start + self.end) / 2.0;
                    let rr = bw / 2.0;
                    (c1, (rr - (a * a + b * b).sqrt() + 0.5).clamp(0.0, 1.0))
                }
            }
            "neon" => {
                // White-hot core in colour2 (default white), coloured tube,
                // glow on both sides.
                let core = r.color2.unwrap_or([1.0, 1.0, 1.0, 1.0]);
                let mid = (self.start + self.end) / 2.0;
                let k = (1.0 - ((d - mid).abs() / (bw / 2.0)).min(1.0)).powf(1.5);
                let out = (d - self.end).max(self.start - d).max(0.0);
                let g = (-out / (bw * 0.9).max(1.0)).exp() * 0.75 * (out > 0.0) as u8 as f32;
                glow = Some(([c1[0], c1[1], c1[2]], g * c1[3]));
                (mix4(c1, core, k * 0.85), band(self.start, self.end))
            }
            _ => (c1, band(self.start, self.end)),
        };
        ([col[0], col[1], col[2]], cov * col[3], glow)
    }

    /// How far beyond `end` the ring paints (glow).
    fn reach(&self) -> f32 {
        if self.ring.style == "neon" {
            self.end + (self.end - self.start) * 4.0
        } else {
            self.end
        }
    }
}

pub fn render(img: &Image, spec: &ShapeCrop) -> Result<Image, String> {
    let (iw, ih) = (img.w as f32, img.h as f32);
    // Default rect: largest centred square.
    let side = iw.min(ih);
    let (x, y, w, h) = match (spec.x, spec.y, spec.w, spec.h) {
        (Some(x), Some(y), Some(w), Some(h)) => (x * iw, y * ih, w * iw, h * ih),
        _ => ((iw - side) / 2.0, (ih - side) / 2.0, side, side),
    };
    let (w, h) = (w.max(4.0), h.max(4.0));
    let sdf = Sdf::new(&spec.shape, w, h, spec.rotation, spec.param)?;
    let size = w.min(h);
    let mut bands = Vec::new();
    let mut cur = 0.0f32;
    for r in &spec.rings {
        let start = cur + r.gap * size;
        let end = start + (r.width.max(0.0) * size).max(1.0);
        cur = end;
        bands.push(Band { start, end, ring: r.clone() });
    }
    let reach = bands.iter().map(|b| b.reach()).fold(0.0f32, f32::max);
    let feather = (spec.feather.max(0.0) * size).max(0.0);
    // Rotated shapes can reach past the box corners: measure the outline.
    let mut ext_x = w / 2.0;
    let mut ext_y = h / 2.0;
    if spec.rotation.rem_euclid(360.0) != 0.0 {
        for p in outline(&spec.shape, w, h, spec.rotation, spec.param, 0.0, 256)? {
            ext_x = ext_x.max(((p[0] - 0.5) * w).abs());
            ext_y = ext_y.max(((p[1] - 0.5) * h).abs());
        }
    }
    let mx = (ext_x - w / 2.0 + reach + feather + 2.0).ceil().max(0.0) as usize;
    let my = (ext_y - h / 2.0 + reach + feather + 2.0).ceil().max(0.0) as usize;
    let (bw, bh) = (w.round() as usize, h.round() as usize);
    let (ow, oh) = (bw + 2 * mx, bh + 2 * my);
    if ow * oh > 400_000_000 {
        return Err("shape result too large".into());
    }
    let bg = spec.background.unwrap_or([0.0, 0.0, 0.0, 0.0]);
    let (cx, cy) = (ow as f32 / 2.0, oh as f32 / 2.0);
    let (x0, y0) = (x - mx as f32, y - my as f32);
    let mut out = Image { w: ow, h: oh, px: vec![[0.0; 4]; ow * oh] };
    out.px.par_chunks_mut(ow).enumerate().for_each(|(oy, row)| {
        for (ox, p) in row.iter_mut().enumerate() {
            let (px, py) = (ox as f32 + 0.5 - cx, oy as f32 + 0.5 - cy);
            let d = sdf.dist(px, py);
            *p = bg;
            // Photo inside the shape.
            let cov = if feather > 0.0 { crate::filters::smoothstep(feather, -feather, d) } else { (0.5 - d).clamp(0.0, 1.0) };
            if cov > 0.0 {
                let sx = (x0 + ox as f32).floor() as isize;
                let sy = (y0 + oy as f32).floor() as isize;
                if sx >= 0 && sy >= 0 && (sx as usize) < img.w && (sy as usize) < img.h {
                    let s = img.px[sy as usize * img.w + sx as usize];
                    over(p, [s[0], s[1], s[2]], s[3] * cov);
                }
            }
            if bands.is_empty() {
                continue;
            }
            let t = (py.atan2(px) + PI / 2.0).rem_euclid(TAU) / TAU;
            let rad = px.hypot(py);
            for b in &bands {
                if d < b.start - 2.0 - (b.end - b.start) * 4.0 || d > b.reach() + 2.0 {
                    continue;
                }
                let (c, a, glow) = b.shade(d, t, rad);
                if let Some((gc, ga)) = glow {
                    over(p, gc, ga);
                }
                over(p, c, a);
            }
        }
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(w: usize, h: usize) -> Image {
        let mut img = Image::new(w, h);
        for (i, p) in img.px.iter_mut().enumerate() {
            *p = [(i % w) as f32 / w as f32, (i / w) as f32 / h as f32, 0.5, 1.0];
        }
        img
    }

    #[test]
    fn sdfs_have_right_sign_and_scale() {
        for (id, _) in SHAPES {
            let s = Sdf::new(id, 200.0, 200.0, 0.0, 0.0).unwrap();
            let c = if *id == "heart" { s.dist(0.0, 20.0) } else { s.dist(0.0, 0.0) };
            assert!(c < -20.0, "{id} centre {c}");
            assert!(s.dist(400.0, 0.0) > 150.0, "{id} far");
            assert!(s.dist(0.0, -400.0) > 150.0, "{id} far up");
        }
        let c = Sdf::new("circle", 200.0, 200.0, 0.0, 0.0).unwrap();
        assert!((c.dist(130.0, 0.0) - 30.0).abs() < 1e-3);
    }

    #[test]
    fn outline_sits_on_the_edge() {
        for (id, _) in SHAPES {
            let s = Sdf::new(id, 300.0, 300.0, 15.0, 0.0).unwrap();
            for off in [0.0f32, 12.0] {
                for p in outline(id, 300.0, 300.0, 15.0, 0.0, off, 64).unwrap() {
                    let d = s.dist(p[0] * 300.0 - 150.0, p[1] * 300.0 - 150.0);
                    assert!((d - off).abs() < 1.5, "{id} off {off}: {d}");
                }
            }
        }
    }

    #[test]
    fn circle_crop_with_rings() {
        let img = photo(300, 200);
        let spec: ShapeCrop = serde_json::from_str(
            r#"{"shape":"circle","rings":[{"width":0.05,"gap":0.02,"color":[1,0,0,1]},{"width":0.03,"style":"dashed","color":[0,1,1,1]}]}"#,
        )
        .unwrap();
        let out = render(&img, &spec).unwrap();
        assert_eq!(out.w, out.h);
        // Corner transparent, centre opaque photo.
        assert!(out.px[0][3] < 0.01);
        let c = out.px[(out.h / 2) * out.w + out.w / 2];
        assert!(c[3] > 0.99);
        // Red ring to the right of the photo: radius 100 + gap 4 .. 104+10.
        let cx = out.w / 2;
        let p = out.px[(out.h / 2) * out.w + cx + 109];
        assert!(p[0] > 0.9 && p[1] < 0.1 && p[3] > 0.99, "{p:?}");
        // Gap stays transparent.
        let g = out.px[(out.h / 2) * out.w + cx + 102];
        assert!(g[3] < 0.05, "{g:?}");
    }

    #[test]
    fn every_shape_and_style_renders() {
        let img = photo(120, 90);
        for (id, _) in SHAPES {
            for (st, _) in RING_STYLES {
                let j = format!(r#"{{"shape":"{id}","rotation":10,"x":0.1,"y":0.1,"w":0.6,"h":0.8,"rings":[{{"style":"{st}","width":0.06,"color2":[0,0,1,1]}}],"background":[0,0,0,1]}}"#);
                let spec: ShapeCrop = serde_json::from_str(&j).unwrap();
                let o = render(&img, &spec).unwrap();
                assert!(o.w > 0 && o.px.iter().all(|p| p.iter().all(|v| v.is_finite())), "{id} {st}");
            }
        }
    }
}
