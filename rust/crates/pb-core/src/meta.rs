//! Photo metadata carried from an original onto its replacement: EXIF
//! (date, camera, lens, GPS...) and the ICC colour profile.
//!
//! Read from JPEG / PNG / WebP (via the `image` decoders) and HEIC/HEIF
//! (own ISOBMFF reader: `meta` → `iinf`/`iloc` for the Exif item,
//! `iprp/ipco/colr` for the profile). The EXIF is patched for the new file:
//! orientation reset to 1 (pixels are stored upright), pixel dimensions
//! updated, and the stale embedded thumbnail (IFD1) dropped.

use crate::image::Image;
use image::ImageDecoder;
use std::io::Cursor;

#[derive(Debug, Clone, Default)]
pub struct Meta {
    /// Raw TIFF-structured EXIF (starts with "II*\0" or "MM\0*").
    pub exif: Option<Vec<u8>>,
    pub icc: Option<Vec<u8>>,
}

fn strip_exif_header(mut v: Vec<u8>) -> Option<Vec<u8>> {
    if v.starts_with(b"Exif\0\0") {
        v.drain(..6);
    }
    if v.len() >= 8 && (v.starts_with(b"II*\0") || v.starts_with(b"MM\0*")) {
        Some(v)
    } else {
        None
    }
}

/// Metadata of an image file (any supported format). Never fails: missing
/// or unreadable metadata is just absent.
pub fn read(bytes: &[u8]) -> Meta {
    if is_heif(bytes) {
        return heif::read(bytes).unwrap_or_default();
    }
    let Ok(r) = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format() else { return Meta::default() };
    let Ok(mut d) = r.into_decoder() else { return Meta::default() };
    let icc = d.icc_profile().ok().flatten().filter(|v| v.len() > 128);
    let exif = d.exif_metadata().ok().flatten().and_then(strip_exif_header);
    Meta { exif, icc }
}

fn is_heif(b: &[u8]) -> bool {
    b.len() >= 12 && &b[4..8] == b"ftyp" && matches!(&b[8..12], b"heic" | b"heix" | b"heim" | b"heis" | b"mif1" | b"msf1" | b"avif" | b"hevc" | b"hevx")
}

// ------------------------------------------------------------- EXIF patch

struct Tiff<'a> {
    b: &'a mut [u8],
    le: bool,
}

impl Tiff<'_> {
    fn u16(&self, o: usize) -> Option<u16> {
        let s = self.b.get(o..o + 2)?;
        Some(if self.le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) })
    }
    fn u32(&self, o: usize) -> Option<u32> {
        let s = self.b.get(o..o + 4)?;
        let a = [s[0], s[1], s[2], s[3]];
        Some(if self.le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }
    fn put16(&mut self, o: usize, v: u16) {
        let a = if self.le { v.to_le_bytes() } else { v.to_be_bytes() };
        if let Some(s) = self.b.get_mut(o..o + 2) {
            s.copy_from_slice(&a);
        }
    }
    fn put32(&mut self, o: usize, v: u32) {
        let a = if self.le { v.to_le_bytes() } else { v.to_be_bytes() };
        if let Some(s) = self.b.get_mut(o..o + 4) {
            s.copy_from_slice(&a);
        }
    }

    /// Visit the entries of the IFD at `ifd`: (entry offset, tag, type, count).
    fn entries(&self, ifd: usize) -> Vec<(usize, u16, u16, u32)> {
        let Some(n) = self.u16(ifd) else { return vec![] };
        (0..n as usize)
            .filter_map(|i| {
                let e = ifd + 2 + i * 12;
                Some((e, self.u16(e)?, self.u16(e + 2)?, self.u32(e + 4)?))
            })
            .collect()
    }

    /// Set a SHORT/LONG value held inline in an entry.
    fn set_inline(&mut self, e: usize, typ: u16, v: u32) {
        match typ {
            3 if v <= u16::MAX as u32 => {
                self.put16(e + 8, v as u16);
                self.put16(e + 10, 0);
            }
            4 => self.put32(e + 8, v),
            _ => {}
        }
    }
}

/// Patch EXIF for upright pixels of size `w` x `h`. Returns None if the
/// blob isn't valid TIFF.
pub fn patch_exif(exif: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut v = exif.to_vec();
    let le = match v.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let mut t = Tiff { b: &mut v, le };
    let ifd0 = t.u32(4)? as usize;
    let ents = t.entries(ifd0);
    if ents.is_empty() {
        return None;
    }
    let mut exif_ifd = None;
    for (e, tag, typ, _) in &ents {
        match tag {
            0x0112 => t.set_inline(*e, *typ, 1),             // Orientation
            0x0100 => t.set_inline(*e, *typ, w),             // ImageWidth
            0x0101 => t.set_inline(*e, *typ, h),             // ImageLength
            0x8769 => exif_ifd = t.u32(e + 8).map(|o| o as usize),
            _ => {}
        }
    }
    // Drop IFD1 (the embedded thumbnail would show the old pixels).
    let next = ifd0 + 2 + ents.len() * 12;
    t.put32(next, 0);
    if let Some(sub) = exif_ifd {
        for (e, tag, typ, _) in t.entries(sub) {
            match tag {
                0xA002 => t.set_inline(e, typ, w), // PixelXDimension
                0xA003 => t.set_inline(e, typ, h), // PixelYDimension
                _ => {}
            }
        }
    }
    Some(v)
}

