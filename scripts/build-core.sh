#!/bin/bash
# Builds the Rust engine (rust/crates/pb-ffi) for iOS and writes:
#   RustCore/PixelBoostCore.xcframework   static libs + C header/module map
#   RustCore/PixelBoostCore.swift         UniFFI-generated Swift API
# Needs macOS with Xcode. Also runs the engine's tests on the host (Apple
# Silicon), which exercises the NEON convolution kernel.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUST="$ROOT/rust"
OUT="$ROOT/RustCore"
export IPHONEOS_DEPLOYMENT_TARGET=17.0

cd "$RUST"
rustup target add aarch64-apple-ios aarch64-apple-ios-sim

if [ "${SKIP_TESTS:-0}" != "1" ]; then
  echo "==> tests (host: $(uname -m))"
  cargo test --release -p pb-nn -p pb-core
fi

for t in aarch64-apple-ios aarch64-apple-ios-sim; do
  echo "==> $t"
  cargo rustc --release -p pb-ffi --lib --crate-type staticlib --target "$t"
done

echo "==> Swift bindings"
cargo build --release -p pb-ffi --lib
GEN="$RUST/target/uniffi-gen"
rm -rf "$GEN" && mkdir -p "$GEN/include"
cargo run --release -p pb-ffi --features bindgen --bin uniffi-bindgen -- \
  generate --library "$RUST/target/release/libpixelboost_core.dylib" --language swift \
  --config "$RUST/crates/pb-ffi/uniffi.toml" --out-dir "$GEN"
cp "$GEN/PixelBoostCoreFFI.h" "$GEN/include/"
cp "$GEN/PixelBoostCoreFFI.modulemap" "$GEN/include/module.modulemap"

echo "==> XCFramework"
rm -rf "$OUT" && mkdir -p "$OUT"
xcodebuild -create-xcframework \
  -library "$RUST/target/aarch64-apple-ios/release/libpixelboost_core.a" -headers "$GEN/include" \
  -library "$RUST/target/aarch64-apple-ios-sim/release/libpixelboost_core.a" -headers "$GEN/include" \
  -output "$OUT/PixelBoostCore.xcframework"
cp "$GEN/PixelBoostCore.swift" "$OUT/"
du -sh "$OUT"/*
