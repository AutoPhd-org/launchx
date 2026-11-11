#!/bin/sh
# Build target/LaunchX.app: the macOS app with lx bundled inside it.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
app="$root/target/LaunchX.app"
work="$root/target/macos"
arch=$(uname -m)

cargo build --release --manifest-path "$root/Cargo.toml"
rm -rf "$app" "$work"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$work/AppIcon.iconset"

swiftc -O -parse-as-library -swift-version 5 -target "$arch-apple-macos14.0" \
  "$root/macos/LaunchX.swift" -o "$app/Contents/MacOS/LaunchX"
cp "$root/target/release/lx" "$app/Contents/MacOS/lx"
cp "$root/macos/Info.plist" "$app/Contents/Info.plist"

swiftc -O "$root/macos/icon.swift" -o "$work/icon"
"$work/icon" "$work/icon-1024.png"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$work/icon-1024.png" --out "$work/AppIcon.iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$work/icon-1024.png" --out "$work/AppIcon.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$work/AppIcon.iconset" -o "$app/Contents/Resources/AppIcon.icns"

# Ad-hoc signature: enough for a locally built app.
codesign --force --sign - "$app/Contents/MacOS/lx"
codesign --force --sign - "$app"
printf 'Built %s\n' "$app"
