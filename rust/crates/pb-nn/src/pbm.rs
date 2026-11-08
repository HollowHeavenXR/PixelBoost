//! `.pbnn` — PixelBoost's own model file: a JSON header describing the
//! architecture and tensors, followed by fp16 tensor data.
//!
//! ```text
//! b"PBNN" | u32 version=1 | u32 header_len | header JSON | pad to 16 | data
//! ```

use crate::half::{f16_to_f32, f32_to_f16};
use crate::pth::{StateDict, TensorF32};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TensorInfo {
    pub name: String,
    pub shape: Vec<usize>,
    /// Byte offset into the data section.
    pub offset: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Header {
    pub arch: String,
    pub config: serde_json::Value,
    pub meta: serde_json::Value,
    pub tensors: Vec<TensorInfo>,
}

pub struct Weights {
    pub header: Header,
    pub tensors: HashMap<String, TensorF32>,
}

impl Weights {
    pub fn get(&self, name: &str) -> Result<&TensorF32, String> {
        self.tensors.get(name).ok_or_else(|| format!("model is missing tensor {name}"))
    }
    pub fn cfg(&self, key: &str) -> Option<i64> {
        self.header.config.get(key).and_then(|v| v.as_i64())
    }
}

pub fn write(arch: &str, config: serde_json::Value, meta: serde_json::Value, sd: &StateDict) -> Vec<u8> {
    let mut data = Vec::new();
    let mut tensors = Vec::new();
    for (name, t) in sd {
        tensors.push(TensorInfo { name: name.clone(), shape: t.shape.clone(), offset: data.len() });
        for v in &t.data {
            data.extend_from_slice(&f32_to_f16(*v).to_le_bytes());
        }
    }
    let header = serde_json::to_vec(&Header { arch: arch.into(), config, meta, tensors }).unwrap();
    let mut out = Vec::with_capacity(header.len() + data.len() + 32);
    out.extend_from_slice(b"PBNN");
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    while out.len() % 16 != 0 {
        out.push(0);
    }
    out.extend_from_slice(&data);
    out
}

pub fn read(bytes: &[u8]) -> Result<Weights, String> {
    if bytes.len() < 12 || &bytes[..4] != b"PBNN" {
        return Err("not a .pbnn model".into());
    }
    let ver = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    if ver != 1 {
        return Err(format!("unsupported .pbnn version {ver}"));
    }
    let hlen = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let header: Header = serde_json::from_slice(&bytes[12..12 + hlen]).map_err(|e| e.to_string())?;
    let data_start = (12 + hlen).div_ceil(16) * 16;
    let data = &bytes[data_start..];
    let mut tensors = HashMap::new();
    for t in &header.tensors {
        let n: usize = t.shape.iter().product();
        let raw = data.get(t.offset..t.offset + n * 2).ok_or_else(|| format!("{} is truncated", t.name))?;
        let v = raw.chunks_exact(2).map(|c| f16_to_f32(u16::from_le_bytes([c[0], c[1]]))).collect();
        tensors.insert(t.name.clone(), TensorF32 { shape: t.shape.clone(), data: v });
    }
    Ok(Weights { header, tensors })
}

/// Network interpolation (DNI): `t * a + (1 - t) * b`, tensor by tensor.
/// Both models must share an architecture (e.g. realesr-general-x4v3 and its
/// `wdn` denoise twin) — this is how the denoise-strength slider works.
pub fn blend(a: &Weights, b: &Weights, t: f32) -> Result<Weights, String> {
    if a.header.arch != b.header.arch || a.header.config != b.header.config {
        return Err("can only blend models of the same architecture".into());
    }
    let mut tensors = HashMap::new();
    for (name, ta) in &a.tensors {
        let tb = b.get(name)?;
        if ta.shape != tb.shape {
            return Err(format!("{name}: shape mismatch"));
        }
        let data = ta.data.iter().zip(&tb.data).map(|(x, y)| t * x + (1.0 - t) * y).collect();
        tensors.insert(name.clone(), TensorF32 { shape: ta.shape.clone(), data });
    }
    Ok(Weights { header: a.header.clone(), tensors })
}
