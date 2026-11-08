# Models

`pbnn/` holds the networks the app ships, in PixelBoost's own `.pbnn`
format (JSON header + fp16 tensors, see `rust/crates/pb-nn/src/pbm.rs`).
They run on PixelBoost's own inference engine (`rust/crates/pb-nn`):
hand-written AVX2/NEON 3x3 convolution kernels, no Core ML / ONNX / PyTorch.

| File | Id | Architecture | Best for |
|---|---|---|---|
| `anime-x4.pbnn` | anime | RRDBNet, 6 blocks | Drawn art, anime, cel shading |
| `photo-x4.pbnn` | photo | RRDBNet, 23 blocks | Photos, game screenshots, 3D renders |
| `clean-x4.pbnn` | clean | RRDBNet, 23 blocks (no GAN loss) | Faces, anything where invented texture looks wrong |
| `fast-x4.pbnn` (+ `fast-denoise-x4.pbnn`) | fast | SRVGGNetCompact, 32 convs | Quick everyday upscale; the two are blended for the denoise slider |
| `native-x2.pbnn` | native2x | RRDBNet, 23 blocks, pixel-unshuffle input | 2x of photos |
| `lineart-x4.pbnn` | lineart | SRVGGNetCompact, 16 convs | Anime video frames, line art |

Reproduce from the official checkpoints (BSD-3-Clause, see
`THIRD_PARTY_NOTICES.md`):

```
cd rust
mkdir -p weights-src && cd weights-src
B=https://github.com/xinntao/Real-ESRGAN/releases/download
for u in v0.1.0/RealESRGAN_x4plus.pth v0.2.2.4/RealESRGAN_x4plus_anime_6B.pth \
         v0.1.1/RealESRNet_x4plus.pth v0.2.5.0/realesr-general-x4v3.pth \
         v0.2.5.0/realesr-general-wdn-x4v3.pth v0.2.1/RealESRGAN_x2plus.pth \
         v0.2.5.0/realesr-animevideov3.pth; do curl -sLO $B/$u; done
cd .. && cargo run --release -p pb-cli -- convert-all weights-src ../Models/pbnn
```
