//! Memory-bounded upscaling for big photos.
//!
//! The whole chain — network (with ensemble), fidelity refinement, detail
//! boost and the final resample to the requested size — runs per tile on
//! the tile plus a context margin, and only the tile's inner part is
//! written, straight into an 8-bit output. Peak memory is the 8-bit output
//! plus one tile's working set, so a 5 MP photo going to 86 MP needs ~0.4 GB
//! instead of several GB of float buffers.

use crate::engine::Engine;
use crate::enhance::sharpen_detail;
use crate::fidelity::refine;
use crate::image::{Format, Image};
use crate::pipeline::{bleed_transparent, Report, Settings};
use crate::resample::{shrink_box, Filter};
use rayon::prelude::*;
use std::time::Instant;

/// Straight-alpha RGBA, 8 bits per channel.
#[derive(Clone)]
pub struct Rgba8 {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

impl Rgba8 {
    pub fn new(w: usize, h: usize) -> Self {
        Rgba8 { w, h, data: vec![255; w * h * 4] }
    }

    pub fn region(&self, x: usize, y: usize, w: usize, h: usize) -> Image {
        let mut img = Image::new(w, h);
        for yy in 0..h {
            let row = &self.data[((y + yy) * self.w + x) * 4..((y + yy) * self.w + x + w) * 4];
            for (xx, c) in row.chunks_exact(4).enumerate() {
                img.px[yy * w + xx] = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0];
            }
        }
        img
    }

    pub fn crop(&self, x: usize, y: usize, w: usize, h: usize) -> Rgba8 {
        let mut data = Vec::with_capacity(w * h * 4);
        for yy in y..y + h {
            data.extend_from_slice(&self.data[(yy * self.w + x) * 4..(yy * self.w + x + w) * 4]);
        }
        Rgba8 { w, h, data }
    }

    pub fn has_alpha(&self) -> bool {
        self.data.par_chunks_exact(4).any(|c| c[3] < 255)
    }

    /// Write PNG (row-streamed, RGB when opaque) or JPEG.
    pub fn save(&self, path: &str, quality: u8) -> Result<(), String> {
        let f = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
        let mut wtr = std::io::BufWriter::with_capacity(1 << 20, f);
        match Format::from_path(path) {
            Format::Jpeg => {
                use image::ImageEncoder;
                let rgb: Vec<u8> = self.data.chunks_exact(4).flat_map(|c| [c[0], c[1], c[2]]).collect();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut wtr, quality)
                    .write_image(&rgb, self.w as u32, self.h as u32, image::ExtendedColorType::Rgb8)
                    .map_err(|e| e.to_string())?;
            }
            Format::Webp => {
                use image::ImageEncoder;
                image::codecs::webp::WebPEncoder::new_lossless(&mut wtr)
                    .write_image(&self.data, self.w as u32, self.h as u32, image::ExtendedColorType::Rgba8)
                    .map_err(|e| e.to_string())?;
            }
            Format::Png => {
                let alpha = self.has_alpha();
                let mut enc = png::Encoder::new(&mut wtr, self.w as u32, self.h as u32);
                enc.set_color(if alpha { png::ColorType::Rgba } else { png::ColorType::Rgb });
                enc.set_depth(png::BitDepth::Eight);
                enc.set_compression(png::Compression::Fast);
                let mut w = enc.write_header().map_err(|e| e.to_string())?;
                let mut sw = w.stream_writer().map_err(|e| e.to_string())?;
                use std::io::Write;
                let mut row = Vec::with_capacity(self.w * 4);
                for y in 0..self.h {
                    let src = &self.data[y * self.w * 4..(y + 1) * self.w * 4];
                    row.clear();
                    if alpha {
                        row.extend_from_slice(src);
                    } else {
                        for c in src.chunks_exact(4) {
                            row.extend_from_slice(&c[..3]);
                        }
                    }
                    sw.write_all(&row).map_err(|e| e.to_string())?;
                }
                sw.finish().map_err(|e| e.to_string())?;
            }
        }
        use std::io::Write;
        wtr.flush().map_err(|e| e.to_string())
    }
}

/// Where tiles read their source pixels from.
pub trait Source: Sync {
    fn size(&self) -> (usize, usize);
    fn region(&self, x: usize, y: usize, w: usize, h: usize) -> Image;
}

impl Source for Image {
    fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }
    fn region(&self, x: usize, y: usize, w: usize, h: usize) -> Image {
        self.crop(x, y, w, h)
    }
}

