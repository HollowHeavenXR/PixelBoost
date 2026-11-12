//! Fast neighbourhood primitives shared by the editing tools: O(1)-per-pixel
//! box blurs (running sums, any radius), a 3-pass Gaussian approximation,
//! and small helpers. Everything is row/column parallel with rayon.

use crate::image::Image;
use rayon::prelude::*;

/// One horizontal box pass of radius `r` over a single-channel plane.
fn box_h(src: &[f32], dst: &mut [f32], w: usize, r: usize) {
    if r == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let inv = 1.0 / (2 * r + 1) as f32;
    dst.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(o, s)| {
        let last = w - 1;
        let at = |i: isize| s[i.clamp(0, last as isize) as usize];
        let mut acc = 0.0f32;
        for i in -(r as isize)..=r as isize {
            acc += at(i);
        }
        for x in 0..w {
            o[x] = acc * inv;
            acc += at(x as isize + r as isize + 1) - at(x as isize - r as isize);
        }
    });
}

/// One vertical box pass (processed in column strips for cache locality).
fn box_v(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    if r == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let inv = 1.0 / (2 * r + 1) as f32;
    const STRIP: usize = 64;
    let strips: Vec<usize> = (0..w).step_by(STRIP).collect();
    let out_ptr = dst.as_mut_ptr() as usize;
    strips.par_iter().for_each(|&x0| {
        let x1 = (x0 + STRIP).min(w);
        let n = x1 - x0;
        let mut acc = vec![0.0f32; n];
        let row = |y: isize| y.clamp(0, h as isize - 1) as usize * w;
        for y in -(r as isize)..=r as isize {
            let b = row(y);
            for i in 0..n {
                acc[i] += src[b + x0 + i];
            }
        }
        for y in 0..h {
            // SAFETY: each strip writes a disjoint set of columns.
            let o = unsafe { std::slice::from_raw_parts_mut((out_ptr as *mut f32).add(y * w + x0), n) };
            for i in 0..n {
                o[i] = acc[i] * inv;
            }
            let add = row(y as isize + r as isize + 1);
            let sub = row(y as isize - r as isize);
            for i in 0..n {
                acc[i] += src[add + x0 + i] - src[sub + x0 + i];
            }
        }
    });
}

/// Box blur of a plane with radius `r` (both directions).
pub fn box_blur_plane(p: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut a = vec![0.0f32; p.len()];
    let mut b = vec![0.0f32; p.len()];
    box_h(p, &mut a, w, r);
    box_v(&a, &mut b, w, h, r);
    b
}

/// Box radii for a 3-pass Gaussian of standard deviation `sigma`.
fn gauss_radii(sigma: f32) -> [usize; 3] {
    let n = 3.0f32;
    let wi = ((12.0 * sigma * sigma / n) + 1.0).sqrt().floor();
    let wl = if (wi as i32) % 2 == 0 { wi - 1.0 } else { wi };
    let wu = wl + 2.0;
    let m = ((12.0 * sigma * sigma - n * wl * wl - 4.0 * n * wl - 3.0 * n) / (-4.0 * wl - 4.0)).round();
    let mut r = [0usize; 3];
    for (i, v) in r.iter_mut().enumerate() {
        let size = if (i as f32) < m { wl } else { wu };
        *v = ((size - 1.0) / 2.0).max(0.0) as usize;
    }
    r
}

/// Gaussian blur of a plane (3 box passes). `sigma` in pixels.
pub fn gauss_plane(p: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma < 0.3 {
        return p.to_vec();
    }
    let mut cur = p.to_vec();
    for r in gauss_radii(sigma) {
        cur = box_blur_plane(&cur, w, h, r);
    }
    cur
}

/// Split into four planes.
pub fn planes(img: &Image) -> [Vec<f32>; 4] {
    let mut out: [Vec<f32>; 4] = Default::default();
    for (c, o) in out.iter_mut().enumerate() {
        *o = img.px.par_iter().map(|p| p[c]).collect();
    }
    out
}

pub fn from_planes(w: usize, h: usize, p: &[Vec<f32>; 4]) -> Image {
    let px = (0..w * h).into_par_iter().map(|i| [p[0][i], p[1][i], p[2][i], p[3][i]]).collect();
    Image { w, h, px }
}

