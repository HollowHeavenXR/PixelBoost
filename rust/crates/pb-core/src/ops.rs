//! One JSON-described operation per editing tool, so the app (and the
//! CLI) drive every tool through a single entry point.

use crate::compose;
use crate::develop::{self, Curves, Grade, Hsl};
use crate::draw::{self, Mark};
use crate::edit::{self, Adjustments};
use crate::fx;
use crate::image::Image;
use crate::mask::{self, MaskSpec};
use crate::text::{self, TextSpec};
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

    // ---- photo editor tools (5.2)
    /// HSL colour mixer.
    Hsl(Hsl),
    /// Colour grading wheels.
    Grade(Grade),
    /// Master + per-channel tone curves.
    Curves(Curves),
    /// Apply `ops` through a gradient / radial / range / painted mask.
    Local {
        mask: MaskSpec,
        ops: Vec<Op>,
        #[serde(default = "full")]
        amount: f32,
    },
    /// Gaussian blur, `radius` as a fraction of the width.
    Blur { radius: f32 },
    TiltShift {
        #[serde(default = "half")]
        y: f32,
        #[serde(default)]
        angle: f32,
        #[serde(default = "fifth")]
        band: f32,
        #[serde(default = "fifth")]
        feather: f32,
        #[serde(default = "blur_r")]
        radius: f32,
    },
    ZoomBlur {
        #[serde(default = "half")]
        cx: f32,
        #[serde(default = "half")]
        cy: f32,
        amount: f32,
    },
    SpinBlur {
        #[serde(default = "half")]
        cx: f32,
        #[serde(default = "half")]
        cy: f32,
        degrees: f32,
    },
    MotionBlur { angle: f32, distance: f32 },
    /// Bokeh blur; `keep` marks what stays sharp (portrait mode).
    LensBlur {
        #[serde(default = "blur_r")]
        radius: f32,
        #[serde(default = "half")]
        bokeh: f32,
        #[serde(default)]
        keep: Option<MaskSpec>,
    },
    Bloom {
        #[serde(default = "half")]
        amount: f32,
        #[serde(default = "blur_r")]
        radius: f32,
        #[serde(default = "threshold")]
        threshold: f32,
    },
    /// Creative effect by name (see `fx::EFFECTS`).
    Effect {
        name: String,
        #[serde(default = "full")]
        amount: f32,
        #[serde(default = "half")]
        size: f32,
        #[serde(default = "fx_colors")]
        colors: [[f32; 3]; 2],
    },
    /// Edge-preserving smoothing (skin retouch).
    Smooth {
        amount: f32,
        #[serde(default = "smooth_r")]
        radius: f32,
    },
    /// Strokes, lines, arrows, shapes.
    Draw { marks: Vec<Mark> },
    Text(TextSpec),
    /// Image layer (sticker, logo, double exposure) with a blend mode.
    Overlay {
        path: String,
        #[serde(default = "half")]
        x: f32,
        #[serde(default = "half")]
        y: f32,
        #[serde(default = "third")]
        w: f32,
        #[serde(default)]
        rotation: f32,
        #[serde(default = "full")]
        opacity: f32,
        #[serde(default = "normal")]
        blend: String,
        #[serde(default)]
        fit: Option<String>,
    },
    /// Border / polaroid / film / pad-to-ratio / rounded / shadow card.
    Frame {
        style: String,
        #[serde(default = "frame_w")]
        width: f32,
        #[serde(default = "white4")]
        color: [f32; 4],
        #[serde(default)]
        ratio: f32,
        #[serde(default = "frame_w")]
        radius: f32,
    },
    /// Crop to a shape (circle, hexagon, heart...) with optional rings.
    ShapeCrop(crate::shape::ShapeCrop),
    /// Keystone correction, each -1..1.
    Perspective {
        #[serde(default)]
        vertical: f32,
        #[serde(default)]
        horizontal: f32,
    },
}

