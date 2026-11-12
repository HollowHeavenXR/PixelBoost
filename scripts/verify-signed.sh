#!/bin/bash
# Fail unless the app and every extension are signed and carry the App
# Group, and the expected extensions are inside the bundle.
# usage: scripts/verify-signed.sh path/to/PixelBoost.app
set -euo pipefail
APP="$1"
GROUP="group.com.pixelboost.shared"
codesign --verify --deep --strict "$APP" || { echo "::error::signature check failed for $APP"; exit 1; }
for want in PixelBoostWidgets PixelBoostShare; do
  ls -d "$APP"/PlugIns/"$want"*.appex >/dev/null 2>&1 || { echo "::error::$want extension missing from PlugIns/"; exit 1; }
done
for b in "$APP" "$APP"/PlugIns/*.appex; do
  [ -e "$b" ] || continue
  codesign -d --entitlements :- "$b" 2>/dev/null | grep -q "$GROUP" \
    || { echo "::error::App Group missing from $b"; exit 1; }
  echo "signed + App Group: $(basename "$b")"
done