/// Gaussian blur of a whole image. Colour is blurred premultiplied so
/// transparent pixels don't bleed their (meaningless) colour.
pub fn gauss(img: &Image, sigma: f32) -> Image {
    if sigma < 0.3 {
        return img.clone();
    }
    let alpha = img.has_alpha();
    let mut p = planes(img);
    if alpha {
        for c in 0..3 {
            let (a, rest) = (&p[3].clone(), &mut p[c]);
            rest.par_iter_mut().zip(a.par_iter()).for_each(|(v, a)| *v *= a);
        }
    }
    let n = if alpha { 4 } else { 3 };
    for c in 0..n {
        p[c] = gauss_plane(&p[c], img.w, img.h, sigma);
    }
    if alpha {
        let a = p[3].clone();
        for c in 0..3 {
            p[c].par_iter_mut().zip(a.par_iter()).for_each(|(v, a)| *v = if *a > 1e-5 { (*v / a).min(1.0) } else { 0.0 });
        }
    }
    from_planes(img.w, img.h, &p)
}

/// Luma plane (BT.709 on encoded values).
pub fn luma_plane(img: &Image) -> Vec<f32> {
    img.px.par_iter().map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).collect()
}

#[inline(always)]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline(always)]
pub fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Bilinear sample with clamp-to-edge (pixel-centre coordinates).
#[inline]
pub fn sample_bilinear(img: &Image, x: f32, y: f32) -> [f32; 4] {
    let x = x.clamp(0.0, (img.w - 1) as f32);
    let y = y.clamp(0.0, (img.h - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(img.w - 1), (y0 + 1).min(img.h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let a = img.px[y0 * img.w + x0];
    let b = img.px[y0 * img.w + x1];
    let c = img.px[y1 * img.w + x0];
    let d = img.px[y1 * img.w + x1];
    let mut o = [0.0f32; 4];
    for k in 0..4 {
        let t = a[k] + (b[k] - a[k]) * fx;
        let u = c[k] + (d[k] - c[k]) * fx;
        o[k] = t + (u - t) * fy;
    }
    o
}

/// Bilinear sample of a plane.
#[inline]
pub fn sample_plane(p: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let t = p[y0 * w + x0] + (p[y0 * w + x1] - p[y0 * w + x0]) * fx;
    let u = p[y1 * w + x0] + (p[y1 * w + x1] - p[y1 * w + x0]) * fx;
    t + (u - t) * fy
}

/// Deterministic per-pixel hash noise in [-1, 1].
#[inline(always)]
pub fn hash_noise(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 32767.5 - 1.0
}

/// Grey-level dilation (max filter) with a square window, separable.
pub fn dilate_plane(p: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return p.to_vec();
    }
    let mut a = vec![0.0f32; p.len()];
    a.par_chunks_mut(w).zip(p.par_chunks(w)).for_each(|(o, s)| {
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            o[x] = s[lo..=hi].iter().copied().fold(0.0, f32::max);
        }
    });
    let mut b = vec![0.0f32; p.len()];
    b.par_chunks_mut(w).enumerate().for_each(|(y, o)| {
        let lo = y.saturating_sub(r);
        let hi = (y + r).min(h - 1);
        for x in 0..w {
            let mut m = 0.0f32;
            for yy in lo..=hi {
                m = m.max(a[yy * w + x]);
            }
            o[x] = m;
        }
    });
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn box_blur_keeps_constant_and_mean() {
        let (w, h) = (37, 23);
        let p = vec![0.4f32; w * h];
        let b = box_blur_plane(&p, w, h, 5);
        assert!(b.iter().all(|v| (v - 0.4).abs() < 1e-5));
        let mut q = vec![0.0f32; w * h];
        q[11 * w + 18] = 1.0;
        let g = gauss_plane(&q, w, h, 2.0);
        let s: f32 = g.iter().sum();
        assert!((s - 1.0).abs() < 0.02, "{s}");
        assert!(g[11 * w + 18] > g[11 * w + 21]);
    }
    #[test]
    fn dilate_grows() {
        let (w, h) = (9, 9);
        let mut p = vec![0.0f32; w * h];
        p[4 * w + 4] = 1.0;
        let d = dilate_plane(&p, w, h, 2);
        assert_eq!(d[2 * w + 2], 1.0);
        assert_eq!(d[1 * w + 1], 0.0);
    }
}