/// Orientation tag (1..8) from EXIF, 1 if absent.
pub fn orientation(exif: &[u8]) -> u16 {
    let mut v = exif.to_vec();
    let le = match v.get(0..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return 1,
    };
    let t = Tiff { b: &mut v, le };
    let Some(ifd0) = t.u32(4) else { return 1 };
    t.entries(ifd0 as usize).iter().find(|e| e.1 == 0x0112).and_then(|e| t.u16(e.0 + 8)).filter(|o| (1..=8).contains(o)).unwrap_or(1)
}

// ------------------------------------------------------------- writing

/// Encode a replacement file: JPEG (opaque) or PNG (has transparency, or
/// `png` asked) with the original's metadata. Pixels are read from `src`
/// straight to 8-bit (never to f32), so 100+ MP results fit in memory.
/// Returns (width, height, extension).
pub fn write_with_meta(src_path: &str, out_path: &str, meta: &Meta, quality: u8, allow_png: bool) -> Result<(u32, u32, &'static str), String> {
    use image::ImageEncoder;
    let img = image::ImageReader::open(src_path).map_err(|e| format!("{src_path}: {e}"))?.with_guessed_format().map_err(|e| e.to_string())?;
    let mut img = img;
    img.no_limits();
    let dynimg = img.decode().map_err(|e| format!("decode: {e}"))?;
    let (w, h) = (dynimg.width(), dynimg.height());
    let alpha = match &dynimg {
        image::DynamicImage::ImageRgba8(b) => b.pixels().any(|p| p.0[3] < 255),
        image::DynamicImage::ImageLumaA8(b) => b.pixels().any(|p| p.0[1] < 255),
        image::DynamicImage::ImageRgba16(b) => b.pixels().any(|p| p.0[3] < 65535),
        other => other.color().has_alpha(),
    };
    let exif = meta.exif.as_deref().and_then(|e| patch_exif(e, w, h));
    let mut out = Vec::new();
    let ext = if alpha && allow_png {
        let rgba = dynimg.into_rgba8();
        let mut enc = image::codecs::png::PngEncoder::new_with_quality(&mut out, image::codecs::png::CompressionType::Fast, image::codecs::png::FilterType::Adaptive);
        if let Some(icc) = &meta.icc {
            let _ = enc.set_icc_profile(icc.clone());
        }
        if let Some(e) = exif {
            let _ = enc.set_exif_metadata(e);
        }
        enc.write_image(rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())?;
        "png"
    } else {
        let rgb = dynimg.into_rgb8();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(50, 100));
        if let Some(icc) = &meta.icc {
            let _ = enc.set_icc_profile(icc.clone());
        }
        // APP1 holds at most 64 KB; skip EXIF that big (rare: maker notes).
        if let Some(e) = exif.filter(|e| e.len() < 65_000) {
            let _ = enc.set_exif_metadata(e);
        }
        enc.write_image(rgb.as_raw(), w, h, image::ExtendedColorType::Rgb8).map_err(|e| e.to_string())?;
        "jpg"
    };
    std::fs::write(out_path, out).map_err(|e| format!("{out_path}: {e}"))?;
    Ok((w, h, ext))
}

/// Same for an in-memory image.
pub fn encode_image_with_meta(img: &Image, meta: &Meta, quality: u8) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let rgb: Vec<u8> = img.to_rgba8().chunks_exact(4).flat_map(|c| [c[0], c[1], c[2]]).collect();
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    if let Some(icc) = &meta.icc {
        let _ = enc.set_icc_profile(icc.clone());
    }
    if let Some(e) = meta.exif.as_deref().and_then(|e| patch_exif(e, img.w as u32, img.h as u32)).filter(|e| e.len() < 65_000) {
        let _ = enc.set_exif_metadata(e);
    }
    enc.write_image(&rgb, img.w as u32, img.h as u32, image::ExtendedColorType::Rgb8).map_err(|e| e.to_string())?;
    Ok(out)
}

// ------------------------------------------------------------- HEIF

mod heif {
    use super::{strip_exif_header, Meta};

