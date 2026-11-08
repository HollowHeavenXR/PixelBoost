//! Reader for PyTorch `torch.save` checkpoints (`.pth`), written from scratch:
//! a minimal ZIP directory reader (PyTorch stores entries uncompressed) and a
//! small pickle VM that understands exactly what a `state_dict` uses —
//! dicts, tuples, strings, ints, `OrderedDict`, `_rebuild_tensor_v2` and
//! persistent storage references.

use std::collections::HashMap;

#[derive(Debug)]
pub struct TensorF32 {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

pub type StateDict = Vec<(String, TensorF32)>;

#[derive(Debug)]
pub struct PthError(pub String);
impl std::fmt::Display for PthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pth: {}", self.0)
    }
}
impl std::error::Error for PthError {}

fn err<T>(m: impl Into<String>) -> Result<T, PthError> {
    Err(PthError(m.into()))
}

// ---------------------------------------------------------------- ZIP

struct ZipEntry {
    name: String,
    offset: usize,
    size: usize,
    method: u16,
}

fn u16le(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes([b[o], b[o + 1]]) as usize
}
fn u32le(b: &[u8], o: usize) -> usize {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize
}
fn u64le(b: &[u8], o: usize) -> usize {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) as usize
}

fn zip_entries(b: &[u8]) -> Result<Vec<ZipEntry>, PthError> {
    // End of central directory record: scan back for PK\x05\x06.
    let min = b.len().saturating_sub(65_557);
    let mut eocd = None;
    let mut i = b.len().saturating_sub(22);
    while i >= min {
        if &b[i..i + 4] == b"PK\x05\x06" {
            eocd = Some(i);
            break;
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    let eocd = match eocd {
        Some(e) => e,
        None => return err("not a zip archive (legacy pre-1.6 torch format is not supported)"),
    };
    let mut count = u16le(b, eocd + 10);
    let mut cd = u32le(b, eocd + 16);
    if cd == 0xFFFF_FFFF || count == 0xFFFF {
        // ZIP64: locator sits right before the EOCD.
        let loc = eocd - 20;
        if &b[loc..loc + 4] != b"PK\x06\x07" {
            return err("bad zip64 locator");
        }
        let rec = u64le(b, loc + 8);
        count = u64le(b, rec + 32);
        cd = u64le(b, rec + 48);
    }
    let mut out = Vec::with_capacity(count);
    let mut p = cd;
    for _ in 0..count {
        if &b[p..p + 4] != b"PK\x01\x02" {
            return err("bad central directory");
        }
        let method = u16le(b, p + 10) as u16;
        let mut csize = u32le(b, p + 20);
        let mut local = u32le(b, p + 42);
        let nlen = u16le(b, p + 28);
        let xlen = u16le(b, p + 30);
        let clen = u16le(b, p + 32);
        let name = String::from_utf8_lossy(&b[p + 46..p + 46 + nlen]).into_owned();
        // ZIP64 extra field.
        let mut x = p + 46 + nlen;
        let xend = x + xlen;
        while x + 4 <= xend {
            let id = u16le(b, x);
            let sz = u16le(b, x + 2);
            if id == 1 {
                let mut q = x + 4;
                if u32le(b, p + 24) == 0xFFFF_FFFF {
                    q += 8;
                }
                if csize == 0xFFFF_FFFF {
                    csize = u64le(b, q);
                    q += 8;
                }
                if local == 0xFFFF_FFFF {
                    local = u64le(b, q);
                }
            }
            x += 4 + sz;
        }
        let lnlen = u16le(b, local + 26);
        let lxlen = u16le(b, local + 28);
        out.push(ZipEntry { name, offset: local + 30 + lnlen + lxlen, size: csize, method });
        p += 46 + nlen + xlen + clen;
    }
    Ok(out)
}

// ---------------------------------------------------------------- pickle

#[derive(Clone, Debug)]
#[allow(dead_code)]
enum V {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Tuple(Vec<V>),
    List(Vec<V>),
    Dict(Vec<(V, V)>),
    Global(String, String),
    Storage { key: String, dtype: String },
    Tensor { key: String, dtype: String, offset: usize, shape: Vec<usize>, stride: Vec<usize> },
    Mark,
    Other,
}

fn as_ints(v: &V) -> Result<Vec<usize>, PthError> {
    match v {
        V::Tuple(t) | V::List(t) => t
            .iter()
            .map(|x| match x {
                V::Int(i) => Ok(*i as usize),
                _ => err("expected int in tuple"),
            })
            .collect(),
        _ => err("expected tuple of ints"),
    }
}

fn unpickle(p: &[u8]) -> Result<V, PthError> {
    let mut st: Vec<V> = Vec::new();
    let mut memo: HashMap<usize, V> = HashMap::new();
    let mut i = 0usize;
    macro_rules! pop {
        () => {
            match st.pop() {
                Some(v) => v,
                None => return err("pickle stack underflow"),
            }
        };
    }
    let pop_mark = |st: &mut Vec<V>| -> Result<Vec<V>, PthError> {
        let pos = st.iter().rposition(|v| matches!(v, V::Mark)).ok_or_else(|| PthError("no mark".into()))?;
        let items = st.split_off(pos + 1);
        st.pop();
        Ok(items)
    };
    loop {
        if i >= p.len() {
            return err("pickle ended without STOP");
        }
        let op = p[i];
        i += 1;
        match op {
            0x80 => i += 1,                // PROTO
            0x95 => i += 8,                // FRAME
            b'.' => return Ok(pop!()),     // STOP
            b'}' => st.push(V::Dict(vec![])),
            b']' => st.push(V::List(vec![])),
            b')' => st.push(V::Tuple(vec![])),
            b'(' => st.push(V::Mark),
            b'N' => st.push(V::None),
            0x88 => st.push(V::Bool(true)),
            0x89 => st.push(V::Bool(false)),
            b'K' => {
                st.push(V::Int(p[i] as i64));
                i += 1;
            }
            b'M' => {
                st.push(V::Int(u16le(p, i) as i64));
                i += 2;
            }
            b'J' => {
                st.push(V::Int(i32::from_le_bytes(p[i..i + 4].try_into().unwrap()) as i64));
                i += 4;
            }
            0x8a => {
                // LONG1
                let n = p[i] as usize;
                i += 1;
                let mut v: i64 = 0;
                for k in 0..n.min(8) {
                    v |= (p[i + k] as i64) << (8 * k);
                }
                if n > 0 && n < 8 && p[i + n - 1] & 0x80 != 0 {
                    v -= 1i64 << (8 * n);
                }
                i += n;
                st.push(V::Int(v));
            }
            b'G' => {
                st.push(V::Float(f64::from_be_bytes(p[i..i + 8].try_into().unwrap())));
                i += 8;
            }
            b'X' => {
                let n = u32le(p, i);
                i += 4;
                st.push(V::Str(String::from_utf8_lossy(&p[i..i + n]).into_owned()));
                i += n;
            }
            0x8c => {
                let n = p[i] as usize;
                i += 1;
                st.push(V::Str(String::from_utf8_lossy(&p[i..i + n]).into_owned()));
                i += n;
            }
            b'q' => {
                memo.insert(p[i] as usize, st.last().cloned().unwrap_or(V::None));
                i += 1;
            }
            b'r' => {
                memo.insert(u32le(p, i), st.last().cloned().unwrap_or(V::None));
                i += 4;
            }
            0x94 => {
                let n = memo.len();
                memo.insert(n, st.last().cloned().unwrap_or(V::None));
            }
            b'h' => {
                st.push(memo.get(&(p[i] as usize)).cloned().unwrap_or(V::None));
                i += 1;
            }
            b'j' => {
                st.push(memo.get(&u32le(p, i)).cloned().unwrap_or(V::None));
                i += 4;
            }
            b'c' => {
                let nl1 = p[i..].iter().position(|&c| c == b'\n').unwrap();
                let module = String::from_utf8_lossy(&p[i..i + nl1]).into_owned();
                i += nl1 + 1;
                let nl2 = p[i..].iter().position(|&c| c == b'\n').unwrap();
                let name = String::from_utf8_lossy(&p[i..i + nl2]).into_owned();
                i += nl2 + 1;
                st.push(V::Global(module, name));
            }
            0x93 => {
                let name = pop!();
                let module = pop!();
                match (module, name) {
                    (V::Str(m), V::Str(n)) => st.push(V::Global(m, n)),
                    _ => return err("bad STACK_GLOBAL"),
                }
            }
            0x85 => {
                let a = pop!();
                st.push(V::Tuple(vec![a]));
            }
            0x86 => {
                let b = pop!();
                let a = pop!();
                st.push(V::Tuple(vec![a, b]));
            }
            0x87 => {
                let c = pop!();
                let b = pop!();
                let a = pop!();
                st.push(V::Tuple(vec![a, b, c]));
            }
            b't' => {
                let items = pop_mark(&mut st)?;
                st.push(V::Tuple(items));
            }
            b'l' => {
                let items = pop_mark(&mut st)?;
                st.push(V::List(items));
            }
            b'a' => {
                let v = pop!();
                if let Some(V::List(l)) = st.last_mut() {
                    l.push(v);
                }
            }
            b'e' => {
                let items = pop_mark(&mut st)?;
                if let Some(V::List(l)) = st.last_mut() {
                    l.extend(items);
                }
            }
            b's' => {
                let v = pop!();
                let k = pop!();
                if let Some(V::Dict(d)) = st.last_mut() {
                    d.push((k, v));
                }
            }
            b'u' => {
                let items = pop_mark(&mut st)?;
                if let Some(V::Dict(d)) = st.last_mut() {
                    let mut it = items.into_iter();
                    while let (Some(k), Some(v)) = (it.next(), it.next()) {
                        d.push((k, v));
                    }
                }
            }
            b'Q' => {
                // BINPERSID: ('storage', StorageType, key, location, numel)
                let pid = pop!();
                match pid {
                    V::Tuple(t) if t.len() >= 3 => {
                        let dtype = match &t[1] {
                            V::Global(_, n) => n.clone(),
                            _ => "FloatStorage".into(),
                        };
                        let key = match &t[2] {
                            V::Str(s) => s.clone(),
                            _ => return err("bad storage key"),
                        };
                        st.push(V::Storage { key, dtype });
                    }
                    _ => return err("unsupported persistent id"),
                }
            }
            b'R' => {
                let args = pop!();
                let f = pop!();
                let args = match args {
                    V::Tuple(t) => t,
                    _ => return err("REDUCE args not a tuple"),
                };
                match f {
                    V::Global(m, n) if m == "torch._utils" && n == "_rebuild_tensor_v2" => {
                        let (key, dtype) = match &args[0] {
                            V::Storage { key, dtype } => (key.clone(), dtype.clone()),
                            _ => return err("tensor without storage"),
                        };
                        let offset = match args[1] {
                            V::Int(o) => o as usize,
                            _ => 0,
                        };
                        st.push(V::Tensor { key, dtype, offset, shape: as_ints(&args[2])?, stride: as_ints(&args[3])? });
                    }
                    V::Global(m, n) if m == "collections" && n == "OrderedDict" => st.push(V::Dict(vec![])),
                    _ => st.push(V::Other),
                }
            }
            b'b' => {
                // BUILD: OrderedDict state / tensor state — ignore.
                pop!();
            }
            0x81 => {
                // NEWOBJ
                pop!();
                pop!();
                st.push(V::Other);
            }
            _ => return err(format!("unsupported pickle opcode 0x{op:02x} at {}", i - 1)),
        }
    }
}

/// Load every tensor of a checkpoint. Real-ESRGAN checkpoints wrap the state
/// dict as `{"params_ema": ...}` or `{"params": ...}`; `params_ema` wins.
pub fn load_pth(bytes: &[u8]) -> Result<StateDict, PthError> {
    let entries = zip_entries(bytes)?;
    let pkl = entries.iter().find(|e| e.name.ends_with("/data.pkl")).ok_or_else(|| PthError("no data.pkl".into()))?;
    let prefix = pkl.name.trim_end_matches("data.pkl").to_string();
    let get = |e: &ZipEntry| -> Result<&[u8], PthError> {
        if e.method != 0 {
            return err(format!("{}: compressed zip entries are not supported", e.name));
        }
        Ok(&bytes[e.offset..e.offset + e.size])
    };
    let root = unpickle(get(pkl)?)?;
    let mut dict = match root {
        V::Dict(d) => d,
        _ => return err("checkpoint root is not a dict"),
    };
    for wrapper in ["params_ema", "params", "state_dict", "model"] {
        if let Some(pos) = dict.iter().position(|(k, v)| matches!(k, V::Str(s) if s == wrapper) && matches!(v, V::Dict(_))) {
            if let (_, V::Dict(inner)) = dict.swap_remove(pos) {
                dict = inner;
                break;
            }
        }
    }
    let by_name: HashMap<&str, &ZipEntry> = entries.iter().map(|e| (e.name.as_str(), e)).collect();
    let mut out = Vec::new();
    for (k, v) in dict {
        let (name, (key, dtype, offset, shape, stride)) = match (k, v) {
            (V::Str(n), V::Tensor { key, dtype, offset, shape, stride }) => (n, (key, dtype, offset, shape, stride)),
            _ => continue,
        };
        let numel: usize = shape.iter().product();
        // Contiguous check.
        let mut expect = 1;
        for d in (0..shape.len()).rev() {
            if shape[d] > 1 && stride[d] != expect {
                return err(format!("{name}: non-contiguous tensor"));
            }
            expect *= shape[d];
        }
        let entry = by_name.get(format!("{prefix}data/{key}").as_str()).ok_or_else(|| PthError(format!("missing storage {key}")))?;
        let raw = get(entry)?;
        let data: Vec<f32> = match dtype.as_str() {
            "FloatStorage" => raw[offset * 4..(offset + numel) * 4].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect(),
            "HalfStorage" => raw[offset * 2..(offset + numel) * 2].chunks_exact(2).map(|c| crate::half::f16_to_f32(u16::from_le_bytes([c[0], c[1]]))).collect(),
            other => return err(format!("{name}: unsupported dtype {other}")),
        };
        out.push((name, TensorF32 { shape, data }));
    }
    Ok(out)
}
