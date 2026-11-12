//! Masks for local adjustments: painted (a file), linear and radial
//! gradients, luminance and colour ranges, and combinations. All geometry
//! is in normalised image coordinates (0..1) so the same mask works on the
//! preview and on the full-size image.

use crate::filters::{gauss_plane, smoothstep};
use crate::image::Image;
use rayon::prelude::*;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaskSpec {
    /// White = selected (alpha if the file has transparency, else luma).
    File { path: String },
    /// Full effect at (x0, y0), fading to none at (x1, y1).
    Linear { x0: f32, y0: f32, x1: f32, y1: f32 },
    /// Ellipse centred at (cx, cy) with radii (rx, ry) as fractions of the
    /// image width / height, rotated by `angle` degrees. `feather` 0..1 is
    /// the soft fraction of the radius.
    Radial {
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        #[serde(default)]
        angle: f32,
        #[serde(default = "half")]
        feather: f32,
    },
    /// Pixels whose luma is in [lo, hi], softened by `feather`.
    Luminance {
        lo: f32,
        hi: f32,
        #[serde(default = "tenth")]
        feather: f32,
    },
    /// Pixels close in colour (hue + saturation) to `color`.
    Color {
        color: [f32; 3],
        #[serde(default = "fifth")]
        tolerance: f32,
    },
    /// Everything.
    All,
    Invert { mask: Box<MaskSpec> },
    /// Product (overlap) of masks.
    Intersect { masks: Vec<MaskSpec> },
    /// Maximum (union) of masks.
    Union { masks: Vec<MaskSpec> },
    /// Soften a mask's edges (`radius` as a fraction of the width).
    Feather { mask: Box<MaskSpec>, radius: f32 },
}

fn half() -> f32 {
    0.5
}
fn tenth() -> f32 {
    0.1
}
fn fifth() -> f32 {
    0.2
}

impl MaskSpec {
    /// Paths this mask reads (for portability checks of saved presets).
    pub fn uses_files(&self) -> bool {
        match self {
            MaskSpec::File { .. } => true,
            MaskSpec::Invert { mask } | MaskSpec::Feather { mask, .. } => mask.uses_files(),
            MaskSpec::Intersect { masks } | MaskSpec::Union { masks } => masks.iter().any(|m| m.uses_files()),
            _ => false,
        }
    }
}

/// Render the mask at the image's size, values in [0, 1].
pub fn render(spec: &MaskSpec, img: &Image) -> Result<Vec<f32>, String> {
    let (w, h) = (img.w, img.h);
    let (fw, fh) = (w as f32, h as f32);
    Ok(match spec {
        MaskSpec::File { path } => crate::ops::load_mask(path, w, h)?,
        MaskSpec::All => vec![1.0; w * h],
        MaskSpec::Linear { x0, y0, x1, y1 } => {
            // Work in pixels so the gradient is perpendicular on screen.
            let (ax, ay, bx, by) = (x0 * fw, y0 * fh, x1 * fw, y1 * fh);
            let (dx, dy) = (bx - ax, by - ay);
            let len2 = (dx * dx + dy * dy).max(1e-6);
            (0..w * h)
                .into_par_iter()
                .map(|i| {
                    let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                    let t = ((x - ax) * dx + (y - ay) * dy) / len2;
                    1.0 - smoothstep(0.0, 1.0, t)
                })
                .collect()
        }
        MaskSpec::Radial { cx, cy, rx, ry, angle, feather } => {
            let (cxp, cyp) = (cx * fw, cy * fh);
            let (rxp, ryp) = ((rx * fw).max(1.0), (ry * fh).max(1.0));
            let (s, c) = (-angle.to_radians()).sin_cos();
            let inner = (1.0 - feather.clamp(0.0, 1.0)).max(0.0);
            (0..w * h)
                .into_par_iter()
                .map(|i| {
                    let (x, y) = ((i % w) as f32 + 0.5 - cxp, (i / w) as f32 + 0.5 - cyp);
                    let (u, v) = (x * c - y * s, x * s + y * c);
                    let d = ((u / rxp).powi(2) + (v / ryp).powi(2)).sqrt();
                    1.0 - smoothstep(inner, 1.0, d)
                })
                .collect()
        }
        MaskSpec::Luminance { lo, hi, feather } => {
            let f = feather.max(1e-3);
            img.px
                .par_iter()
                .map(|p| {
                    let l = Image::luma(*p);
                    smoothstep(lo - f, *lo, l) * (1.0 - smoothstep(*hi, hi + f, l))
                })
                .collect()
        }
        MaskSpec::Color { color, tolerance } => {
            let t = tolerance.clamp(0.01, 1.0);
            let target = chroma(*color);
            img.px
                .par_iter()
                .map(|p| {
                    let q = chroma([p[0], p[1], p[2]]);
                    let d = ((q.0 - target.0).powi(2) + (q.1 - target.1).powi(2)).sqrt() + 0.35 * (q.2 - target.2).abs();
                    1.0 - smoothstep(t * 0.5, t, d)
                })
                .collect()
        }
        MaskSpec::Invert { mask } => render(mask, img)?.into_par_iter().map(|v| 1.0 - v).collect(),
        MaskSpec::Intersect { masks } => {
            let mut acc = vec![1.0f32; w * h];
            for m in masks {
                let r = render(m, img)?;
                acc.par_iter_mut().zip(r.par_iter()).for_each(|(a, b)| *a *= b);
            }
            acc
        }
        MaskSpec::Union { masks } => {
            let mut acc = vec![0.0f32; w * h];
            for m in masks {
                let r = render(m, img)?;
                acc.par_iter_mut().zip(r.par_iter()).for_each(|(a, b)| *a = a.max(*b));
            }
            acc
        }
        MaskSpec::Feather { mask, radius } => {
            let m = render(mask, img)?;
            gauss_plane(&m, w, h, (radius * fw).max(0.0))
        }
    })
}

/// Opponent-colour chroma (a, b) plus luma, cheap Lab-like space.
fn chroma(c: [f32; 3]) -> (f32, f32, f32) {
    let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    (c[0] - c[1], 0.5 * (c[0] + c[1]) - c[2], l)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shapes() {
        let img = Image::new(40, 20);
        let lin = render(&MaskSpec::Linear { x0: 0.0, y0: 0.5, x1: 1.0, y1: 0.5 }, &img).unwrap();
        assert!(lin[10 * 40] > 0.95 && lin[10 * 40 + 39] < 0.05);
        let rad = render(&MaskSpec::Radial { cx: 0.5, cy: 0.5, rx: 0.25, ry: 0.5, angle: 0.0, feather: 0.2 }, &img).unwrap();
        assert!(rad[10 * 40 + 20] > 0.99 && rad[0] < 0.01);
        let inv = render(&MaskSpec::Invert { mask: Box::new(MaskSpec::All) }, &img).unwrap();
        assert!(inv.iter().all(|v| *v == 0.0));
    }
    #[test]
    fn parses() {
        let m: MaskSpec = serde_json::from_str(r#"{"kind":"intersect","masks":[{"kind":"luminance","lo":0.6,"hi":1},{"kind":"radial","cx":0.5,"cy":0.5,"rx":0.3,"ry":0.3}]}"#).unwrap();
        assert!(!m.uses_files());
    }
}
