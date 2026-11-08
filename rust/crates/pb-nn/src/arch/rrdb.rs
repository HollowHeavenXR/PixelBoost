//! RRDBNet (ESRGAN / Real-ESRGAN x4plus, x2plus, anime_6B, RealESRNet).
//!
//! Dense blocks keep their whole concatenation in one 192-channel map, so the
//! five convs of a residual dense block read a growing channel prefix and
//! append their 32 new channels in place.

use super::{conv, SrNetwork};
use crate::conv::{conv3x3, conv3x3_within, Act, Conv3x3, Epi};
use crate::fmap::{pixel_unshuffle, upsample_nearest2x, Fmap};
use crate::pbm::Weights;

pub struct Rrdb {
    scale: usize,
    unshuffle: usize,
    feat: usize,
    grow: usize,
    first: Conv3x3,
    blocks: Vec<[[Conv3x3; 5]; 3]>,
    body: Conv3x3,
    up1: Conv3x3,
    up2: Conv3x3,
    hr: Conv3x3,
    last: Conv3x3,
}

const SLOPE: f32 = 0.2;

impl Rrdb {
    pub fn load(w: &Weights) -> Result<Self, String> {
        let n = w.cfg("num_block").ok_or("num_block missing")? as usize;
        let scale = w.cfg("scale").unwrap_or(4) as usize;
        let mut blocks = Vec::with_capacity(n);
        for i in 0..n {
            let rdb = |r: usize| -> Result<[Conv3x3; 5], String> {
                Ok([
                    conv(w, &format!("body.{i}.rdb{r}.conv1"))?,
                    conv(w, &format!("body.{i}.rdb{r}.conv2"))?,
                    conv(w, &format!("body.{i}.rdb{r}.conv3"))?,
                    conv(w, &format!("body.{i}.rdb{r}.conv4"))?,
                    conv(w, &format!("body.{i}.rdb{r}.conv5"))?,
                ])
            };
            blocks.push([rdb(1)?, rdb(2)?, rdb(3)?]);
        }
        let first = conv(w, "conv_first")?;
        let feat = first.cout;
        let grow = blocks.first().map(|b| b[0][0].cout).unwrap_or(32);
        Ok(Rrdb {
            scale,
            unshuffle: match scale {
                4 => 1,
                2 => 2,
                1 => 4,
                s => return Err(format!("unsupported RRDB scale {s}")),
            },
            feat,
            grow,
            first,
            blocks,
            body: conv(w, "conv_body")?,
            up1: conv(w, "conv_up1")?,
            up2: conv(w, "conv_up2")?,
            hr: conv(w, "conv_hr")?,
            last: conv(w, "conv_last")?,
        })
    }

    /// One residual dense block: dense buffer `d` holds x in 0..feat; the
    /// result goes to `out[0..feat]` as `conv5 * s + sum(res)`.
    fn rdb(&self, convs: &[Conv3x3; 5], d: &mut Fmap) {
        let (f, g) = (self.feat, self.grow);
        for k in 0..4 {
            conv3x3_within(d, 0, &convs[k], f + k * g, Epi::lrelu(SLOPE));
        }
    }
}

impl SrNetwork for Rrdb {
    fn scale(&self) -> usize {
        self.scale
    }
    fn align(&self) -> usize {
        self.unshuffle
    }
    fn macs_per_px(&self) -> f64 {
        let u2 = (self.unshuffle * self.unshuffle) as f64;
        let body: usize = self.blocks.iter().flatten().flatten().map(|c| c.macs_per_px()).sum::<usize>() + self.first.macs_per_px() + self.body.macs_per_px();
        // Upsampler runs at 4x / 16x the body's pixel count.
        let up = self.up1.macs_per_px() * 4 + (self.up2.macs_per_px() + self.hr.macs_per_px() + self.last.macs_per_px()) * 16;
        (body + up) as f64 / u2
    }

