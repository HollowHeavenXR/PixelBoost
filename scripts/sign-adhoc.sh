#!/bin/bash
# Ad-hoc sign a built PixelBoost.app (no certificate) with only the App
# Group entitlement, inner code first, then verify every part.
#
# SideStore, AltStore, Sideloadly, LiveContainer and free Apple IDs copy the
# App Group from the IPA's signature when they re-sign; an unsigned IPA has
# none, so widgets / the Share extension couldn't share data with the app.
# Only the App Group is embedded (no push, HealthKit, ...) so free Apple IDs
# can still install it.
#
# usage: scripts/sign-adhoc.sh path/to/PixelBoost.app
set -euo pipefail
APP="$1"
GROUP="group.com.pixelboost.shared"
ENT="$(mktemp -t pb-ent).plist"
cat > "$ENT" <<ENT_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.application-groups</key>
    <array>
        <string>$GROUP</string>
    </array>
</dict>
</plist>
ENT_EOF

APP_ID=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Info.plist")

# 1. Frameworks and dylibs (no entitlements).
find "$APP" \( -name '*.framework' -o -name '*.dylib' \) -prune -print | while read -r f; do
  codesign -f -s - "$f"
done
# 2. Every extension, each with the App Group; bundle IDs must sit under the app's.
find "$APP" -name '*.appex' -prune -print | while read -r ext; do
  EXT_ID=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$ext/Info.plist")
  case "$EXT_ID" in
    "$APP_ID".*) ;;
    *) echo "::error::$ext bundle id $EXT_ID is not prefixed by $APP_ID"; exit 1 ;;
  esac
  codesign -f -s - --entitlements "$ENT" "$ext"
done
# 3. The app itself.
codesign -f -s - --entitlements "$ENT" "$APP"

bash "$(dirname "$0")/verify-signed.sh" "$APP"
