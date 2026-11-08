//! Feature maps: one image of activations, stored channel-last (HWC) with a
//! one-pixel zero border so a 3x3 convolution can read every tap without
//! bounds checks.
//!
//! `c` is the per-pixel channel *capacity*. A dense block keeps all of its
//! growing concatenation in one map (x at 0..64, x1 at 64..96, ...), so a
//! "concat" is just writing a conv's output into a later channel slot.

use crate::conv::MR;

#[derive(Default, Clone)]
pub struct Fmap {
    pub h: usize,
    pub w: usize,
    pub c: usize,
    pub data: Vec<f32>,
}

impl Fmap {
    pub fn new(h: usize, w: usize, c: usize) -> Self {
        // Slack past the end: the conv micro-kernel always reads MR pixels,
        // even for the last partial block of the last row.
        let len = (h + 2) * (w + 2) * c + (MR + 2) * c;
        Fmap { h, w, c, data: vec![0.0; len] }
    }

    #[inline(always)]
    pub fn row_stride(&self) -> usize {
        (self.w + 2) * self.c
    }

    /// Offset of pixel (y, x); both may be -1 or h / w to address the border.
    #[inline(always)]
    pub fn off(&self, y: isize, x: isize) -> usize {
        (((y + 1) as usize) * (self.w + 2) + (x + 1) as usize) * self.c
    }

    #[inline(always)]
    pub fn px(&self, y: usize, x: usize) -> &[f32] {
        let o = self.off(y as isize, x as isize);
        &self.data[o..o + self.c]
    }

    #[inline(always)]
    pub fn px_mut(&mut self, y: usize, x: usize) -> &mut [f32] {
        let o = self.off(y as isize, x as isize);
        let c = self.c;
        &mut self.data[o..o + c]
    }

    /// Copy `n` channels from `src[src_off..]` into `self[dst_off..]` for every pixel.
    pub fn copy_channels(&mut self, dst_off: usize, src: &Fmap, src_off: usize, n: usize) {
        assert_eq!((self.h, self.w), (src.h, src.w));
        for y in 0..self.h {
            for x in 0..self.w {
                let s = src.off(y as isize, x as isize) + src_off;
                let d = self.off(y as isize, x as isize) + dst_off;
                self.data[d..d + n].copy_from_slice(&src.data[s..s + n]);
            }
        }
    }

    /// Zero the border ring (needed only if something wrote into it).
    pub fn clear_border(&mut self) {
        let (h, w, c) = (self.h as isize, self.w as isize, self.c);
        for x in -1..=w {
            for y in [-1, h] {
                let o = self.off(y, x);
                self.data[o..o + c].fill(0.0);
            }
        }
        for y in 0..h {
            for x in [-1, w] {
                let o = self.off(y, x);
                self.data[o..o + c].fill(0.0);
            }
        }
    }
}

/// Nearest-neighbour 2x upsample of channels 0..n.
pub fn upsample_nearest2x(src: &Fmap, n: usize) -> Fmap {
    use rayon::prelude::*;
    let mut out = Fmap::new(src.h * 2, src.w * 2, n);
    let rs = out.row_stride();
    let (h2, w2) = (out.h, out.w);
    out.data[rs..rs * (h2 + 1)]
        .par_chunks_mut(rs)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..w2 {
                let s = src.off((y / 2) as isize, (x / 2) as isize);
                let d = (x + 1) * n;
                row[d..d + n].copy_from_slice(&src.data[s..s + n]);
            }
        });
    out
}

/// (C*r*r, H, W) -> (C, H*r, W*r), PyTorch `pixel_shuffle` channel order.
pub fn pixel_shuffle(src: &Fmap, r: usize) -> Fmap {
    let c = src.c / (r * r);
    let mut out = Fmap::new(src.h * r, src.w * r, c);
    for y in 0..src.h {
        for x in 0..src.w {
            let s = src.px(y, x).to_vec();
            for ch in 0..c {
                for dy in 0..r {
                    for dx in 0..r {
                        let v = s[ch * r * r + dy * r + dx];
                        let o = out.off((y * r + dy) as isize, (x * r + dx) as isize);
                        out.data[o + ch] = v;
                    }
                }
            }
        }
    }
    out
}

/// (C, H*r, W*r) -> (C*r*r, H, W), inverse of `pixel_shuffle`.
pub fn pixel_unshuffle(src: &Fmap, r: usize) -> Fmap {
    assert!(src.h % r == 0 && src.w % r == 0, "pixel_unshuffle needs dims divisible by {r}");
    let c = src.c;
    let mut out = Fmap::new(src.h / r, src.w / r, c * r * r);
    for y in 0..out.h {
        for x in 0..out.w {
            for ch in 0..c {
                for dy in 0..r {
                    for dx in 0..r {
                        let v = src.px(y * r + dy, x * r + dx)[ch];
                        let o = out.off(y as isize, x as isize);
                        out.data[o + ch * r * r + dy * r + dx] = v;
                    }
                }
            }
        }
    }
    out
}
