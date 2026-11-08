# PixelBoost — Rust remake tracker

Goal: rebuild PixelBoost from the ground up in Rust, including the AI
upscaling itself (own inference library, own model format, own kernels — no
Core ML, ONNX Runtime, PyTorch or other ML runtime). Swift stays only as the
thin SwiftUI shell on iOS (same pattern as SwarmPanel/Nyxframe/Lumisound:
Rust core via UniFFI xcframework).

Test images: public images from https://gallery.xenusanimations.studio
(downloaded to `rust/testimages/src`, git-ignored).

## Layout

```
rust/
  crates/pb-nn     inference library: padded HWC feature maps, AVX2/NEON 6x16
                   register-tiled 3x3 conv with fused epilogues, .pth reader
                   (own zip + pickle VM), .pbnn model format (fp16), DNI blend,
                   RRDBNet + SRVGGNetCompact
  crates/pb-core   engine: image I/O, resampling, tiled model runner (dedups
                   identical tiles), fidelity (back-projection + halo guard),
                   halo-free detail boost, auto model pick, metrics, pipeline
  crates/pb-cli    `pixelboost` CLI: convert, upscale, raw, bench, eval
Models/pbnn/       converted models (fp16 .pbnn)
```

## Status

- [x] pb-nn conv kernels (AVX2+FMA, NEON, scalar) — tested against a naive conv
- [x] PyTorch checkpoint reader + `.pbnn` writer/reader, fp16 round-trip test
- [x] RRDBNet (x4plus, x2plus, anime_6B, RealESRNet) and SRVGGNetCompact
- [x] All 7 official Real-ESRGAN checkpoints converted (`pixelboost convert-all`)
- [x] Tiled engine, fidelity refinement, detail boost, pipeline, auto model pick
- [x] Test bench (`pixelboost eval`): ground-truth crops from gallery images
- [x] Fidelity tuned from bench + visual review (guided back-projection,
      smooth halo guard; Natural = smooth-only correction)
- [x] Self-ensemble (High = 2 passes, Max = 8)
- [x] Editing tools in Rust behind one JSON op dispatcher (`pb_core::ops`)
- [x] pb-ffi (UniFFI): upscale + live tiles + cancel, edits, history, server client
- [x] `scripts/build-core.sh` + CI steps (xcframework; tests run on Apple Silicon = NEON)
- [x] Swift shell rewritten (5 tabs: Upscale, Edit, Batch, History, Settings);
      old Swift services/views, Core ML models, Lua + TinyEXR vendoring removed
- [x] `server/` rewritten in Rust (`pb-server`), every endpoint tested against
      a scratch Postgres DB + the app's Rust client round-trip test
- [x] App builds in CI (one round of Swift fixes); released as v5.0.0
      (PixelBoost-5.0.0-unsigned.ipa) on 2026-11-08 — not yet tried on a device
- [x] Rust upscaler-bridge deployed on Mel 2026-11-08 (rollback image
      `music-upscaler-bridge:python-backup`, compose backup
      `docker-compose.yml.bak-pbrust-20261108`); verified with live data
- [x] Repo made public 2026-11-08 so macOS CI runs (billing had blocked it)
- [x] Auto pick is compute-budgeted: photo net up to 1 MP then "fast";
      anime net up to 4 MP then "lineart"
- [ ] GPU backend (wgpu -> Metal) for faster on-device runs of the 23-block models
- [ ] Own segmentation model for Cutout (currently Vision's subject mask)
- [ ] Own render-denoise model (old app had OIDN via Core ML; not ported yet)
- [ ] Scripted (Lua) filters from the old app: not ported; replace with a
      Rust expression filter if wanted
- [ ] Model compare sweep UI (old "Compare Models"); server endpoints still exist

## 5.1 (2026-11-08)

- [x] Models were missing from the IPA (resources phase doesn't run in CI) —
      workflows now copy Models/pbnn into the .app and check it
- [x] Streaming pipeline (`pb_core::stream`): model + fidelity + detail +
      resample per tile, written straight to 8-bit output. 1737x3088 -> 4x
      High (86 MP): 541 MB peak, 5 min on this PC; matches the in-memory
      pipeline to 69 dB. Output capped at 120 MP on phone / 300 MP server.
- [x] PixelBoost Cloud: `pb-worker` (this PC, docker `worker/`, jobs on
      /mnt/fastssd/pixelboost-jobs, 7-day results) + bridge on Mel streams
      `/jobs*` to it with X-Worker-Key. E2E tested over the public URL.
- [x] App redesign "Studio": one canvas, floating command bar
      (Upscale / Edit / Cloud), upscale sheet with This iPhone vs Cloud,
      Cloud screen with live jobs + full-res resumable download, on-device
      history, settings as a sheet, background check + "Upscale ready"
      notification.

## Bench numbers (26 gallery images, 256px GT crop -> 64px -> 4x)

Final settings (`pixelboost eval testimages/src OUT --crop 256`):

| variant | PSNR | SSIM | edge sim | grad ratio |
|---|---|---|---|---|
| bicubic | 22.20 | 0.692 | 0.800 | 0.55 |
| lanczos | 22.37 | 0.693 | 0.798 | 0.60 |
| anime raw | 21.11 | 0.693 | 0.794 | 0.85 |
| anime natural | 22.31 | 0.712 | 0.806 | 0.76 |
| anime faithful | 22.99 | 0.739 | 0.825 | 0.73 |
| photo natural | 23.26 | 0.735 | 0.828 | 0.75 |
| fast natural | 23.54 | 0.753 | 0.829 | 0.71 |
| clean faithful | 24.41 | 0.785 | 0.844 | 0.65 |

PSNR/SSIM reward smoothness ("clean" always wins them); the GAN models are
judged visually too (contact sheets in the eval output dir). Natural is
the default because it keeps the GAN's crisp edges while fixing colour.

Speed on this PC (Ryzen 3 7320U, 4C/8T, AVX2): ~195 GFLOP/s;
480x270 -> 4x: anime 16 s, photo 35 s.

## Commands

```
cd rust
cargo build --release
./target/release/pixelboost convert-all weights-src ../Models/pbnn
./target/release/pixelboost upscale in.jpg out.png --model auto
./target/release/pixelboost eval testimages/src /tmp/eval --crop 256 [--jpeg 85]
./target/release/pixelboost bench photo 96
```
