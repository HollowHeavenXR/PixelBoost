//! SRVGGNetCompact (realesr-general-x4v3, realesr-animevideov3): a plain
//! stack of 3x3 convs + PReLU at input resolution, a pixel shuffle, and a
//! nearest-upsampled copy of the input added back.

use super::{conv, SrNetwork};
use crate::conv::{conv3x3, Conv3x3, Epi};
use crate::fmap::{pixel_shuffle, Fmap};
use crate::pbm::Weights;

pub struct Srvgg {
    scale: usize,
    convs: Vec<Conv3x3>,
    alphas: Vec<Vec<f32>>,
}

impl Srvgg {
    pub fn load(w: &Weights) -> Result<Self, String> {
        let n = w.cfg("num_conv").ok_or("num_conv missing")? as usize;
        let scale = w.cfg("scale").unwrap_or(4) as usize;
        let mut convs = Vec::new();
        let mut alphas = Vec::new();
        for i in 0..n + 2 {
            convs.push(conv(w, &format!("body.{}", i * 2))?);
            if i < n + 1 {
                let a = w.get(&format!("body.{}.weight", i * 2 + 1))?.data.clone();
                let mut padded = a;
                padded.resize(padded.len().div_ceil(16) * 16, 0.0);
                alphas.push(padded);
            }
        }
        Ok(Srvgg { scale, convs, alphas })
    }
}

impl SrNetwork for Srvgg {
    fn scale(&self) -> usize {
        self.scale
    }
    fn macs_per_px(&self) -> f64 {
        self.convs.iter().map(|c| c.macs_per_px()).sum::<usize>() as f64
    }
    fn forward(&self, x: &Fmap) -> Fmap {
        let (h, w) = (x.h, x.w);
        let f = self.convs[0].cout;
        let mut a = Fmap::new(h, w, f);
        let mut b = Fmap::new(h, w, f);
        conv3x3(x, 0, &self.convs[0], &mut a, 0, Epi::prelu(&self.alphas[0]));
        let n = self.convs.len();
        for i in 1..n - 1 {
            conv3x3(&a, 0, &self.convs[i], &mut b, 0, Epi::prelu(&self.alphas[i]));
            std::mem::swap(&mut a, &mut b);
        }
        drop(b);
        let last = &self.convs[n - 1];
        let mut o = Fmap::new(h, w, last.cout);
        conv3x3(&a, 0, last, &mut o, 0, Epi::PLAIN);
        drop(a);
        let mut out = pixel_shuffle(&o, self.scale);
        let s = self.scale;
        for y in 0..out.h {
            for xx in 0..out.w {
                let base = x.px(y / s, xx / s);
                let (r, g, bl) = (base[0], base[1], base[2]);
                let p = out.px_mut(y, xx);
                p[0] += r;
                p[1] += g;
                p[2] += bl;
            }
        }
        out
    }
}