    struct Boxes<'a> {
        b: &'a [u8],
        pos: usize,
    }

    /// (type, payload)
    impl<'a> Iterator for Boxes<'a> {
        type Item = ([u8; 4], &'a [u8]);
        fn next(&mut self) -> Option<Self::Item> {
            let b = self.b;
            let p = self.pos;
            if p + 8 > b.len() {
                return None;
            }
            let mut size = u32::from_be_bytes(b[p..p + 4].try_into().ok()?) as u64;
            let typ: [u8; 4] = b[p + 4..p + 8].try_into().ok()?;
            let mut hdr = 8usize;
            if size == 1 {
                size = u64::from_be_bytes(b.get(p + 8..p + 16)?.try_into().ok()?);
                hdr = 16;
            } else if size == 0 {
                size = (b.len() - p) as u64;
            }
            let end = p.checked_add(size as usize)?;
            if size < hdr as u64 || end > b.len() {
                return None;
            }
            self.pos = end;
            Some((typ, &b[p + hdr..end]))
        }
    }

    fn boxes(b: &[u8]) -> Boxes<'_> {
        Boxes { b, pos: 0 }
    }

    fn find<'a>(b: &'a [u8], t: &[u8; 4]) -> Option<&'a [u8]> {
        boxes(b).find(|(ty, _)| ty == t).map(|(_, p)| p)
    }

    fn be(b: &[u8], o: usize, n: usize) -> Option<u64> {
        let s = b.get(o..o + n)?;
        Some(s.iter().fold(0u64, |a, v| (a << 8) | *v as u64))
    }

    pub fn read(file: &[u8]) -> Option<Meta> {
        let meta = find(file, b"meta")?;
        let meta = meta.get(4..)?; // full box
        let mut out = Meta::default();

        // ICC: first colr box of type prof/rICC in ipco.
        if let Some(ipco) = find(meta, b"iprp").and_then(|p| find(p, b"ipco")) {
            for (t, p) in boxes(ipco) {
                if &t == b"colr" && p.len() > 4 && (&p[..4] == b"prof" || &p[..4] == b"rICC") {
                    out.icc = Some(p[4..].to_vec());
                    break;
                }
            }
        }

        // Exif item id from iinf.
        let iinf = find(meta, b"iinf")?;
        let ver = *iinf.first()?;
        let entries_at = if ver == 0 { 6 } else { 8 };
        let mut exif_id = None;
        for (t, p) in boxes(iinf.get(entries_at..)?) {
            if &t != b"infe" || p.len() < 4 {
                continue;
            }
            let v = p[0];
            if v < 2 {
                continue;
            }
            let (id, at) = if v == 2 { (be(p, 4, 2)?, 8) } else { (be(p, 4, 4)?, 10) };
            if p.get(at..at + 4) == Some(b"Exif") {
                exif_id = Some(id);
                break;
            }
        }
        let Some(exif_id) = exif_id else { return Some(out) };

        // Location from iloc.
        let iloc = find(meta, b"iloc")?;
        let v = iloc[0];
        let mut o = 4;
        let off_size = (iloc[o] >> 4) as usize;
        let len_size = (iloc[o] & 15) as usize;
        let base_size = (iloc[o + 1] >> 4) as usize;
        let idx_size = if v == 1 || v == 2 { (iloc[o + 1] & 15) as usize } else { 0 };
        o += 2;
        let (count, cs) = if v < 2 { (be(iloc, o, 2)?, 2) } else { (be(iloc, o, 4)?, 4) };
        o += cs;
        for _ in 0..count {
            let (id, is) = if v < 2 { (be(iloc, o, 2)?, 2) } else { (be(iloc, o, 4)?, 4) };
            o += is;
            let method = if v == 1 || v == 2 {
                let m = be(iloc, o, 2)? & 15;
                o += 2;
                m
            } else {
                0
            };
            o += 2; // data_reference_index
            let base = be(iloc, o, base_size).unwrap_or(0);
            o += base_size;
            let n = be(iloc, o, 2)?;
            o += 2;
            let mut data = Vec::new();
            for _ in 0..n {
                o += idx_size;
                let off = be(iloc, o, off_size).unwrap_or(0);
                o += off_size;
                let len = be(iloc, o, len_size).unwrap_or(0);
                o += len_size;
                if id == exif_id && method == 0 {
                    let s = (base + off) as usize;
                    data.extend_from_slice(file.get(s..s + len as usize)?);
                }
            }
            if id == exif_id {
                // 4-byte offset to the TIFF header, then the TIFF data.
                let skip = be(&data, 0, 4)? as usize;
                out.exif = data.get(4 + skip..).map(|d| d.to_vec()).and_then(strip_exif_header);
                break;
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiff(le: bool) -> Vec<u8> {
        // IFD0: Orientation=6 (SHORT), ExifIFD pointer; ExifIFD: PixelX/Y (LONG).
        let mut v = Vec::new();
        let w16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&if le { x.to_le_bytes() } else { x.to_be_bytes() });
        let w32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&if le { x.to_le_bytes() } else { x.to_be_bytes() });
        v.extend_from_slice(if le { b"II" } else { b"MM" });
        w16(&mut v, 42);
        w32(&mut v, 8);
        w16(&mut v, 2);
        w16(&mut v, 0x0112);
        w16(&mut v, 3);
        w32(&mut v, 1);
        w16(&mut v, 6);
        w16(&mut v, 0);
        w16(&mut v, 0x8769);
        w16(&mut v, 4);
        w32(&mut v, 1);
        w32(&mut v, 8 + 2 + 24 + 4);
        w32(&mut v, 99); // next IFD (thumbnail)
        w16(&mut v, 2);
        for (tag, val) in [(0xA002u16, 4032u32), (0xA003, 3024)] {
            w16(&mut v, tag);
            w16(&mut v, 4);
            w32(&mut v, 1);
            w32(&mut v, val);
        }
        w32(&mut v, 0);
        v
    }

    #[test]
    fn patches_both_endians() {
        for le in [true, false] {
            let t = tiff(le);
            assert_eq!(orientation(&t), 6);
            let p = patch_exif(&t, 16128, 12096).unwrap();
            assert_eq!(orientation(&p), 1);
            let mut pp = p.clone();
            let tt = Tiff { b: &mut pp, le };
            assert_eq!(tt.u32(8 + 2 + 24), Some(0), "IFD1 dropped");
            let sub = 8 + 2 + 24 + 4;
            assert_eq!(tt.u32(sub + 2 + 8), Some(16128));
            assert_eq!(tt.u32(sub + 2 + 12 + 8), Some(12096));
        }
    }

    #[test]
    fn jpeg_round_trip_keeps_meta() {
        let mut img = Image::new(40, 30);
        for (i, p) in img.px.iter_mut().enumerate() {
            *p = [(i % 40) as f32 / 40.0, 0.4, 0.6, 1.0];
        }
        let icc = vec![7u8; 600];
        let m = Meta { exif: Some(tiff(true)), icc: Some(icc.clone()) };
        let bytes = encode_image_with_meta(&img, &m, 90).unwrap();
        let back = read(&bytes);
        assert_eq!(back.icc.as_deref(), Some(&icc[..]));
        assert_eq!(orientation(back.exif.as_deref().unwrap()), 1);
    }

    #[test]
    fn heif_exif_and_icc() {
        // Minimal HEIF: ftyp + meta(iinf(infe Exif id 7), iloc(v0), iprp/ipco/colr prof) + mdat.
        fn bx(t: &[u8; 4], p: &[u8]) -> Vec<u8> {
            let mut v = ((p.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(t);
            v.extend_from_slice(p);
            v
        }
        let exif = tiff(false);
        let mut item = vec![0, 0, 0, 0];
        item.extend_from_slice(&exif);
        let ftyp = bx(b"ftyp", b"heic\0\0\0\0mif1heic");
        let infe = bx(b"infe", &[2, 0, 0, 0, 0, 7, 0, 0, b'E', b'x', b'i', b'f', 0]);
        let mut iinf_p = vec![0, 0, 0, 0, 0, 1];
        iinf_p.extend_from_slice(&infe);
        let iinf = bx(b"iinf", &iinf_p);
        let mut colr_p = b"prof".to_vec();
        colr_p.extend_from_slice(&[9u8; 300]);
        let iprp = bx(b"iprp", &bx(b"ipco", &bx(b"colr", &colr_p)));
        let make = |off: u32| {
            let mut p = vec![0, 0, 0, 0, 0x44, 0x00, 0, 1, 0, 7, 0, 0, 0, 1];
            p.extend_from_slice(&off.to_be_bytes());
            p.extend_from_slice(&(item.len() as u32).to_be_bytes());
            bx(b"iloc", &p)
        };
        let meta_len = |iloc: &Vec<u8>| 8 + 4 + iinf.len() + iloc.len() + iprp.len();
        let probe = make(0);
        let off = (ftyp.len() + meta_len(&probe) + 8) as u32;
        let iloc = make(off);
        let mut meta_p = vec![0, 0, 0, 0];
        meta_p.extend_from_slice(&iinf);
        meta_p.extend_from_slice(&iloc);
        meta_p.extend_from_slice(&iprp);
        let mut file = ftyp.clone();
        file.extend_from_slice(&bx(b"meta", &meta_p));
        file.extend_from_slice(&bx(b"mdat", &item));
        let m = read(&file);
        assert_eq!(m.icc.as_deref().map(|v| v.len()), Some(300));
        assert_eq!(m.exif.as_deref().map(orientation), Some(6));
    }
}
