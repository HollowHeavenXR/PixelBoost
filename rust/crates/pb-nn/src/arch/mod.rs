pub mod rrdb;
pub mod srvgg;

use crate::conv::Conv3x3;
use crate::fmap::Fmap;
use crate::pbm::Weights;

/// A super-resolution network: RGB in [0,1] (3-channel `Fmap`) to RGB at
/// `scale()` times the size.
pub trait SrNetwork: Send + Sync {
    fn scale(&self) -> usize;
    /// Input height/width must be multiples of this.
    fn align(&self) -> usize {
        1
    }
    fn forward(&self, x: &Fmap) -> Fmap;
    /// Multiply-accumulates per *input* pixel.
    fn macs_per_px(&self) -> f64;
}

pub(crate) fn conv(w: &Weights, name: &str) -> Result<Conv3x3, String> {
    let wt = w.get(&format!("{name}.weight"))?;
    let b = w.get(&format!("{name}.bias")).ok();
    if wt.shape.len() != 4 || wt.shape[2] != 3 || wt.shape[3] != 3 {
        return Err(format!("{name}: expected a 3x3 conv, got {:?}", wt.shape));
    }
    Ok(Conv3x3::from_torch(&wt.data, b.map(|b| b.data.as_slice()), wt.shape[0], wt.shape[1]))
}

pub fn build(w: &Weights) -> Result<Box<dyn SrNetwork>, String> {
    match w.header.arch.as_str() {
        "rrdb" => Ok(Box::new(rrdb::Rrdb::load(w)?)),
        "srvgg" => Ok(Box::new(srvgg::Srvgg::load(w)?)),
        a => Err(format!("unknown architecture {a}")),
    }
}

/// Guess architecture + config from a PyTorch state dict's tensor names.
pub fn detect(sd: &crate::pth::StateDict) -> Result<(String, serde_json::Value), String> {
    let has = |n: &str| sd.iter().any(|(k, _)| k == n);
    let shape = |n: &str| sd.iter().find(|(k, _)| k == n).map(|(_, t)| t.shape.clone());
    if has("conv_first.weight") && has("body.0.rdb1.conv1.weight") {
        let blocks = (0..).take_while(|i| has(&format!("body.{i}.rdb1.conv1.weight"))).count();
        let first = shape("conv_first.weight").unwrap();
        let num_in = first[1];
        let scale = match num_in {
            3 => 4,
            12 => 2,
            48 => 1,
            n => return Err(format!("unexpected conv_first input channels {n}")),
        };
        let feat = first[0];
        let grow = shape("body.0.rdb1.conv1.weight").unwrap()[0];
        return Ok(("rrdb".into(), serde_json::json!({"num_block": blocks, "scale": scale, "num_feat": feat, "num_grow": grow})));
    }
    if has("body.0.weight") && has("body.1.weight") {
        let convs = (0..).take_while(|i| has(&format!("body.{}.weight", i * 2))).count();
        let last = shape(&format!("body.{}.weight", (convs - 1) * 2)).unwrap();
        let s2 = last[0] / 3;
        let scale = (s2 as f64).sqrt().round() as usize;
        let feat = shape("body.0.weight").unwrap()[0];
        return Ok(("srvgg".into(), serde_json::json!({"num_conv": convs - 2, "scale": scale, "num_feat": feat})));
    }
    Err("unrecognised checkpoint layout".into())
}
