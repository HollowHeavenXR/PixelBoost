//! One JSON-described operation per editing tool, so the app (and the
//! CLI) drive every tool through a single entry point.

use crate::edit::{self, Adjustments};
use crate::image::Image;
use crate::resample::{resize, Filter};
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Adjust(Adjustments),
    Look { name: String, #[serde(default = "full")] amount: f32 },
    AutoEnhance,
    Rotate { clockwise: bool },
    Flip { horizontal: bool },
    /// Normalised rectangle (0..1).
    Crop { x: f32, y: f32, w: f32, h: f32 },
    Straighten { degrees: f32 },
    Resize { w: usize, h: usize },
    Lut { path: String, #[serde(default = "full")] amount: f32 },
    Denoise { amount: f32 },
    Sharpen { amount: f32, #[serde(default = "one")] radius: f32 },
    PixelArt { cell: usize, colors: usize, #[serde(default)] dither: bool, #[serde(default)] outline: bool },
    NormalMap { #[serde(default = "two")] strength: f32 },
    Seamless { #[serde(default = "quarter")] blend: f32 },
    AoBlend { path: String, #[serde(default = "full")] amount: f32 },
    DepthFog { path: String, color: [f32; 3], density: f32, #[serde(default)] start: f32 },
    /// Fill the white area of the mask image.
    Inpaint { mask: String },
    /// Normalised coordinates (0..1); `radius` is a fraction of the width.
    CloneStamp { sx: f32, sy: f32, dx: f32, dy: f32, radius: f32, #[serde(default = "half")] hardness: f32 },
    /// Apply `ops` only where the mask is white.
    Masked { mask: String, ops: Vec<Op> },
    /// Put `path` (an image with transparency) behind this one.
    Background { path: String },
    /// Solid / two-colour gradient background behind a transparent image.
    Fill { top: [f32; 3], bottom: [f32; 3] },
    /// Replace transparency with a blurred copy of `path` (fake bokeh).
    BlurredBackground { path: String, radius: f32 },
    /// Use the mask as the image's transparency (cutout).
    MaskAlpha { mask: String },
    /// Restore (white) or erase (black) parts of a cutout from the original.
    TouchUp { original: String, mask: String, restore: bool },
    Chain { ops: Vec<Op> },
}

fn full() -> f32 {
    1.0
}
fn one() -> f32 {
    1.0
}
fn two() -> f32 {
    2.0
}
fn half() -> f32 {
    0.5
}
fn quarter() -> f32 {
    0.25
}

/// Mask in [0,1] at the image's size: alpha if the mask has transparency,
/// else luma.
pub fn load_mask(path: &str, w: usize, h: usize) -> Result<Vec<f32>, String> {
    let m = Image::open(path)?;
    let m = if (m.w, m.h) != (w, h) { resize(&m, w, h, Filter::Triangle) } else { m };
    let alpha = m.has_alpha();
    Ok(m.px.iter().map(|p| if alpha { p[3] } else { Image::luma(*p) }).collect())
}

fn over(fg: &Image, bg: &Image) -> Image {
    let bg = if (bg.w, bg.h) != (fg.w, fg.h) { resize(bg, fg.w, fg.h, Filter::CatmullRom) } else { bg.clone() };
    let mut out = fg.clone();
    for (o, b) in out.px.iter_mut().zip(&bg.px) {
        let a = o[3];
        for c in 0..3 {
            o[c] = o[c] * a + b[c] * (1.0 - a);
        }
        o[3] = a + b[3] * (1.0 - a);
    }
    out
}

fn box_blur(img: &Image, r: usize) -> Image {
    // Three box passes approximate a Gaussian.
    let mut cur = img.clone();
    for _ in 0..3 {
        let f = (2 * r + 1) as f32;
        let mut tmp = cur.clone();
        for y in 0..cur.h {
            for x in 0..cur.w {
                let mut acc = [0.0f32; 4];
                for d in -(r as isize)..=r as isize {
                    let p = cur.at_clamped(x as isize + d, y as isize);
                    for c in 0..4 {
                        acc[c] += p[c];
                    }
                }
                tmp.px[y * cur.w + x] = acc.map(|v| v / f);
            }
        }
        for y in 0..cur.h {
            for x in 0..cur.w {
                let mut acc = [0.0f32; 4];
                for d in -(r as isize)..=r as isize {
                    let p = tmp.at_clamped(x as isize, y as isize + d);
                    for c in 0..4 {
                        acc[c] += p[c];
                    }
                }
                cur.px[y * cur.w + x] = acc.map(|v| v / f);
            }
        }
    }
    cur
}

pub fn apply(img: &Image, op: &Op) -> Result<Image, String> {
    let mut out = img.clone();
    match op {
        Op::Adjust(a) => edit::adjust(&mut out, a),
        Op::Look { name, amount } => {
            if !edit::LOOKS.contains(&name.as_str()) {
                return Err(format!("unknown look {name}"));
            }
            edit::apply_look(&mut out, name, *amount)
        }
        Op::AutoEnhance => edit::auto_enhance(&mut out),
        Op::Rotate { clockwise } => out = edit::rotate90(img, *clockwise),
        Op::Flip { horizontal } => out = if *horizontal { img.flip_h() } else { img.flip_v() },
        Op::Crop { x, y, w, h } => out = edit::crop_norm(img, *x, *y, *w, *h),
        Op::Straighten { degrees } => out = edit::straighten(img, *degrees),
        Op::Resize { w, h } => {
            let f = if *w < img.w { Filter::Lanczos3 } else { Filter::CatmullRom };
            out = resize(img, (*w).max(1), (*h).max(1), f)
        }
        Op::Lut { path, amount } => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            edit::apply_lut(&mut out, &edit::parse_cube(&text)?, *amount)
        }
        Op::Denoise { amount } => out = edit::denoise(img, *amount),
        Op::Sharpen { amount, radius } => edit::sharpen(&mut out, *amount, *radius),
        Op::PixelArt { cell, colors, dither, outline } => out = edit::pixel_art(img, *cell, *colors, *dither, *outline),
        Op::NormalMap { strength } => out = edit::normal_map(img, *strength),
        Op::Seamless { blend } => out = edit::seamless(img, *blend),
        Op::AoBlend { path, amount } => edit::ao_blend(&mut out, &Image::open(path)?, *amount),
        Op::DepthFog { path, color, density, start } => edit::depth_fog(&mut out, &Image::open(path)?, *color, *density, *start),
        Op::Inpaint { mask } => out = edit::inpaint(img, &load_mask(mask, img.w, img.h)?),
        Op::CloneStamp { sx, sy, dx, dy, radius, hardness } => {
            let (w, h) = (img.w as f32, img.h as f32);
            edit::clone_stamp(&mut out, sx * w, sy * h, dx * w, dy * h, radius * w, *hardness)
        }
        Op::Masked { mask, ops } => {
            let m = load_mask(mask, img.w, img.h)?;
            let mut e = img.clone();
            for o in ops {
                e = apply(&e, o)?;
            }
            if (e.w, e.h) != (img.w, img.h) {
                return Err("masked ops must keep the image size".into());
            }
            out = edit::masked_blend(img, &e, &m);
        }
        Op::Background { path } => out = over(img, &Image::open(path)?),
        Op::Fill { top, bottom } => {
            let mut bg = Image::new(img.w, img.h);
            for y in 0..img.h {
                let t = y as f32 / (img.h.max(2) - 1) as f32;
                for x in 0..img.w {
                    bg.px[y * img.w + x] = [top[0] + (bottom[0] - top[0]) * t, top[1] + (bottom[1] - top[1]) * t, top[2] + (bottom[2] - top[2]) * t, 1.0];
                }
            }
            out = over(img, &bg);
        }
        Op::BlurredBackground { path, radius } => {
            let src = Image::open(path)?;
            let small_f = 4usize;
            let small = resize(&src, (src.w / small_f).max(1), (src.h / small_f).max(1), Filter::Box);
            let b = box_blur(&small, ((*radius as usize) / small_f).max(1));
            out = over(img, &resize(&b, img.w, img.h, Filter::CatmullRom));
        }
        Op::MaskAlpha { mask } => {
            let m = load_mask(mask, img.w, img.h)?;
            for (p, a) in out.px.iter_mut().zip(m) {
                p[3] *= a.clamp(0.0, 1.0);
            }
        }
        Op::TouchUp { original, mask, restore } => {
            let o = Image::open(original)?;
            let o = if (o.w, o.h) != (img.w, img.h) { resize(&o, img.w, img.h, Filter::CatmullRom) } else { o };
            let m = load_mask(mask, img.w, img.h)?;
            for ((p, q), a) in out.px.iter_mut().zip(&o.px).zip(m) {
                if *restore {
                    for c in 0..4 {
                        p[c] += (q[c] - p[c]) * a;
                    }
                } else {
                    p[3] *= 1.0 - a;
                }
            }
        }
        Op::Chain { ops } => {
            for o in ops {
                out = apply(&out, o)?;
            }
        }
    }
    Ok(out)
}

pub fn parse(json: &str) -> Result<Op, String> {
    serde_json::from_str(json).map_err(|e| format!("bad op: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_and_runs() {
        let mut img = Image::new(16, 12);
        for (i, p) in img.px.iter_mut().enumerate() {
            *p = [(i % 16) as f32 / 15.0, 0.5, 0.2, 1.0];
        }
        for j in [
            r#"{"op":"adjust","brightness":0.1,"contrast":1.1,"saturation":1.2,"exposure":0.3,"warmth":0.2,"highlights":0,"shadows":0.2,"curve":[0,0.3,0.5,0.7,1],"vignette":0.3}"#,
            r#"{"op":"look","name":"noir"}"#,
            r#"{"op":"auto_enhance"}"#,
            r#"{"op":"rotate","clockwise":true}"#,
            r#"{"op":"crop","x":0.25,"y":0.25,"w":0.5,"h":0.5}"#,
            r#"{"op":"straighten","degrees":5}"#,
            r#"{"op":"pixel_art","cell":2,"colors":4,"dither":true,"outline":true}"#,
            r#"{"op":"chain","ops":[{"op":"sharpen","amount":0.5},{"op":"denoise","amount":0.5},{"op":"seamless"},{"op":"normal_map"}]}"#,
        ] {
            let op = parse(j).unwrap();
            let o = apply(&img, &op).unwrap();
            assert!(o.w > 0 && o.h > 0 && o.px.iter().all(|p| p.iter().all(|v| v.is_finite())), "{j}");
        }
    }
}
