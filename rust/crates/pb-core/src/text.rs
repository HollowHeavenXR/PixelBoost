//! Text layers: bundled fonts (Noto, OFL), multi-line layout with
//! alignment, letter spacing and line height, outline, soft drop shadow,
//! optional rounded background box, any rotation. Position and size are
//! normalised so the preview and the export match.

use crate::filters::{dilate_plane, gauss_plane, mix};
use crate::image::Image;
use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use rayon::prelude::*;
use serde::Deserialize;

pub struct FontInfo {
    pub id: &'static str,
    pub name: &'static str,
    data: &'static [u8],
}

pub const FONTS: &[FontInfo] = &[
    FontInfo { id: "sans", name: "Sans", data: include_bytes!("../fonts/NotoSans-Regular.ttf") },
    FontInfo { id: "heavy", name: "Heavy", data: include_bytes!("../fonts/NotoSans-Black.ttf") },
    FontInfo { id: "serif", name: "Serif", data: include_bytes!("../fonts/NotoSerif-Bold.ttf") },
    FontInfo { id: "display", name: "Display", data: include_bytes!("../fonts/NotoSerifDisplay-BlackItalic.ttf") },
    FontInfo { id: "mono", name: "Mono", data: include_bytes!("../fonts/NotoSansMono-Bold.ttf") },
];

#[derive(Deserialize, Debug, Clone)]
pub struct TextSpec {
    pub text: String,
    /// Centre of the text block, normalised.
    pub x: f32,
    pub y: f32,
    /// Font size as a fraction of the image width.
    pub size: f32,
    #[serde(default = "sans")]
    pub font: String,
    #[serde(default = "white")]
    pub color: [f32; 4],
    /// "left", "center" or "right" (within the block).
    #[serde(default = "center")]
    pub align: String,
    /// Degrees, clockwise.
    #[serde(default)]
    pub rotation: f32,
    /// Outline width as a fraction of the font size.
    #[serde(default)]
    pub outline: f32,
    #[serde(default = "black")]
    pub outline_color: [f32; 4],
    /// Drop shadow strength 0..1.
    #[serde(default)]
    pub shadow: f32,
    /// Box behind the text (alpha 0 = none).
    #[serde(default)]
    pub background: [f32; 4],
    /// Extra space between letters, in ems.
    #[serde(default)]
    pub letter_spacing: f32,
    #[serde(default = "line_height")]
    pub line_height: f32,
}

fn sans() -> String {
    "sans".into()
}
fn white() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}
fn black() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
}
fn center() -> String {
    "center".into()
}
fn line_height() -> f32 {
    1.15
}

fn font_for(id: &str) -> FontRef<'static> {
    let f = FONTS.iter().find(|f| f.id == id).unwrap_or(&FONTS[0]);
    FontRef::try_from_slice(f.data).expect("bundled font")
}

/// Rendered text block: coverage planes in a local box.
struct Block {
    w: usize,
    h: usize,
    fill: Vec<f32>,
    outline: Option<Vec<f32>>,
    shadow: Option<(Vec<f32>, f32, f32)>,
    bg: Option<Vec<f32>>,
}

/// Octagonal dilation (square then diamond) approximating a disc of `r` px.
fn dilate_disc(p: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    let a = (r * 0.4142).round() as usize;
    let b = (r * 0.5858).round() as usize;
    let mut cur = dilate_plane(p, w, h, a);
    for _ in 0..b {
        let prev = cur.clone();
        cur.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let mut m = prev[y * w + x];
                if x > 0 {
                    m = m.max(prev[y * w + x - 1]);
                }
                if x + 1 < w {
                    m = m.max(prev[y * w + x + 1]);
                }
                if y > 0 {
                    m = m.max(prev[(y - 1) * w + x]);
                }
                if y + 1 < h {
                    m = m.max(prev[(y + 1) * w + x]);
                }
                row[x] = m;
            }
        });
    }
    cur
}