fn fifth() -> f32 {
    0.2
}
fn third() -> f32 {
    0.33
}
fn blur_r() -> f32 {
    0.02
}
fn smooth_r() -> f32 {
    0.008
}
fn threshold() -> f32 {
    0.7
}
fn frame_w() -> f32 {
    0.05
}
fn normal() -> String {
    "normal".into()
}
fn white4() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}
fn fx_colors() -> [[f32; 3]; 2] {
    [[0.10, 0.08, 0.25], [1.0, 0.82, 0.55]]
}

impl Op {
    /// True when the op reads files (masks, LUTs, overlays...), so it can't
    /// be saved in a preset or replayed on another photo.
    pub fn uses_files(&self) -> bool {
        match self {
            Op::Lut { .. }
            | Op::AoBlend { .. }
            | Op::DepthFog { .. }
            | Op::Inpaint { .. }
            | Op::Masked { .. }
            | Op::Background { .. }
            | Op::BlurredBackground { .. }
            | Op::MaskAlpha { .. }
            | Op::TouchUp { .. }
            | Op::Overlay { .. }
            | Op::CloneStamp { .. } => true,
            Op::Chain { ops } => ops.iter().any(|o| o.uses_files()),
            Op::Local { mask, ops, .. } => mask.uses_files() || ops.iter().any(|o| o.uses_files()),
            Op::LensBlur { keep: Some(m), .. } => m.uses_files(),
            _ => false,
        }
    }

