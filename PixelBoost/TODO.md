# PixelBoost/ (SwiftUI shell) — to do

Overall tracker: ../RUST-REMAKE.md. Swift here is screens only; every pixel
operation is a call into PixelBoostCore (Rust).

- [x] Builds in CI; v5.0.0 released. Needs a test on a real iPhone.
- [x] Sideload widgets/share: App Group resolved at runtime (`Shared/AppGroup.swift`: literal or `.<TeamID>` suffix) and release IPAs ad-hoc signed with `group.com.pixelboost.shared` so SideStore/AltStore/Sideloadly register it (v5.1.1).
- [ ] Verify on device: widget shows last upscale and Share extension hands photos over on a SideStore install.
- [ ] Compare-models sweep screen (server already logs /log/comparison).
- [ ] Pinch-zoom pixel inspector on results.
- [ ] Background-task continuation for long upscales.
