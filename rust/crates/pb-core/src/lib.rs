//! pb-core — the PixelBoost engine.

pub mod analyze;
pub mod classify;
pub mod compose;
pub mod develop;
pub mod draw;
pub mod edit;
pub mod engine;
pub mod enhance;
pub mod fidelity;
pub mod filters;
pub mod fx;
pub mod image;
pub mod mask;
pub mod meta;
pub mod metrics;
pub mod models;
pub mod ops;
pub mod pipeline;
pub mod presets;
pub mod resample;
pub mod shape;
pub mod stream;
pub mod text;

pub use engine::{Engine, TileEvent, TileOptions};
pub use image::Image;

/// Which SIMD convolution kernel is active.
pub fn kernel_name() -> &'static str {
    pb_nn::conv::kernel_name()
}
