//! Bundled model catalogue. Every model is an official Real-ESRGAN release
//! (BSD-3-Clause) converted to `.pbnn` by `pixelboost convert`.

use serde::Serialize;

#[derive(Serialize, Clone, Copy, Debug)]
pub struct ModelInfo {
    pub id: &'static str,
    pub file: &'static str,
    pub name: &'static str,
    pub best_for: &'static str,
    pub scale: u32,
    /// Relative cost per input pixel (1.0 = the 23-block RRDB models).
    pub cost: f32,
    /// Source checkpoint, for `pixelboost convert --all`.
    pub source: &'static str,
}

pub const MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "anime",
        file: "anime-x4.pbnn",
        name: "Anime & Illustration",
        best_for: "Drawn art, anime, cel shading, line art — crisp lines, flat colour kept flat",
        scale: 4,
        cost: 0.28,
        source: "RealESRGAN_x4plus_anime_6B.pth",
    },
    ModelInfo {
        id: "photo",
        file: "photo-x4.pbnn",
        name: "Photo & Render",
        best_for: "Photos, game screenshots, 3D renders — strongest texture reconstruction",
        scale: 4,
        cost: 1.0,
        source: "RealESRGAN_x4plus.pth",
    },
    ModelInfo {
        id: "clean",
        file: "clean-x4.pbnn",
        name: "Clean / Faithful",
        best_for: "Faces and portraits, anything where invented texture would look wrong",
        scale: 4,
        cost: 1.0,
        source: "RealESRNet_x4plus.pth",
    },
    ModelInfo {
        id: "fast",
        file: "fast-x4.pbnn",
        name: "Fast General",
        best_for: "Everyday quick upscale; supports a denoise strength slider",
        scale: 4,
        cost: 0.07,
        source: "realesr-general-x4v3.pth",
    },
    ModelInfo {
        id: "fast-denoise",
        file: "fast-denoise-x4.pbnn",
        name: "Fast General (denoise weights)",
        best_for: "Blended with Fast General by the denoise slider",
        scale: 4,
        cost: 0.07,
        source: "realesr-general-wdn-x4v3.pth",
    },
    ModelInfo {
        id: "native2x",
        file: "native-x2.pbnn",
        name: "Native 2×",
        best_for: "2× upscales of photos — runs at the requested ratio directly",
        scale: 2,
        cost: 0.25,
        source: "RealESRGAN_x2plus.pth",
    },
    ModelInfo {
        id: "lineart",
        file: "lineart-x4.pbnn",
        name: "Anime Video / Line Art",
        best_for: "Anime video frames, flat line art, compressed screenshots — very fast",
        scale: 4,
        cost: 0.035,
        source: "realesr-animevideov3.pth",
    },
];

pub fn find(id: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.id == id)
}
