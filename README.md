# PixelBoost 5

An iOS photo upscaler and editor, rebuilt from the ground up in Rust.

Everything that touches pixels — the AI upscaling, the neural-network
inference itself, fidelity refinement, and every editing tool — is
PixelBoost's own Rust code. There is no Core ML, ONNX Runtime, PyTorch or
other ML runtime in the app. SwiftUI is only the screen layer on top of the
Rust core (UniFFI), and the optional server is Rust too.

## Layout

```
rust/
  crates/pb-nn       inference library: padded channel-last feature maps,
                     register-tiled 6x16 3x3-conv kernels (AVX2+FMA, NEON,
                     portable), fused bias/activation/residual epilogues,
                     PyTorch .pth reader (own zip + pickle VM), .pbnn model
                     format (fp16), network blending (DNI), RRDBNet and
                     SRVGGNetCompact
  crates/pb-core     engine: image I/O, antialiased resampling, tiled model
                     runner (every source pixel goes through the network,
                     identical tiles run once), self-ensemble, fidelity
                     (guided back-projection + smooth halo guard), halo-free
                     detail boost, Auto model pick, metrics, and the photo
                     editor (edit, develop, mask, fx, draw, text, compose,
                     presets, filters)
  crates/pb-ffi      PixelBoostCore Swift module (UniFFI): upscale with live
                     tile previews + cancel, editing ops, presets, histogram,
                     eyedroppers, export, history store, server client
  crates/pb-cli      `pixelboost` CLI: convert, upscale, raw, bench, eval, classify
  crates/pb-server   upscaler-bridge server (axum + sqlx, see server/README.md)
Models/pbnn/         the bundled networks (.pbnn)
PixelBoost/          SwiftUI app shell (no image processing)
scripts/build-core.sh  builds RustCore/PixelBoostCore.xcframework for iOS
scripts/sign-adhoc.sh  ad-hoc signs the .app + extensions with the App Group
scripts/verify-signed.sh  CI check: every part signed and carries the App Group
```

## The upscaler

1. **Network** — one of six Real-ESRGAN-family networks (Auto picks drawn-art
   vs photo/render from image statistics) runs over every pixel in tiles
   with a 16px context margin, so seams see the same neighbourhood a
   whole-image pass would.
2. **Quality** — Standard (1 pass), High (2 mirrored passes averaged) or Max
   (8 rotations/mirrors averaged).
3. **Clarity / fidelity** — *Natural* keeps the network's crisp edges but
   restores colour, shading and faint detail it flattened: the result is
   shrunk back, compared with the source pixel by pixel, and the smooth part
   of the difference is pushed back in through a guided (edge-respecting)
   upsampler; a smoothly interpolated halo guard then clamps every output
   pixel to its source neighbourhood's range, removing outlines and ringing.
   *Faithful* corrects the full difference. *Raw AI* skips this.
4. **Detail** — halo-free sharpening: luma is boosted against a Gaussian
   base, gated by a noise floor, and limited to each pixel's own 3x3 range so
   it never overshoots.
5. Output size 2x / 3x / 4x / 8x (8x runs the network twice); alpha is
   preserved.

Measured on 26 gallery images (ground-truth crops shrunk 4x and restored,
`pixelboost eval`): every model + Natural beats bicubic and Lanczos on PSNR,
SSIM and edge similarity, and the fidelity stage adds ~+0.8–1.6 dB over the
raw network output. See `RUST-REMAKE.md` for the numbers.

## The photo editor (5.2) — all Rust, via one JSON op dispatcher (`pb_core::ops`)

The Edit mode is a full photo editor, not just an upscaler. Every tool
previews live on a 1280px copy (6–140 ms per change on a laptop CPU) and is
applied at full resolution; positions and sizes are normalised, so the
preview matches the export.

- **Presets & looks** — 12 built-in presets (Clean Pop, Teal & Orange,
  Moody Film, Cinematic, B&W Contrast, Vintage...) with live thumbnails and
  an amount slider; save your own edits as presets; copy / paste edits
  between photos; 13 Looks; `.cube` LUTs.
- **Light & colour** — exposure, brightness, contrast, highlights, shadows,
  whites, blacks; temperature, tint (with a white-balance eyedropper),
  vibrance, saturation; clarity, texture, dehaze, vignette (dark or light),
  fade, film grain; master + R/G/B tone curves; 8-band HSL colour mixer;
  colour-grading wheels (shadows / midtones / highlights / global, balance).
- **Local adjustments** — brush, linear gradient and radial masks with
  on-canvas handles (invert / affect outside); masks can also be luminance
  or colour ranges and combinations (`pb_core::mask`).
- **Retouch** — heal (inpainting), clone stamp, skin smoothing (guided
  filter, whole photo or painted), denoise + halo-free sharpen.
- **Geometry** — crop with 11 aspect ratios, rotate, flip, straighten,
  perspective (keystone) correction with a grid.
- **Blur** — soft (Gaussian), portrait (subject stays sharp, lens blur with
  bokeh highlights behind it), lens, tilt-shift, zoom, spin, motion.
- **Effects** — glow, chromatic aberration, halftone, duotone, posterize,
  sketch, glitch, oil paint (Kuwahara), mosaic, invert, solarize,
  threshold, emboss, light leak.
- **Create** — text (5 bundled Noto fonts, outline, shadow, background box,
  rotation, letter spacing), drawing (pen, marker, highlighter, neon,
  lines, arrows, boxes, ellipses), image layers / stickers / double
  exposure with 12 blend modes, frames (border, polaroid, film, card,
  fit-to-ratio, rounded corners), cutout with backgrounds, pixel art,
  seamless textures, normal maps, AO and depth fog.
- **Workflow** — histogram with clipping warnings, press-and-hold
  before/after, pinch-zoom, full edit history (jump to any step), export to
  JPEG (quality) / PNG / WebP / 16-bit TIFF at original or reduced size.

Try any op from the CLI: `pixelboost edit in.jpg out.jpg '{"op":"hsl","sat":[0,0,0,-1,0,0,0,0]}'`
or `pixelboost edit in.jpg out.jpg preset:cinematic`.

## Build

```
cd rust && cargo build --release && cargo test --release
./target/release/pixelboost upscale in.jpg out.png --model auto
```

The iOS app is built in GitHub Actions: `scripts/build-core.sh` (Rust ->
xcframework, also runs the tests on Apple Silicon) then XcodeGen + xcodebuild.
Push to `main` builds an unsigned IPA; a `v*` tag publishes a release.
