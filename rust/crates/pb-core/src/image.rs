//! In-memory image: straight (un-premultiplied) RGBA, f32 in [0, 1], values
//! kept in the file's own encoding (sRGB-encoded, which is what every
//! bundled model was trained on).

use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct Image {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 4]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
}

impl Format {
    pub fn from_path(p: &str) -> Format {
        let l = p.to_ascii_lowercase();
        if l.ends_with(".jpg") || l.ends_with(".jpeg") {
            Format::Jpeg
        } else if l.ends_with(".webp") {
            Format::Webp
        } else {
            Format::Png
        }
    }
}

/// Width and height from the file header, without decoding pixels.
pub fn dimensions(bytes: &[u8]) -> Result<(usize, usize), String> {
    let r = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    r.into_dimensions().map(|(w, h)| (w as usize, h as usize)).map_err(|e| e.to_string())
}

impl Image {
    pub fn new(w: usize, h: usize) -> Self {
        Image { w, h, px: vec![[0.0, 0.0, 0.0, 1.0]; w * h] }
    }

    #[inline(always)]
    pub fn at(&self, x: usize, y: usize) -> [f32; 4] {
        self.px[y * self.w + x]
    }

    /// Clamp-to-edge sample.
    #[inline(always)]
    pub fn at_clamped(&self, x: isize, y: isize) -> [f32; 4] {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.px[y * self.w + x]
    }

    pub fn has_alpha(&self) -> bool {
        self.px.par_iter().any(|p| p[3] < 0.999)
    }

    pub fn decode(bytes: &[u8]) -> Result<Image, String> {
        let img = image::load_from_memory(bytes).map_err(|e| format!("decode: {e}"))?;
        Ok(Self::from_dynamic(img))
    }

    pub fn open(path: &str) -> Result<Image, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        Self::decode(&bytes)
    }

    fn from_dynamic(img: image::DynamicImage) -> Image {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let px = match img {
            image::DynamicImage::ImageRgba16(b) => b.as_raw().par_chunks_exact(4).map(|c| [c[0] as f32 / 65535.0, c[1] as f32 / 65535.0, c[2] as f32 / 65535.0, c[3] as f32 / 65535.0]).collect(),
            image::DynamicImage::ImageRgb16(b) => b.as_raw().par_chunks_exact(3).map(|c| [c[0] as f32 / 65535.0, c[1] as f32 / 65535.0, c[2] as f32 / 65535.0, 1.0]).collect(),
            other => {
                let b = other.to_rgba8();
                b.as_raw().par_chunks_exact(4).map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]).collect()
            }
        };
        Image { w, h, px }
    }

    pub fn to_rgba8(&self) -> Vec<u8> {
        self.px.par_iter().flat_map_iter(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect()
    }

    pub fn encode(&self, fmt: Format, quality: u8) -> Result<Vec<u8>, String> {
        use image::ImageEncoder;
        let mut out = Vec::new();
        let (w, h) = (self.w as u32, self.h as u32);
        let alpha = self.has_alpha();
        let rgba = self.to_rgba8();
        let (buf, ct) = if alpha || fmt == Format::Webp {
            (rgba, image::ExtendedColorType::Rgba8)
        } else {
            (rgba.chunks_exact(4).flat_map(|c| [c[0], c[1], c[2]]).collect(), image::ExtendedColorType::Rgb8)
        };
        match fmt {
            Format::Png => image::codecs::png::PngEncoder::new_with_quality(&mut out, image::codecs::png::CompressionType::Fast, image::codecs::png::FilterType::Adaptive)
                .write_image(&buf, w, h, ct)
                .map_err(|e| e.to_string())?,
            Format::Jpeg => {
                let rgb: Vec<u8> = if ct == image::ExtendedColorType::Rgba8 { buf.chunks_exact(4).flat_map(|c| [c[0], c[1], c[2]]).collect() } else { buf };
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).write_image(&rgb, w, h, image::ExtendedColorType::Rgb8).map_err(|e| e.to_string())?
            }
            Format::Webp => image::codecs::webp::WebPEncoder::new_lossless(&mut out).write_image(&buf, w, h, ct).map_err(|e| e.to_string())?,
        }
        Ok(out)
    }

    pub fn save(&self, path: &str) -> Result<(), String> {
        let bytes = self.encode(Format::from_path(path), 95)?;
        std::fs::write(path, bytes).map_err(|e| format!("{path}: {e}"))
    }

    pub fn crop(&self, x: usize, y: usize, w: usize, h: usize) -> Image {
        let mut px = Vec::with_capacity(w * h);
        for yy in y..y + h {
            px.extend_from_slice(&self.px[yy * self.w + x..yy * self.w + x + w]);
        }
        Image { w, h, px }
    }

    pub fn paste(&mut self, src: &Image, x: usize, y: usize) {
        for yy in 0..src.h {
            let d = (y + yy) * self.w + x;
            self.px[d..d + src.w].copy_from_slice(&src.px[yy * src.w..(yy + 1) * src.w]);
        }
    }

    /// BT.709 luma of the encoded values.
    #[inline(always)]
    pub fn luma(p: [f32; 4]) -> f32 {
        0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
    }

    pub fn flip_h(&self) -> Image {
        let mut o = self.clone();
        for row in o.px.chunks_mut(self.w) {
            row.reverse();
        }
        o
    }

    pub fn flip_v(&self) -> Image {
        let mut o = Image::new(self.w, self.h);
        for y in 0..self.h {
            o.px[y * self.w..(y + 1) * self.w].copy_from_slice(&self.px[(self.h - 1 - y) * self.w..(self.h - y) * self.w]);
        }
        o
    }

    pub fn transpose(&self) -> Image {
        let mut o = Image::new(self.h, self.w);
        for y in 0..self.h {
            for x in 0..self.w {
                o.px[x * self.h + y] = self.px[y * self.w + x];
            }
        }
        o
    }

    /// The 8 dihedral transforms (k = 0..8): bit0 flip_h, bit1 flip_v, bit2 transpose first.
    pub fn dihedral(&self, k: usize) -> Image {
        let mut o = if k & 4 != 0 { self.transpose() } else { self.clone() };
        if k & 1 != 0 {
            o = o.flip_h();
        }
        if k & 2 != 0 {
            o = o.flip_v();
        }
        o
    }

    pub fn dihedral_inverse(&self, k: usize) -> Image {
        let mut o = self.clone();
        if k & 2 != 0 {
            o = o.flip_v();
        }
        if k & 1 != 0 {
            o = o.flip_h();
        }
        if k & 4 != 0 {
            o = o.transpose();
        }
        o
    }
}
