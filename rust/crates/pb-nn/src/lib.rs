//! pb-nn — PixelBoost's neural-network inference library.
//!
//! Everything a super-resolution model needs, written for this project:
//! padded channel-last feature maps, register-tiled SIMD 3x3 convolutions
//! with fused epilogues, a PyTorch checkpoint reader, the `.pbnn` model
//! format and the RRDBNet / SRVGGNetCompact architectures.

pub mod arch;
pub mod conv;
pub mod fmap;
pub mod half;
pub mod pbm;
pub mod pth;

pub use arch::{SrNetwork, build};
pub use fmap::Fmap;

/// Load a `.pbnn` model from bytes.
pub fn load_model(bytes: &[u8]) -> Result<(Box<dyn SrNetwork>, pbm::Header), String> {
    let w = pbm::read(bytes)?;
    let net = arch::build(&w)?;
    Ok((net, w.header))
}