    fn forward(&self, x: &Fmap) -> Fmap {
        let shuffled;
        let inp = if self.unshuffle > 1 {
            shuffled = pixel_unshuffle(x, self.unshuffle);
            &shuffled
        } else {
            x
        };
        let (h, w, f) = (inp.h, inp.w, self.feat);
        let dc = f + 4 * self.grow;
        let mut feat = Fmap::new(h, w, f);
        conv3x3(inp, 0, &self.first, &mut feat, 0, Epi::PLAIN);

        let mut d = [Fmap::new(h, w, dc), Fmap::new(h, w, dc), Fmap::new(h, w, dc)];
        d[0].copy_channels(0, &feat, 0, f);
        let mut cur = 0;
        for blk in &self.blocks {
            let (a, b, c) = (cur, (cur + 1) % 3, (cur + 2) % 3);
            // RDB1: d[a] -> d[b][0..f] = conv5*0.2 + x0
            self.rdb(&blk[0], &mut d[a]);
            {
                let [da, db] = pick2(&mut d, a, b);
                conv3x3(da, 0, &blk[0][4], db, 0, Epi { act: Act::None, scale: 0.2, res: [Some((&*da, 0, 1.0)), None] });
            }
            // RDB2: d[b] -> d[c][0..f]
            self.rdb(&blk[1], &mut d[b]);
            {
                let [db, dcm] = pick2(&mut d, b, c);
                conv3x3(db, 0, &blk[1][4], dcm, 0, Epi { act: Act::None, scale: 0.2, res: [Some((&*db, 0, 1.0)), None] });
            }
            // RDB3 + RRDB skip: d[c] -> d[b][0..f] = conv5*0.04 + 0.2*x2 + x0
            self.rdb(&blk[2], &mut d[c]);
            {
                let [da, db, dcm] = pick3(&mut d, a, b, c);
                conv3x3(dcm, 0, &blk[2][4], db, 0, Epi { act: Act::None, scale: 0.04, res: [Some((&*dcm, 0, 0.2)), Some((&*da, 0, 1.0))] });
            }
            cur = b;
        }
        // trunk: feat + conv_body(body)
        let mut trunk = Fmap::new(h, w, f);
        conv3x3(&d[cur], 0, &self.body, &mut trunk, 0, Epi { act: Act::None, scale: 1.0, res: [Some((&feat, 0, 1.0)), None] });
        drop(d);
        drop(feat);

        let u = upsample_nearest2x(&trunk, f);
        drop(trunk);
        let mut a = Fmap::new(u.h, u.w, f);
        conv3x3(&u, 0, &self.up1, &mut a, 0, Epi::lrelu(SLOPE));
        drop(u);
        let u = upsample_nearest2x(&a, f);
        drop(a);
        let mut b = Fmap::new(u.h, u.w, f);
        conv3x3(&u, 0, &self.up2, &mut b, 0, Epi::lrelu(SLOPE));
        let mut c = u; // reuse the buffer
        conv3x3(&b, 0, &self.hr, &mut c, 0, Epi::lrelu(SLOPE));
        drop(b);
        let mut out = Fmap::new(c.h, c.w, self.last.cout);
        conv3x3(&c, 0, &self.last, &mut out, 0, Epi::PLAIN);
        out
    }
}

fn pick2(d: &mut [Fmap; 3], i: usize, j: usize) -> [&mut Fmap; 2] {
    assert_ne!(i, j);
    let p = d.as_mut_ptr();
    // SAFETY: distinct indices into the same array.
    unsafe { [&mut *p.add(i), &mut *p.add(j)] }
}

fn pick3(d: &mut [Fmap; 3], i: usize, j: usize, k: usize) -> [&mut Fmap; 3] {
    assert!(i != j && j != k && i != k);
    let p = d.as_mut_ptr();
    // SAFETY: distinct indices into the same array.
    unsafe { [&mut *p.add(i), &mut *p.add(j), &mut *p.add(k)] }
}
