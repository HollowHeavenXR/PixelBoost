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