fn layout(spec: &TextSpec, size_px: f32) -> Option<Block> {
    let font = font_for(&spec.font);
    let scale = PxScale::from(size_px);
    let sf = font.as_scaled(scale);
    let lines: Vec<&str> = spec.text.split('\n').collect();
    let spacing = spec.letter_spacing * size_px;
    let line_h = (sf.ascent() - sf.descent()) * spec.line_height.max(0.5);
    let widths: Vec<f32> = lines
        .iter()
        .map(|l| {
            let mut x = 0.0f32;
            let mut prev = None;
            for ch in l.chars() {
                let id = sf.glyph_id(ch);
                if let Some(p) = prev {
                    x += sf.kern(p, id);
                }
                x += sf.h_advance(id) + spacing;
                prev = Some(id);
            }
            (x - spacing).max(0.0)
        })
        .collect();
    let max_w = widths.iter().copied().fold(0.0f32, f32::max);
    if max_w <= 0.0 {
        return None;
    }
    let text_h = line_h * (lines.len() as f32 - 1.0) + (sf.ascent() - sf.descent());
    let outline_px = spec.outline.max(0.0) * size_px;
    let shadow_px = spec.shadow.clamp(0.0, 1.0) * size_px * 0.12;
    let bg_pad = if spec.background[3] > 0.0 { size_px * 0.35 } else { 0.0 };
    let pad = (outline_px + shadow_px * 4.0 + bg_pad + 3.0).ceil();
    let w = (max_w + 2.0 * pad).ceil() as usize;
    let h = (text_h + 2.0 * pad).ceil() as usize;
    if w * h > 80_000_000 {
        return None;
    }
    let mut fill = vec![0.0f32; w * h];
    for (li, l) in lines.iter().enumerate() {
        let lw = widths[li];
        let mut x = pad
            + match spec.align.as_str() {
                "left" => 0.0,
                "right" => max_w - lw,
                _ => (max_w - lw) / 2.0,
            };
        let base = pad + sf.ascent() + line_h * li as f32;
        let mut prev = None;
        for ch in l.chars() {
            let id = sf.glyph_id(ch);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            let g = id.with_scale_and_position(scale, point(x, base));
            if let Some(og) = font.outline_glyph(g) {
                let bb = og.px_bounds();
                og.draw(|gx, gy, c| {
                    let px = bb.min.x as i64 + gx as i64;
                    let py = bb.min.y as i64 + gy as i64;
                    if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        let o = &mut fill[py as usize * w + px as usize];
                        *o = (*o + c).min(1.0);
                    }
                });
            }
            x += sf.h_advance(id) + spacing;
            prev = Some(id);
        }
    }
    let outline = if outline_px >= 0.5 {
        let d = dilate_disc(&fill, w, h, outline_px);
        Some(gauss_plane(&d, w, h, 0.6))
    } else {
        None
    };
    let shadow = if shadow_px > 0.0 {
        let src = outline.as_ref().unwrap_or(&fill);
        Some((gauss_plane(src, w, h, shadow_px * 1.5), shadow_px, shadow_px))
    } else {
        None
    };
    let bg = if bg_pad > 0.0 {
        let (x0, y0, x1, y1) = (pad - bg_pad, pad - bg_pad, pad + max_w + bg_pad, pad + text_h + bg_pad);
        let r = size_px * 0.3;
        let (cx, cy, hx, hy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0) / 2.0, (y1 - y0) / 2.0);
        Some(
            (0..w * h)
                .into_par_iter()
                .map(|i| {
                    let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                    let qx = (x - cx).abs() - hx + r;
                    let qy = (y - cy).abs() - hy + r;
                    let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
                    (0.5 - d).clamp(0.0, 1.0)
                })
                .collect(),
        )
    } else {
        None
    };
    Some(Block { w, h, fill, outline, shadow, bg })
}

