# rust/ — to do

Overall tracker: ../RUST-REMAKE.md

- [ ] GPU backend for pb-nn (wgpu compute: Metal on iOS, Vulkan here) — the
      23-block models are ~18 MMAC per input pixel; CPU-only is ~35 s for a
      480x270 photo on this PC and slower on a phone.
- [ ] fp16 / int8 NEON kernel variants (halve weight bandwidth on iPhone).
- [ ] Own segmentation network for Cutout (needs strided conv, pooling,
      bilinear upsample in pb-nn).
- [ ] Render-denoise network (OIDN U-Net) — same new ops as above.
- [ ] Perceptual metric for `pixelboost eval` (PSNR/SSIM favour blur).
- [ ] Classifier: learn thresholds from more labelled gallery images
      (`pixelboost classify`).

## Photo editor (5.2)

- [x] filters / develop / mask / fx / draw / text / compose / presets
      modules + pb-ffi editor API (see ../RUST-REMAKE.md, 5.2).
- [ ] Zoom / spin blur are the slowest previews (~140 ms at 1280 px here):
      sample count could scale with the streak length.
- [ ] Full-resolution edits hold the image as f32 RGBA (16 B/px): a 48 MP
      photo needs ~770 MB per copy. Stream / tile the pointwise ops or use
      f16 planes for big images.
- [ ] Healing brush on top of inpaint (texture-aware patch match).
- [ ] Own subject segmentation (Portrait blur and Cutout use Vision now).
- [ ] Text: kerning is per pair only (no shaping) — Arabic / Indic scripts
      need a shaper (rustybuzz).
