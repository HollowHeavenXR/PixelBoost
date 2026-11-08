//! 3x3 convolution, stride 1, zero padding 1 — the only conv every
//! PixelBoost super-resolution network uses.
//!
//! Layout: activations are padded HWC (`Fmap`), weights are packed per block
//! of 16 output channels as `[block][tap 0..9][cin][16]`. For one output row
//! the conv is nine small GEMMs (one per tap) over shifted input rows, so no
//! im2col buffer is ever built. The micro-kernel computes a 6-pixel x
//! 16-channel tile in registers (AVX2+FMA on x86-64, NEON on aarch64, plain
//! Rust elsewhere) and an epilogue fuses bias, activation, scaling and up to
//! two residual adds.

use crate::fmap::Fmap;
use rayon::prelude::*;

/// Output channels per register tile.
pub const NR: usize = 16;
/// Output pixels per register tile.
pub const MR: usize = 6;

pub struct Conv3x3 {
    pub cin: usize,
    pub cout: usize,
    /// `[cout_pad/16][9][cin][16]`
    pub packed: Vec<f32>,
    /// Padded to a multiple of 16.
    pub bias: Vec<f32>,
}

impl Conv3x3 {
    /// `w` is PyTorch layout `[cout][cin][3][3]`.
    pub fn from_torch(w: &[f32], b: Option<&[f32]>, cout: usize, cin: usize) -> Self {
        assert_eq!(w.len(), cout * cin * 9, "conv weight size mismatch");
        let blocks = cout.div_ceil(NR);
        let mut packed = vec![0.0f32; blocks * 9 * cin * NR];
        for co in 0..cout {
            let (cb, j) = (co / NR, co % NR);
            for ci in 0..cin {
                for k in 0..9 {
                    packed[((cb * 9 + k) * cin + ci) * NR + j] = w[(co * cin + ci) * 9 + k];
                }
            }
        }
        let mut bias = vec![0.0f32; blocks * NR];
        if let Some(b) = b {
            bias[..cout].copy_from_slice(b);
        }
        Conv3x3 { cin, cout, packed, bias }
    }

    pub fn blocks(&self) -> usize {
        self.bias.len() / NR
    }

    /// Multiply-accumulates per output pixel (for benchmarks / ETA).
    pub fn macs_per_px(&self) -> usize {
        self.cin * self.cout * 9
    }
}

#[derive(Clone, Copy)]
pub enum Act<'a> {
    None,
    LeakyRelu(f32),
    /// Per-channel negative slope (PReLU), indexed by output channel.
    PRelu(&'a [f32]),
}

/// `out = act(conv + bias) * scale + sum(coef_i * res_i)`
#[derive(Clone, Copy)]
pub struct Epi<'a> {
    pub act: Act<'a>,
    pub scale: f32,
    pub res: [Option<(&'a Fmap, usize, f32)>; 2],
}

impl<'a> Epi<'a> {
    pub const PLAIN: Epi<'static> = Epi { act: Act::None, scale: 1.0, res: [None, None] };
    pub fn lrelu(slope: f32) -> Self {
        Epi { act: Act::LeakyRelu(slope), scale: 1.0, res: [None, None] }
    }
    pub fn prelu(alpha: &'a [f32]) -> Self {
        Epi { act: Act::PRelu(alpha), scale: 1.0, res: [None, None] }
    }
}

type Kernel = unsafe fn(*const f32, usize, usize, usize, *const f32, &mut [[f32; NR]; MR]);

fn pick_kernel() -> Kernel {
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma") {
            return kern_avx2;
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        return kern_neon;
    }
    #[allow(unreachable_code)]
    kern_scalar
}

static KERNEL: std::sync::OnceLock<Kernel> = std::sync::OnceLock::new();

pub fn kernel_name() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma") {
            return "avx2-fma 6x16";
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        return "neon 6x16";
    }
    #[allow(unreachable_code)]
    "scalar"
}