    /// True when the op changes the image size (or crops/rotates), so
    /// masks recorded after it won't line up if it's toggled.
    pub fn changes_geometry(&self) -> bool {
        match self {
            Op::Rotate { .. } | Op::Crop { .. } | Op::Straighten { .. } | Op::Resize { .. } | Op::Frame { .. } | Op::Perspective { .. } | Op::Flip { .. } | Op::ShapeCrop(_) => true,
            Op::Chain { ops } => ops.iter().any(|o| o.changes_geometry()),
            _ => false,
        }
    }
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
            let b = crate::filters::gauss(&small, (*radius / small_f as f32).max(1.0));
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
        Op::Hsl(p) => develop::hsl(&mut out, p),
        Op::Grade(g) => develop::grade(&mut out, g),
        Op::Curves(c) => develop::curves(&mut out, c),
        Op::Local { mask: spec, ops, amount } => {
            let mut e = img.clone();
            for o in ops {
                e = apply(&e, o)?;
            }
            if (e.w, e.h) != (img.w, img.h) {
                return Err("local adjustments must keep the image size".into());
            }
            let mut m = mask::render(spec, img)?;
            if *amount < 1.0 {
                let a = amount.clamp(0.0, 1.0);
                m.iter_mut().for_each(|v| *v *= a);
            }
            out = edit::masked_blend(img, &e, &m);
        }
        Op::Blur { radius } => out = fx::blur(img, *radius),
        Op::TiltShift { y, angle, band, feather, radius } => out = fx::tilt_shift(img, *y, *angle, *band, *feather, *radius),
        Op::ZoomBlur { cx, cy, amount } => out = fx::zoom_blur(img, *cx, *cy, *amount),
        Op::SpinBlur { cx, cy, degrees } => out = fx::spin_blur(img, *cx, *cy, *degrees),
        Op::MotionBlur { angle, distance } => out = fx::motion_blur(img, *angle, *distance),
        Op::LensBlur { radius, bokeh, keep } => {
            let k = match keep {
                Some(spec) => Some(mask::render(spec, img)?),
                None => None,
            };
            out = fx::lens_blur(img, *radius, *bokeh, k.as_deref())
        }
        Op::Bloom { amount, radius, threshold } => out = fx::bloom(img, *amount, *radius, *threshold),
        Op::Effect { name, amount, size, colors } => out = fx::effect(img, name, *amount, *size, *colors)?,
        Op::Smooth { amount, radius } => out = fx::smooth(img, *amount, *radius),
        Op::Draw { marks } => draw::draw(&mut out, marks),
        Op::Text(t) => text::draw_text(&mut out, t),
        Op::Overlay { path, x, y, w, rotation, opacity, blend, fit } => {
            out = compose::overlay(img, &Image::open(path)?, *x, *y, *w, *rotation, *opacity, blend, fit.as_deref())
        }
        Op::Frame { style, width, color, ratio, radius } => out = compose::frame(img, style, *width, *color, *ratio, *radius)?,
        Op::Perspective { vertical, horizontal } => out = compose::perspective(img, *vertical, *horizontal),
        Op::ShapeCrop(s) => out = crate::shape::render(img, s)?,
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
            r#"{"op":"adjust","tint":0.3,"vibrance":0.5,"whites":0.2,"blacks":-0.2,"fade":0.3,"clarity":0.5,"texture":0.4,"dehaze":0.5,"grain":0.4}"#,
            r#"{"op":"adjust","exposure":0.5}"#,
            r#"{"op":"hsl","hue":[0.2,0,0,0,0,0,0,0],"sat":[0,0,0,-1,0,0,0,0]}"#,
            r#"{"op":"grade","shadows":{"hue":220,"sat":0.4},"highlights":{"hue":40,"sat":0.3,"lum":0.1}}"#,
            r#"{"op":"curves","master":[0,0.2,0.5,0.8,1],"blue":[0.1,0.5,0.9]}"#,
            r#"{"op":"local","mask":{"kind":"linear","x0":0.5,"y0":0,"x1":0.5,"y1":0.6},"ops":[{"op":"adjust","exposure":-1}]}"#,
            r#"{"op":"local","mask":{"kind":"radial","cx":0.5,"cy":0.5,"rx":0.3,"ry":0.3},"ops":[{"op":"effect","name":"mosaic"}],"amount":0.5}"#,
            r#"{"op":"blur","radius":0.05}"#,
            r#"{"op":"tilt_shift"}"#,
            r#"{"op":"zoom_blur","amount":0.4}"#,
            r#"{"op":"spin_blur","degrees":15}"#,
            r#"{"op":"motion_blur","angle":20,"distance":0.1}"#,
            r#"{"op":"lens_blur","keep":{"kind":"radial","cx":0.5,"cy":0.5,"rx":0.2,"ry":0.3}}"#,
            r#"{"op":"bloom"}"#,
            r#"{"op":"effect","name":"oil","size":0.3}"#,
            r#"{"op":"smooth","amount":0.6}"#,
            r#"{"op":"draw","marks":[{"kind":"stroke","points":[[0.1,0.1],[0.9,0.9]],"width":0.05,"color":[1,1,0,1],"style":"highlighter"}]}"#,
            r#"{"op":"text","text":"Hello","x":0.5,"y":0.5,"size":0.3,"font":"heavy","outline":0.08}"#,
            r#"{"op":"frame","style":"polaroid"}"#,
            r#"{"op":"perspective","vertical":0.5}"#,
            r#"{"op":"shape_crop","shape":"heart","rings":[{"style":"story","width":0.05}]}"#,
            r#"{"op":"shape_crop","shape":"hexagon","x":0.1,"y":0.1,"w":0.5,"h":0.5,"rotation":30,"background":[1,1,1,1]}"#,
        ] {
            let op = parse(j).unwrap();
            let o = apply(&img, &op).unwrap();
            assert!(o.w > 0 && o.h > 0 && o.px.iter().all(|p| p.iter().all(|v| v.is_finite())), "{j}");
        }
    }
    #[test]
    fn portability() {
        assert!(!parse(r#"{"op":"chain","ops":[{"op":"adjust"},{"op":"hsl"}]}"#).unwrap().uses_files());
        assert!(parse(r#"{"op":"local","mask":{"kind":"file","path":"/x.png"},"ops":[]}"#).unwrap().uses_files());
        assert!(parse(r#"{"op":"crop","x":0,"y":0,"w":0.5,"h":0.5}"#).unwrap().changes_geometry());
    }
}