impl Source for Rgba8 {
    fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }
    fn region(&self, x: usize, y: usize, w: usize, h: usize) -> Image {
        Rgba8::region(self, x, y, w, h)
    }
}

pub struct StreamEvent<'a> {
    pub done: usize,
    pub total: usize,
    /// Output pixels written: x, y, w, h (in final-output coordinates; for
    /// the first of two passes, scaled to them).
    pub rect: [usize; 4],
    pub pixels: &'a Rgba8,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Largest output, in pixels; the scale is reduced to fit.
    pub max_output_px: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_output_px: 120_000_000 }
    }
}

/// Context margin around each tile, in source pixels: covers the network's
/// receptive field plus the fidelity and detail neighbourhoods.
const MARGIN: usize = 24;

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

struct PassOut<'a> {
    out: &'a mut Rgba8,
    /// For the live view of a first pass: factor from this pass's output to
    /// the final output.
    view_scale: f32,
}

#[allow(clippy::too_many_arguments)]
fn run_pass(
    engine: &Engine,
    src: &dyn Source,
    st: &Settings,
    dst: PassOut<'_>,
    done: &mut usize,
    total: usize,
    se: &mut Option<(f64, usize)>,
    progress: &mut dyn FnMut(StreamEvent<'_>) -> bool,
) -> bool {
    let s = engine.scale();
    let (w, h) = src.size();
    let out = dst.out;
    let (ow, oh) = (out.w, out.h);
    let (mw, mh) = (w * s, h * s);
    let exact = ow == mw && oh == mh;
    let filter = if ow < mw { Filter::Lanczos3 } else { Filter::CatmullRom };
    let tile = st.tile.clamp(32, 512);
    let n_ens = match st.ensemble {
        0 | 1 => 1,
        2 => 2,
        3 | 4 => 4,
        _ => 8,
    };
    let order = [0usize, 1, 4, 6, 2, 3, 5, 7];
    let fid = st.fidelity.params();
    for ty in (0..h).step_by(tile) {
        for tx in (0..w).step_by(tile) {
            let (iw, ih) = (tile.min(w - tx), tile.min(h - ty));
            let rx = tx.saturating_sub(MARGIN);
            let ry = ty.saturating_sub(MARGIN);
            let rw = (tx + iw + MARGIN).min(w) - rx;
            let rh = (ty + ih + MARGIN).min(h) - ry;
            let reg = src.region(rx, ry, rw, rh);
            // Network, averaged over flipped / rotated copies.
            let mut up: Option<Image> = None;
            for &k in order.iter().take(n_ens) {
                let o = engine.forward(&reg.dihedral(k)).dihedral_inverse(k);
                up = Some(match up {
                    None => o,
                    Some(mut a) => {
                        a.px.par_iter_mut().zip(o.px.par_iter()).for_each(|(a, b)| {
                            for c in 0..3 {
                                a[c] += b[c];
                            }
                        });
                        a
                    }
                });
            }
            let mut up = up.unwrap();
            if n_ens > 1 {
                let inv = 1.0 / n_ens as f32;
                up.px.par_iter_mut().for_each(|a| {
                    for c in 0..3 {
                        a[c] *= inv;
                    }
                });
            }
            if let Some(p) = fid {
                refine(&reg, &mut up, s, p);
            }
            sharpen_detail(&mut up, st.detail, (s as f32 / 4.0).max(0.5));
            // Fidelity score over the inner part.
            if let Some((acc, n)) = se.as_mut() {
                let down = shrink_box(&up, s);
                for y in (ty - ry)..(ty - ry + ih) {
                    for x in (tx - rx)..(tx - rx + iw) {
                        let d = (Image::luma(reg.at(x, y)) - Image::luma(down.at(x, y))) as f64;
                        *acc += d * d;
                        *n += 1;
                    }
                }
            }
            // Write the inner part.
            let (ox, oy, rw_o, rh_o);
            if exact {
                ox = tx * s;
                oy = ty * s;
                rw_o = iw * s;
                rh_o = ih * s;
                let (lx, ly) = ((tx - rx) * s, (ty - ry) * s);
                out.data[(oy * ow) * 4..((oy + rh_o) * ow) * 4].par_chunks_mut(ow * 4).enumerate().for_each(|(y, row)| {
                    for x in 0..rw_o {
                        let p = up.px[(ly + y) * up.w + lx + x];
                        let d = &mut row[(ox + x) * 4..(ox + x) * 4 + 4];
                        d[0] = to_u8(p[0]);
                        d[1] = to_u8(p[1]);
                        d[2] = to_u8(p[2]);
                        d[3] = 255;
                    }
                });
            } else {
                let rxs = ow as f64 / mw as f64;
                let rys = oh as f64 / mh as f64;
                let first = |a: usize, r: f64, lim: usize| (((a * s) as f64 * r - 0.5).ceil().max(0.0) as usize).min(lim);
                let (x0, x1) = (first(tx, rxs, ow), first(tx + iw, rxs, ow));
                let (y0, y1) = (first(ty, rys, oh), first(ty + ih, rys, oh));
                ox = x0;
                oy = y0;
                rw_o = x1 - x0;
                rh_o = y1 - y0;
                if rw_o > 0 && rh_o > 0 {
                    let hx = axis_taps(x0, x1, mw, ow, (rx * s) as isize, up.w, filter);
                    let hy = axis_taps(y0, y1, mh, oh, (ry * s) as isize, up.h, filter);
                    // Horizontal over the rows the vertical taps need.
                    let (rmin, rmax) = (hy.min_idx, hy.max_idx);
                    let mut mid = vec![[0.0f32; 3]; (rmax - rmin + 1) * rw_o];
                    mid.par_chunks_mut(rw_o).enumerate().for_each(|(r, row)| {
                        let src = &up.px[(rmin + r) * up.w..(rmin + r + 1) * up.w];
                        for (i, o) in row.iter_mut().enumerate() {
                            let mut acc = [0.0f32; 3];
                            for (&j, &wt) in hx.idx[i].iter().zip(&hx.w[i]) {
                                let p = src[j];
                                acc[0] += wt * p[0];
                                acc[1] += wt * p[1];
                                acc[2] += wt * p[2];
                            }
                            *o = acc;
                        }
                    });
                    out.data[(oy * ow) * 4..((oy + rh_o) * ow) * 4].par_chunks_mut(ow * 4).enumerate().for_each(|(y, row)| {
                        for x in 0..rw_o {
                            let mut acc = [0.0f32; 3];
                            for (&j, &wt) in hy.idx[y].iter().zip(&hy.w[y]) {
                                let p = mid[(j - rmin) * rw_o + x];
                                acc[0] += wt * p[0];
                                acc[1] += wt * p[1];
                                acc[2] += wt * p[2];
                            }
                            let d = &mut row[(ox + x) * 4..(ox + x) * 4 + 4];
                            d[0] = to_u8(acc[0]);
                            d[1] = to_u8(acc[1]);
                            d[2] = to_u8(acc[2]);
                            d[3] = 255;
                        }
                    });
                }
            }
            *done += 1;
            let tile_px = out.crop(ox, oy, rw_o, rh_o);
            let vs = dst.view_scale;
            let rect = [(ox as f32 * vs) as usize, (oy as f32 * vs) as usize, (rw_o as f32 * vs).ceil() as usize, (rh_o as f32 * vs).ceil() as usize];
            if !progress(StreamEvent { done: *done, total, rect, pixels: &tile_px }) {
                return false;
            }
        }
    }
    true
}

struct AxisTaps {
    idx: Vec<Vec<usize>>,
    w: Vec<Vec<f32>>,
    min_idx: usize,
    max_idx: usize,
}

/// Filter taps for output indices `o0..o1` of an axis that maps `m` model
/// pixels to `o` output pixels, as indices into a tile buffer starting at
/// model coordinate `origin` with length `len`.
fn axis_taps(o0: usize, o1: usize, m: usize, o: usize, origin: isize, len: usize, f: Filter) -> AxisTaps {
    let scale = o as f32 / m as f32;
    let fs = if scale < 1.0 { 1.0 / scale } else { 1.0 };
    let support = f.support() * fs;
    let mut idx = Vec::with_capacity(o1 - o0);
    let mut w = Vec::with_capacity(o1 - o0);
    let (mut lo, mut hi) = (usize::MAX, 0);
    for i in o0..o1 {
        let center = (i as f32 + 0.5) / scale - 0.5;
        let a = (center - support).ceil() as isize;
        let b = (center + support).floor() as isize;
        let mut ii = Vec::new();
        let mut ww = Vec::new();
        let mut sum = 0.0;
        for k in a..=b {
            let v = f.eval((k as f32 - center) / fs);
            if v == 0.0 {
                continue;
            }
            // Clamp to the image (edge) and then to the tile buffer.
            let g = k.clamp(0, m as isize - 1);
            let l = (g - origin).clamp(0, len as isize - 1) as usize;
            ii.push(l);
            ww.push(v);
            sum += v;
            lo = lo.min(l);
            hi = hi.max(l);
        }
        if sum.abs() > 1e-8 {
            ww.iter_mut().for_each(|v| *v /= sum);
        }
        idx.push(ii);
        w.push(ww);
    }
    if lo == usize::MAX {
        lo = 0;
    }
    AxisTaps { idx, w, min_idx: lo, max_idx: hi }
}

/// Upscale `img` per `st`, bounded by `lim`. Returns the 8-bit result.
pub fn upscale(engine: &Engine, img: &Image, st: &Settings, lim: Limits, progress: &mut dyn FnMut(StreamEvent<'_>) -> bool) -> Option<(Rgba8, Report, bool)> {
    let t0 = Instant::now();
    let had_alpha = img.has_alpha();
    let px = (img.w * img.h) as f64;
    let mut target = st.scale.clamp(1.0, 16.0) as f64;
    let mut capped = false;
    if px * target * target > lim.max_output_px as f64 {
        target = (lim.max_output_px as f64 / px).sqrt().max(1.0);
        capped = true;
    }
    let (tw, th) = (((img.w as f64) * target).round().max(1.0) as usize, ((img.h as f64) * target).round().max(1.0) as usize);
    let s = engine.scale();
    let rgb = if had_alpha { bleed_transparent(img) } else { img.clone() };
    let two = target > s as f64 * 1.5;
    let tiles = |w: usize, h: usize| w.div_ceil(st.tile.clamp(32, 512)) * h.div_ceil(st.tile.clamp(32, 512));
    let total = tiles(img.w, img.h) + if two { tiles(img.w * s, img.h * s) } else { 0 };
    let mut done = 0;
    let mut se = Some((0.0f64, 0usize));
    let mut out = Rgba8::new(tw, th);
    if two {
        let mut mid = Rgba8::new(img.w * s, img.h * s);
        let vs = tw as f32 / mid.w as f32;
        if !run_pass(engine, &rgb, st, PassOut { out: &mut mid, view_scale: vs }, &mut done, total, &mut se, progress) {
            return None;
        }
        drop(rgb);
        let mut none = None;
        if !run_pass(engine, &mid, st, PassOut { out: &mut out, view_scale: 1.0 }, &mut done, total, &mut none, progress) {
            return None;
        }
    } else if !run_pass(engine, &rgb, st, PassOut { out: &mut out, view_scale: 1.0 }, &mut done, total, &mut se, progress) {
        return None;
    }
    if had_alpha {
        // Alpha: bicubic from the source alpha.
        let (sx, sy) = (img.w as f32 / tw as f32, img.h as f32 / th as f32);
        out.data.par_chunks_mut(tw * 4).enumerate().for_each(|(y, row)| {
            let v = (y as f32 + 0.5) * sy - 0.5;
            for x in 0..tw {
                let u = (x as f32 + 0.5) * sx - 0.5;
                row[x * 4 + 3] = to_u8(sample_alpha(img, u, v));
            }
        });
    }
    let fidelity_db = se.map(|(acc, n)| if n == 0 || acc <= 1e-12 { 99.0 } else { 10.0 * ((n as f64) / acc).log10() }).unwrap_or(0.0);
    let report = Report {
        model: engine.info.id.to_string(),
        in_w: img.w,
        in_h: img.h,
        out_w: tw,
        out_h: th,
        passes: if two { 2 } else { 1 },
        tiles: total,
        seconds: t0.elapsed().as_secs_f64(),
        fidelity_db,
        had_alpha,
    };
    Some((out, report, capped))
}

fn sample_alpha(img: &Image, x: f32, y: f32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let wts = |t: f32| {
        let w = |d: f32| {
            let d = d.abs();
            if d < 1.0 {
                1.5 * d * d * d - 2.5 * d * d + 1.0
            } else if d < 2.0 {
                -0.5 * d * d * d + 2.5 * d * d - 4.0 * d + 2.0
            } else {
                0.0
            }
        };
        [w(t + 1.0), w(t), w(1.0 - t), w(2.0 - t)]
    };
    let (wx, wy) = (wts(fx), wts(fy));
    let mut acc = 0.0;
    for j in 0..4 {
        for i in 0..4 {
            acc += wx[i] * wy[j] * img.at_clamped(x0 as isize + i as isize - 1, y0 as isize + j as isize - 1)[3];
        }
    }
    acc.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_taps_cover_and_normalise() {
        let t = axis_taps(10, 20, 400, 200, 0, 400, Filter::Lanczos3);
        for w in &t.w {
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        }
        assert!(t.min_idx <= 20 && t.max_idx <= 400);
    }
}