/// Force the portable kernel (tests compare the SIMD kernels against it).
pub fn conv3x3_scalar(inp: &Fmap, cin_off: usize, conv: &Conv3x3, out: &mut Fmap, cout_off: usize, epi: Epi) {
    let o = RawMut::of(out);
    run(kern_scalar, Raw::of(inp), cin_off, conv, o, cout_off, epi)
}

/// Convolve channels `cin_off..cin_off+conv.cin` of `inp` into channels
/// `cout_off..cout_off+conv.cout` of `out`.
pub fn conv3x3(inp: &Fmap, cin_off: usize, conv: &Conv3x3, out: &mut Fmap, cout_off: usize, epi: Epi) {
    let k = *KERNEL.get_or_init(pick_kernel);
    let o = RawMut::of(out);
    run(k, Raw::of(inp), cin_off, conv, o, cout_off, epi)
}

/// Convolve within one map: read channels `cin_off..+cin`, write the disjoint
/// range `cout_off..+cout` (how a dense block appends to its concatenation).
pub fn conv3x3_within(buf: &mut Fmap, cin_off: usize, conv: &Conv3x3, cout_off: usize, epi: Epi) {
    assert!(cout_off >= cin_off + conv.cin || cout_off + conv.cout <= cin_off, "conv3x3_within: channel ranges overlap");
    let k = *KERNEL.get_or_init(pick_kernel);
    let o = RawMut::of(buf);
    let i = Raw { p: o.p as *const f32, h: o.h, w: o.w, c: o.c };
    run(k, i, cin_off, conv, o, cout_off, epi)
}

#[derive(Clone, Copy)]
struct Raw {
    p: *const f32,
    h: usize,
    w: usize,
    c: usize,
}
#[derive(Clone, Copy)]
struct RawMut {
    p: *mut f32,
    h: usize,
    w: usize,
    c: usize,
}
unsafe impl Send for Raw {}
unsafe impl Sync for Raw {}
unsafe impl Send for RawMut {}
unsafe impl Sync for RawMut {}
impl Raw {
    fn of(f: &Fmap) -> Self {
        Raw { p: f.data.as_ptr(), h: f.h, w: f.w, c: f.c }
    }
    #[inline(always)]
    fn off(&self, y: isize, x: isize) -> usize {
        (((y + 1) as usize) * (self.w + 2) + (x + 1) as usize) * self.c
    }
}
impl RawMut {
    fn of(f: &mut Fmap) -> Self {
        RawMut { p: f.data.as_mut_ptr(), h: f.h, w: f.w, c: f.c }
    }
}

