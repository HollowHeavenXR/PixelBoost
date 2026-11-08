//! Objective quality measures used by the test bench and the app's
//! fidelity report.

use crate::image::Image;
use rayon::prelude::*;

fn luma_plane(img: &Image) -> Vec<f32> {
    img.px.par_iter().map(|p| Image::luma(*p)).collect()
}

/// PSNR (dB) of the luma channel, ignoring a `border`-pixel frame.
pub fn psnr(a: &Image, b: &Image, border: usize) -> f64 {
    assert_eq!((a.w, a.h), (b.w, b.h));
    let (la, lb) = (luma_plane(a), luma_plane(b));
    let mut se = 0.0f64;
    let mut n = 0usize;
    for y in border..a.h - border {
        for x in border..a.w - border {
            let d = (la[y * a.w + x] - lb[y * a.w + x]) as f64;
            se += d * d;
            n += 1;
        }
    }
    let mse = se / n.max(1) as f64;
    if mse <= 1e-12 {
        99.0
    } else {
        10.0 * (1.0 / mse).log10()
    }
}

fn gauss_blur(p: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let r = (sigma * 3.0).ceil() as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let ks: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / ks).collect();
    let mut tmp = vec![0.0; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let mut s = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let xx = (x as isize + i as isize - r).clamp(0, w as isize - 1) as usize;
                s += kv * p[y * w + xx];
            }
            row[x] = s;
        }
    });
    let mut out = vec![0.0; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let mut s = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let yy = (y as isize + i as isize - r).clamp(0, h as isize - 1) as usize;
                s += kv * tmp[yy * w + x];
            }
            row[x] = s;
        }
    });
    out
}

/// Mean SSIM of luma (Gaussian window, sigma 1.5).
pub fn ssim(a: &Image, b: &Image) -> f64 {
    let (w, h) = (a.w, a.h);
    let (x, y) = (luma_plane(a), luma_plane(b));
    let xx: Vec<f32> = x.iter().map(|v| v * v).collect();
    let yy: Vec<f32> = y.iter().map(|v| v * v).collect();
    let xy: Vec<f32> = x.iter().zip(&y).map(|(a, b)| a * b).collect();
    let (mx, my) = (gauss_blur(&x, w, h, 1.5), gauss_blur(&y, w, h, 1.5));
    let (sxx, syy, sxy) = (gauss_blur(&xx, w, h, 1.5), gauss_blur(&yy, w, h, 1.5), gauss_blur(&xy, w, h, 1.5));
    let (c1, c2) = (0.01f32 * 0.01, 0.03f32 * 0.03);
    let total: f64 = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (mx, my) = (mx[i], my[i]);
            let vx = sxx[i] - mx * mx;
            let vy = syy[i] - my * my;
            let cv = sxy[i] - mx * my;
            (((2.0 * mx * my + c1) * (2.0 * cv + c2)) / ((mx * mx + my * my + c1) * (vx + vy + c2))) as f64
        })
        .sum();
    total / (w * h) as f64
}

/// Mean gradient magnitude of luma — how much edge/texture energy there is.
pub fn gradient_energy(img: &Image) -> f64 {
    let l = luma_plane(img);
    let (w, h) = (img.w, img.h);
    let s: f64 = (1..h - 1)
        .into_par_iter()
        .map(|y| {
            let mut acc = 0.0f64;
            for x in 1..w - 1 {
                let gx = l[y * w + x + 1] - l[y * w + x - 1];
                let gy = l[(y + 1) * w + x] - l[(y - 1) * w + x];
                acc += ((gx * gx + gy * gy) as f64).sqrt();
            }
            acc
        })
        .sum();
    s / ((w - 2) * (h - 2)) as f64
}

/// Gradient-magnitude similarity deviation-style score in [0,1] (1 = edge
/// structure identical). Penalises both smearing and invented edges.
pub fn edge_similarity(a: &Image, b: &Image) -> f64 {
    let (la, lb) = (luma_plane(a), luma_plane(b));
    let (w, h) = (a.w, a.h);
    let g = |l: &[f32], x: usize, y: usize| {
        let gx = l[y * w + x + 1] - l[y * w + x - 1];
        let gy = l[(y + 1) * w + x] - l[(y - 1) * w + x];
        (gx * gx + gy * gy).sqrt()
    };
    let c = 0.0026f32;
    let s: f64 = (1..h - 1)
        .into_par_iter()
        .map(|y| {
            let mut acc = 0.0f64;
            for x in 1..w - 1 {
                let (ga, gb) = (g(&la, x, y), g(&lb, x, y));
                acc += ((2.0 * ga * gb + c) / (ga * ga + gb * gb + c)) as f64;
            }
            acc
        })
        .sum();
    s / ((w - 2) * (h - 2)) as f64
}