#[inline]
fn sample(p: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    if x < -1.0 || y < -1.0 || x > w as f32 || y > h as f32 {
        return 0.0;
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let (fx, fy) = (x - x0, y - y0);
    let at = |xx: f32, yy: f32| -> f32 {
        if xx < 0.0 || yy < 0.0 || xx >= w as f32 || yy >= h as f32 {
            0.0
        } else {
            p[yy as usize * w + xx as usize]
        }
    };
    let a = at(x0, y0) + (at(x0 + 1.0, y0) - at(x0, y0)) * fx;
    let b = at(x0, y0 + 1.0) + (at(x0 + 1.0, y0 + 1.0) - at(x0, y0 + 1.0)) * fx;
    a + (b - a) * fy
}

fn over(p: &mut [f32; 4], c: [f32; 4], a: f32) {
    let a = (a * c[3]).clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    for k in 0..3 {
        p[k] = mix(p[k], c[k], a);
    }
    p[3] = p[3] + (1.0 - p[3]) * a;
}

pub fn draw_text(img: &mut Image, spec: &TextSpec) {
    if spec.text.trim().is_empty() {
        return;
    }
    let size_px = (spec.size * img.w as f32).max(2.0);
    let Some(b) = layout(spec, size_px) else { return };
    let (cx, cy) = (spec.x * img.w as f32, spec.y * img.h as f32);
    let (s, c) = spec.rotation.to_radians().sin_cos();
    // Bounding box of the rotated block in the image.
    let (hw, hh) = (b.w as f32 / 2.0, b.h as f32 / 2.0);
    let ex = hw * c.abs() + hh * s.abs();
    let ey = hw * s.abs() + hh * c.abs();
    let x0 = (cx - ex).floor().max(0.0) as usize;
    let y0 = (cy - ey).floor().max(0.0) as usize;
    let x1 = ((cx + ex).ceil().max(0.0) as usize).min(img.w);
    let y1 = ((cy + ey).ceil().max(0.0) as usize).min(img.h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let iw = img.w;
    img.px[y0 * iw..y1 * iw].par_chunks_mut(iw).enumerate().for_each(|(j, row)| {
        let y = (y0 + j) as f32 + 0.5 - cy;
        for xi in x0..x1 {
            let x = xi as f32 + 0.5 - cx;
            // Inverse rotation into the block.
            let u = x * c + y * s + hw - 0.5;
            let v = -x * s + y * c + hh - 0.5;
            let p = &mut row[xi];
            if let Some(bg) = &b.bg {
                over(p, spec.background, sample(bg, b.w, b.h, u, v));
            }
            if let Some((sh, ox, oy)) = &b.shadow {
                over(p, [0.0, 0.0, 0.0, 0.65 * spec.shadow.clamp(0.0, 1.0).sqrt()], sample(sh, b.w, b.h, u - ox, v - oy));
            }
            if let Some(o) = &b.outline {
                over(p, spec.outline_color, sample(o, b.w, b.h, u, v));
            }
            over(p, spec.color, sample(&b.fill, b.w, b.h, u, v));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_draws_in_place() {
        let mut img = Image::new(200, 100);
        let spec: TextSpec = serde_json::from_str(r#"{"text":"HI\nthere","x":0.5,"y":0.5,"size":0.2,"color":[1,0,0,1],"outline":0.05,"shadow":0.5,"background":[0,0,1,0.5],"rotation":10}"#).unwrap();
        draw_text(&mut img, &spec);
        let red = img.px.iter().filter(|p| p[0] > 0.8 && p[1] < 0.2).count();
        assert!(red > 100, "{red}");
        // Corners untouched.
        assert_eq!(img.at(0, 0), [0.0, 0.0, 0.0, 1.0]);
        for f in FONTS {
            let mut i2 = Image::new(120, 40);
            draw_text(&mut i2, &TextSpec { font: f.id.into(), ..spec.clone() });
            assert!(i2.px.iter().all(|p| p.iter().all(|v| v.is_finite())));
        }
    }
}
