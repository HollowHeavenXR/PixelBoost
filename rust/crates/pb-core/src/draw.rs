//! Drawing and markup: freehand strokes (pen, marker, highlighter, neon),
//! lines and arrows, rectangles and ellipses. Coordinates are normalised
//! (x by width, y by height); widths are fractions of the image width.

use crate::filters::{gauss_plane, mix, smoothstep};
use crate::image::Image;
use rayon::prelude::*;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mark {
    Stroke {
        points: Vec<[f32; 2]>,
        width: f32,
        color: [f32; 4],
        /// "pen", "marker", "highlighter" or "neon".
        #[serde(default = "pen")]
        style: String,
    },
    Line {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        width: f32,
        color: [f32; 4],
        #[serde(default)]
        arrow: bool,
    },
    Rect {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        width: f32,
        color: [f32; 4],
        #[serde(default)]
        fill: bool,
        /// Corner radius as a fraction of the shorter side of the rectangle.
        #[serde(default)]
        radius: f32,
    },
    Ellipse {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        width: f32,
        color: [f32; 4],
        #[serde(default)]
        fill: bool,
    },
}

fn pen() -> String {
    "pen".into()
}

/// Coverage plane over a pixel box.
struct Cov {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    v: Vec<f32>,
}

impl Cov {
    fn new(img: &Image, bx0: f32, by0: f32, bx1: f32, by1: f32) -> Option<Cov> {
        let x0 = bx0.floor().max(0.0) as usize;
        let y0 = by0.floor().max(0.0) as usize;
        let x1 = (bx1.ceil().max(0.0) as usize).min(img.w);
        let y1 = (by1.ceil().max(0.0) as usize).min(img.h);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Cov { x0, y0, w: x1 - x0, h: y1 - y0, v: vec![0.0; (x1 - x0) * (y1 - y0)] })
    }

    /// Max-accumulate a capsule (segment with round caps).
    fn capsule(&mut self, a: (f32, f32), b: (f32, f32), half: f32, soft: f32) {
        let pad = half + soft + 1.0;
        let xs = ((a.0.min(b.0) - pad).floor().max(self.x0 as f32) as usize).max(self.x0);
        let xe = ((a.0.max(b.0) + pad).ceil() as usize).min(self.x0 + self.w);
        let ys = ((a.1.min(b.1) - pad).floor().max(self.y0 as f32) as usize).max(self.y0);
        let ye = ((a.1.max(b.1) + pad).ceil() as usize).min(self.y0 + self.h);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len2 = dx * dx + dy * dy;
        let (x0, w) = (self.x0, self.w);
        let y0 = self.y0;
        if ye <= ys || xe <= xs {
            return;
        }
        self.v[(ys - y0) * w..(ye - y0) * w].par_chunks_mut(w).enumerate().for_each(|(j, row)| {
            let py = (ys + j) as f32 + 0.5;
            for x in xs..xe {
                let px = x as f32 + 0.5;
                let t = if len2 > 1e-6 { (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let (cx, cy) = (a.0 + dx * t - px, a.1 + dy * t - py);
                let d = (cx * cx + cy * cy).sqrt();
                let c = 1.0 - smoothstep(half - soft, half + 0.75, d);
                let o = &mut row[x - x0];
                if c > *o {
                    *o = c;
                }
            }
        });
    }

    /// Fill from a signed-distance function (negative inside).
    fn fill_sdf(&mut self, sd: impl Fn(f32, f32) -> f32 + Sync, outline: Option<f32>) {
        let (x0, y0, w) = (self.x0, self.y0, self.w);
        self.v.par_chunks_mut(w).enumerate().for_each(|(j, row)| {
            let py = (y0 + j) as f32 + 0.5;
            for (i, o) in row.iter_mut().enumerate() {
                let px = (x0 + i) as f32 + 0.5;
                let d = sd(px, py);
                let c = match outline {
                    Some(half) => 1.0 - smoothstep(half - 0.75, half + 0.75, d.abs()),
                    None => 1.0 - smoothstep(-0.75, 0.75, d),
                };
                *o = o.max(c);
            }
        });
    }
}

fn composite(img: &mut Image, cov: &Cov, color: [f32; 4], multiply: bool) {
    let (iw, cw) = (img.w, cov.w);
    img.px[cov.y0 * iw..(cov.y0 + cov.h) * iw].par_chunks_mut(iw).enumerate().for_each(|(j, row)| {
        for i in 0..cw {
            let a = cov.v[j * cw + i] * color[3];
            if a <= 0.0 {
                continue;
            }
            let p = &mut row[cov.x0 + i];
            for c in 0..3 {
                let top = if multiply { p[c] * color[c] } else { color[c] };
                p[c] = mix(p[c], top, a);
            }
            p[3] = p[3] + (1.0 - p[3]) * a;
        }
    });
}

pub fn draw(img: &mut Image, marks: &[Mark]) {
    let (fw, fh) = (img.w as f32, img.h as f32);
    let px = |p: [f32; 2]| (p[0] * fw, p[1] * fh);
    for m in marks {
        match m {
            Mark::Stroke { points, width, color, style } => {
                if points.is_empty() {
                    continue;
                }
                let half = (width * fw * 0.5).max(0.5);
                let (soft, mult, alpha) = match style.as_str() {
                    "marker" => (half * 0.15, false, color[3] * 0.85),
                    "highlighter" => (half * 0.05, true, color[3]),
                    "neon" => (half * 0.3, false, color[3]),
                    _ => (half * 0.05, false, color[3]),
                };
                let glow = if style == "neon" { half * 3.0 } else { 0.0 };
                let pts: Vec<(f32, f32)> = points.iter().map(|p| px(*p)).collect();
                let (mut bx0, mut by0, mut bx1, mut by1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for p in &pts {
                    bx0 = bx0.min(p.0);
                    by0 = by0.min(p.1);
                    bx1 = bx1.max(p.0);
                    by1 = by1.max(p.1);
                }
                let pad = half + soft + glow * 3.0 + 2.0;
                let Some(mut cov) = Cov::new(img, bx0 - pad, by0 - pad, bx1 + pad, by1 + pad) else { continue };
                if pts.len() == 1 {
                    cov.capsule(pts[0], pts[0], half, soft);
                } else {
                    for s in pts.windows(2) {
                        cov.capsule(s[0], s[1], half, soft);
                    }
                }
                if style == "neon" {
                    let g = gauss_plane(&cov.v, cov.w, cov.h, glow);
                    let gc = Cov { x0: cov.x0, y0: cov.y0, w: cov.w, h: cov.h, v: g.iter().map(|v| (v * 2.2).min(1.0)).collect() };
                    composite(img, &gc, [color[0], color[1], color[2], alpha * 0.9], false);
                    let core = [mix(color[0], 1.0, 0.75), mix(color[1], 1.0, 0.75), mix(color[2], 1.0, 0.75), alpha];
                    let thin = Cov { v: cov.v.iter().map(|v| smoothstep(0.35, 0.9, *v)).collect(), ..cov };
                    composite(img, &thin, core, false);
                } else {
                    composite(img, &cov, [color[0], color[1], color[2], alpha], mult);
                }
            }
            Mark::Line { x0, y0, x1, y1, width, color, arrow } => {
                let (a, b) = (px([*x0, *y0]), px([*x1, *y1]));
                let half = (width * fw * 0.5).max(0.5);
                let head = (half * 8.0).max(((b.0 - a.0).hypot(b.1 - a.1)) * 0.12).min((b.0 - a.0).hypot(b.1 - a.1) * 0.5);
                let pad = half + head + 2.0;
                let Some(mut cov) = Cov::new(img, a.0.min(b.0) - pad, a.1.min(b.1) - pad, a.0.max(b.0) + pad, a.1.max(b.1) + pad) else { continue };
                cov.capsule(a, b, half, 0.5);
                if *arrow {
                    let ang = (a.1 - b.1).atan2(a.0 - b.0);
                    for s in [-0.5f32, 0.5] {
                        let t = ang + s;
                        cov.capsule(b, (b.0 + head * t.cos(), b.1 + head * t.sin()), half, 0.5);
                    }
                }
                composite(img, &cov, *color, false);
            }
            Mark::Rect { x0, y0, x1, y1, width, color, fill, radius } => {
                let (a, b) = (px([x0.min(*x1), y0.min(*y1)]), px([x0.max(*x1), y0.max(*y1)]));
                let half = (width * fw * 0.5).max(0.5);
                let Some(mut cov) = Cov::new(img, a.0 - half - 2.0, a.1 - half - 2.0, b.0 + half + 2.0, b.1 + half + 2.0) else { continue };
                let (cx, cy) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                let (hx, hy) = ((b.0 - a.0) / 2.0, (b.1 - a.1) / 2.0);
                let r = radius.clamp(0.0, 0.5) * 2.0 * hx.min(hy);
                cov.fill_sdf(
                    move |x, y| {
                        let qx = (x - cx).abs() - hx + r;
                        let qy = (y - cy).abs() - hy + r;
                        qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
                    },
                    if *fill { None } else { Some(half) },
                );
                composite(img, &cov, *color, false);
            }
            Mark::Ellipse { x0, y0, x1, y1, width, color, fill } => {
                let (a, b) = (px([x0.min(*x1), y0.min(*y1)]), px([x0.max(*x1), y0.max(*y1)]));
                let half = (width * fw * 0.5).max(0.5);
                let Some(mut cov) = Cov::new(img, a.0 - half - 2.0, a.1 - half - 2.0, b.0 + half + 2.0, b.1 + half + 2.0) else { continue };
                let (cx, cy) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                let (rx, ry) = (((b.0 - a.0) / 2.0).max(1.0), ((b.1 - a.1) / 2.0).max(1.0));
                cov.fill_sdf(
                    move |x, y| {
                        // First-order distance: f / |grad f|.
                        let (dx, dy) = (x - cx, y - cy);
                        let f = (dx / rx).powi(2) + (dy / ry).powi(2) - 1.0;
                        let g = 2.0 * ((dx / (rx * rx)).powi(2) + (dy / (ry * ry)).powi(2)).sqrt();
                        f / g.max(1e-6)
                    },
                    if *fill { None } else { Some(half) },
                );
                composite(img, &cov, *color, false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marks_paint_where_expected() {
        let mut img = Image::new(100, 60);
        for p in img.px.iter_mut() {
            *p = [1.0, 1.0, 1.0, 1.0];
        }
        let marks: Vec<Mark> = serde_json::from_str(
            r#"[
            {"kind":"stroke","points":[[0.1,0.5],[0.5,0.5]],"width":0.04,"color":[1,0,0,1]},
            {"kind":"line","x0":0.6,"y0":0.2,"x1":0.9,"y1":0.2,"width":0.02,"color":[0,0,1,1],"arrow":true},
            {"kind":"rect","x0":0.6,"y0":0.6,"x1":0.9,"y1":0.9,"width":0.01,"color":[0,1,0,1],"fill":true,"radius":0.2},
            {"kind":"ellipse","x0":0.0,"y0":0.0,"x1":0.2,"y1":0.3,"width":0.01,"color":[0,0,0,1]},
            {"kind":"stroke","points":[[0.2,0.8]],"width":0.05,"color":[1,0,1,1],"style":"neon"}
        ]"#,
        )
        .unwrap();
        draw(&mut img, &marks);
        let p = img.at(30, 30);
        assert!(p[0] > 0.9 && p[1] < 0.1, "{p:?}");
        let g = img.at(75, 45);
        assert!(g[1] > 0.9 && g[0] < 0.1, "{g:?}");
        assert!(img.at(99, 0)[0] > 0.99);
        assert!(img.px.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }
}
