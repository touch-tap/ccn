#!/bin/sh
# Builds "Claude Code Notifier.app", the helper that posts ccn's notifications on macOS with our icon:
#   scripts/build-macos-notifier.sh 0.2.0 dist            # ad-hoc signed, fine for local testing
#   CODESIGN_IDENTITY="Developer ID Application: …" scripts/build-macos-notifier.sh 0.2.0 dist
# Universal (arm64 + x86_64). Notarization is a separate step (see RELEASING.md).
set -eu
version=$1
out=$2
identity=${CODESIGN_IDENTITY:--}
root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/macos/notifier"
app="$out/Claude Code Notifier.app"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

for arch in arm64 x86_64; do
  swiftc -O -target "$arch-apple-macos11" -o "$work/ccn-notifier-$arch" "$src/main.swift"
done

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/ccn-notifier" "$work/ccn-notifier-arm64" "$work/ccn-notifier-x86_64"
sed "s/VERSION/$version/g" "$src/Info.plist" > "$app/Contents/Info.plist"
# The phone apps' icon (Icon Composer file copied from the iOS app), compiled to an .icns for older
# macOS plus Assets.car for the light, dark and tinted looks on macOS 26 and later
xcrun actool "$src/AppIcon.icon" --compile "$app/Contents/Resources" --platform macosx \
  --minimum-deployment-target 11.0 --app-icon AppIcon --output-partial-info-plist "$work/icon.plist" >/dev/null

if [ "$identity" = - ]; then
  codesign --force --sign - "$app"
else
  codesign --force --options runtime --timestamp --sign "$identity" "$app"
fi
codesign --verify --strict "$app"
echo "$app"