fn run(kern: Kernel, inp: Raw, cin_off: usize, conv: &Conv3x3, out: RawMut, cout_off: usize, epi: Epi) {
    assert_eq!((inp.h, inp.w), (out.h, out.w), "conv in/out size mismatch");
    assert!(cin_off + conv.cin <= inp.c);
    assert!(cout_off + conv.cout <= out.c);
    let mut res: [Option<(Raw, usize, f32)>; 2] = [None, None];
    for (slot, r) in res.iter_mut().zip(epi.res.iter()) {
        if let Some((m, off, coef)) = r {
            assert_eq!((m.h, m.w), (out.h, out.w));
            assert!(off + conv.cout <= m.c);
            *slot = Some((Raw::of(m), *off, *coef));
        }
    }
    let (h, w) = (out.h, out.w);
    let cin = conv.cin;
    let cout = conv.cout;
    let blocks = conv.blocks();
    let block_len = 9 * cin * NR;
    let act = epi.act;
    let scale = epi.scale;
    let ow = out.w;
    let oc = out.c;
    let op = SendMut(out.p);

    (0..h).into_par_iter().for_each(|y| {
        let op = op;
        let mut acc = [[0.0f32; NR]; MR];
        for cb in 0..blocks {
            let wp = conv.packed[cb * block_len..].as_ptr();
            let ncols = NR.min(cout - cb * NR);
            let bias = &conv.bias[cb * NR..cb * NR + NR];
            let mut x = 0;
            while x < w {
                let n = MR.min(w - x);
                // Tap (0,0) of pixel (y, x) is pixel (y-1, x-1).
                let base = inp.off(y as isize - 1, x as isize - 1) + cin_off;
                // SAFETY: Fmap carries MR pixels of slack past its end, the
                // border ring covers the -1 / +1 taps, packed weights hold
                // exactly 9*cin*16 floats per block, and each row y is written
                // by exactly one task into a channel range nobody reads.
                unsafe {
                    kern(inp.p.add(base), inp.c, (inp.w + 2) * inp.c, cin, wp, &mut acc);
                    for i in 0..n {
                        let px = x + i;
                        let dst = op.0.add((((y + 1) * (ow + 2)) + px + 1) * oc + cout_off + cb * NR);
                        let mut rp: [(*const f32, f32); 2] = [(std::ptr::null(), 0.0); 2];
                        for (slot, r) in rp.iter_mut().zip(res.iter()) {
                            if let Some((m, off, coef)) = r {
                                *slot = (m.p.add(m.off(y as isize, px as isize) + off + cb * NR), *coef);
                            }
                        }
                        for j in 0..ncols {
                            let mut v = acc[i][j] + bias[j];
                            v = match act {
                                Act::None => v,
                                Act::LeakyRelu(s) => {
                                    if v < 0.0 {
                                        v * s
                                    } else {
                                        v
                                    }
                                }
                                Act::PRelu(a) => {
                                    if v < 0.0 {
                                        v * a[cb * NR + j]
                                    } else {
                                        v
                                    }
                                }
                            };
                            v *= scale;
                            for (p, coef) in rp {
                                if !p.is_null() {
                                    v += coef * *p.add(j);
                                }
                            }
                            *dst.add(j) = v;
                        }
                    }
                }
                x += MR;
            }
        }
    });
}

#[derive(Clone, Copy)]
struct SendMut(*mut f32);
unsafe impl Send for SendMut {}
unsafe impl Sync for SendMut {}

