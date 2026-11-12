# scripts/ — to do

- `build-core.sh` — Rust engine -> RustCore/PixelBoostCore.xcframework (macOS CI).
- `sign-adhoc.sh` — ad-hoc signs the built app: frameworks, then every
  .appex with the App Group (bundle IDs checked to be under the app's),
  then the app; ends with `verify-signed.sh`.
- `verify-signed.sh` — fails unless app + Share + Widgets are signed and
  carry `group.com.pixelboost.shared`. Used before packaging and on the
  IPA downloaded back from the release.

- [x] Build and release workflows both sign + verify (2026-11-12).
- [ ] Real-certificate path if signing secrets are ever added
      (IOS_CERTIFICATE_BASE64 etc.): sign with the identity instead of `-`.