/// Portable reference kernel.
unsafe fn kern_scalar(a: *const f32, ps: usize, rs: usize, cin: usize, w: *const f32, acc: &mut [[f32; NR]; MR]) {
    *acc = [[0.0; NR]; MR];
    let mut wp = w;
    for ky in 0..3 {
        for kx in 0..3 {
            let base = a.add(ky * rs + kx * ps);
            for ci in 0..cin {
                let wrow = std::slice::from_raw_parts(wp, NR);
                for (i, row) in acc.iter_mut().enumerate() {
                    let av = *base.add(i * ps + ci);
                    for j in 0..NR {
                        row[j] += av * wrow[j];
                    }
                }
                wp = wp.add(NR);
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn kern_avx2(a: *const f32, ps: usize, rs: usize, cin: usize, w: *const f32, acc: &mut [[f32; NR]; MR]) {
    use std::arch::x86_64::*;
    let z = _mm256_setzero_ps();
    let (mut c00, mut c01, mut c10, mut c11, mut c20, mut c21) = (z, z, z, z, z, z);
    let (mut c30, mut c31, mut c40, mut c41, mut c50, mut c51) = (z, z, z, z, z, z);
    let mut wp = w;
    for ky in 0..3 {
        for kx in 0..3 {
            let p0 = a.add(ky * rs + kx * ps);
            let (p1, p2, p3, p4, p5) = (p0.add(ps), p0.add(2 * ps), p0.add(3 * ps), p0.add(4 * ps), p0.add(5 * ps));
            for ci in 0..cin {
                let b0 = _mm256_loadu_ps(wp);
                let b1 = _mm256_loadu_ps(wp.add(8));
                wp = wp.add(NR);
                let a0 = _mm256_broadcast_ss(&*p0.add(ci));
                c00 = _mm256_fmadd_ps(a0, b0, c00);
                c01 = _mm256_fmadd_ps(a0, b1, c01);
                let a1 = _mm256_broadcast_ss(&*p1.add(ci));
                c10 = _mm256_fmadd_ps(a1, b0, c10);
                c11 = _mm256_fmadd_ps(a1, b1, c11);
                let a2 = _mm256_broadcast_ss(&*p2.add(ci));
                c20 = _mm256_fmadd_ps(a2, b0, c20);
                c21 = _mm256_fmadd_ps(a2, b1, c21);
                let a3 = _mm256_broadcast_ss(&*p3.add(ci));
                c30 = _mm256_fmadd_ps(a3, b0, c30);
                c31 = _mm256_fmadd_ps(a3, b1, c31);
                let a4 = _mm256_broadcast_ss(&*p4.add(ci));
                c40 = _mm256_fmadd_ps(a4, b0, c40);
                c41 = _mm256_fmadd_ps(a4, b1, c41);
                let a5 = _mm256_broadcast_ss(&*p5.add(ci));
                c50 = _mm256_fmadd_ps(a5, b0, c50);
                c51 = _mm256_fmadd_ps(a5, b1, c51);
            }
        }
    }
    let o = acc.as_mut_ptr() as *mut f32;
    for (i, (lo, hi)) in [(c00, c01), (c10, c11), (c20, c21), (c30, c31), (c40, c41), (c50, c51)].into_iter().enumerate() {
        _mm256_storeu_ps(o.add(i * NR), lo);
        _mm256_storeu_ps(o.add(i * NR + 8), hi);
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn kern_neon(a: *const f32, ps: usize, rs: usize, cin: usize, w: *const f32, acc: &mut [[f32; NR]; MR]) {
    use std::arch::aarch64::*;
    let z = vdupq_n_f32(0.0);
    let mut c = [[z; 4]; MR];
    let mut wp = w;
    for ky in 0..3 {
        for kx in 0..3 {
            let p0 = a.add(ky * rs + kx * ps);
            for ci in 0..cin {
                let b0 = vld1q_f32(wp);
                let b1 = vld1q_f32(wp.add(4));
                let b2 = vld1q_f32(wp.add(8));
                let b3 = vld1q_f32(wp.add(12));
                wp = wp.add(NR);
                for i in 0..MR {
                    let av = vdupq_n_f32(*p0.add(i * ps + ci));
                    c[i][0] = vfmaq_f32(c[i][0], av, b0);
                    c[i][1] = vfmaq_f32(c[i][1], av, b1);
                    c[i][2] = vfmaq_f32(c[i][2], av, b2);
                    c[i][3] = vfmaq_f32(c[i][3], av, b3);
                }
            }
        }
    }
    let o = acc.as_mut_ptr() as *mut f32;
    for i in 0..MR {
        for q in 0..4 {
            vst1q_f32(o.add(i * NR + q * 4), c[i][q]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(seed: &mut u64) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        ((*seed >> 40) as f32 / (1u64 << 24) as f32) - 0.5
    }

    /// Direct textbook convolution straight from PyTorch-layout weights.
    fn naive(inp: &Fmap, cin_off: usize, w: &[f32], b: &[f32], cout: usize, cin: usize) -> Vec<f32> {
        let mut out = vec![0.0; inp.h * inp.w * cout];
        for y in 0..inp.h as isize {
            for x in 0..inp.w as isize {
                for co in 0..cout {
                    let mut s = b[co];
                    for ci in 0..cin {
                        for ky in 0..3isize {
                            for kx in 0..3isize {
                                let (yy, xx) = (y + ky - 1, x + kx - 1);
                                if yy < 0 || xx < 0 || yy >= inp.h as isize || xx >= inp.w as isize {
                                    continue;
                                }
                                s += inp.data[inp.off(yy, xx) + cin_off + ci] * w[(co * cin + ci) * 9 + (ky * 3 + kx) as usize];
                            }
                        }
                    }
                    out[((y as usize) * inp.w + x as usize) * cout + co] = s;
                }
            }
        }
        out
    }

    #[test]
    fn simd_and_scalar_match_naive() {
        let mut seed = 0x1234_5678_9abc_def1u64;
        for &(h, w, cin, cout, cstride, cin_off) in &[(7, 13, 3, 64, 3, 0), (9, 11, 40, 32, 96, 24), (5, 6, 64, 3, 64, 0), (12, 17, 12, 48, 12, 0)] {
            let mut inp = Fmap::new(h, w, cstride);
            for y in 0..h {
                for x in 0..w {
                    for v in inp.px_mut(y, x) {
                        *v = rng(&mut seed);
                    }
                }
            }
            let wt: Vec<f32> = (0..cout * cin * 9).map(|_| rng(&mut seed)).collect();
            let bs: Vec<f32> = (0..cout).map(|_| rng(&mut seed)).collect();
            let conv = Conv3x3::from_torch(&wt, Some(&bs), cout, cin);
            let want = naive(&inp, cin_off, &wt, &bs, cout, cin);
            for simd in [false, true] {
                let mut out = Fmap::new(h, w, cout + 5);
                if simd {
                    conv3x3(&inp, cin_off, &conv, &mut out, 5, Epi::PLAIN);
                } else {
                    conv3x3_scalar(&inp, cin_off, &conv, &mut out, 5, Epi::PLAIN);
                }
                for y in 0..h {
                    for x in 0..w {
                        for co in 0..cout {
                            let g = out.px(y, x)[5 + co];
                            let e = want[(y * w + x) * cout + co];
                            assert!((g - e).abs() < 1e-4, "simd={simd} {h}x{w} cin{cin} cout{cout} @({y},{x},{co}) {g} vs {e}");
                        }
                    }
                }
                // Border ring must stay zero.
                for x in -1..=w as isize {
                    assert!(out.data[out.off(-1, x)..out.off(-1, x) + out.c].iter().all(|v| *v == 0.0));
                    assert!(out.data[out.off(h as isize, x)..out.off(h as isize, x) + out.c].iter().all(|v| *v == 0.0));
                }
            }
        }
    }

    #[test]
    fn epilogue_residuals() {
        let mut seed = 99u64;
        let (h, w) = (4, 5);
        let mut inp = Fmap::new(h, w, 16);
        let mut r1 = Fmap::new(h, w, 16);
        let mut r2 = Fmap::new(h, w, 20);
        for y in 0..h {
            for x in 0..w {
                inp.px_mut(y, x).iter_mut().for_each(|v| *v = rng(&mut seed));
                r1.px_mut(y, x).iter_mut().for_each(|v| *v = rng(&mut seed));
                r2.px_mut(y, x).iter_mut().for_each(|v| *v = rng(&mut seed));
            }
        }
        let wt: Vec<f32> = (0..16 * 16 * 9).map(|_| rng(&mut seed)).collect();
        let conv = Conv3x3::from_torch(&wt, None, 16, 16);
        let mut plain = Fmap::new(h, w, 16);
        conv3x3(&inp, 0, &conv, &mut plain, 0, Epi::PLAIN);
        let mut fused = Fmap::new(h, w, 16);
        let epi = Epi { act: Act::LeakyRelu(0.2), scale: 0.2, res: [Some((&r1, 0, 1.0)), Some((&r2, 4, 0.5))] };
        conv3x3(&inp, 0, &conv, &mut fused, 0, epi);
        for y in 0..h {
            for x in 0..w {
                for c in 0..16 {
                    let p = plain.px(y, x)[c];
                    let a = if p < 0.0 { p * 0.2 } else { p };
                    let e = a * 0.2 + r1.px(y, x)[c] + 0.5 * r2.px(y, x)[4 + c];
                    assert!((fused.px(y, x)[c] - e).abs() < 1e-5);
                }
            }
        }
    }
}
