#!/usr/bin/env bash
# Build Qell.app from a compiled binary, and zip it.
#
#   scripts/bundle-macos.sh <qell binary> <version> <out dir>
#
# The icon is made from assets/icon/qell-1024.png with sips and iconutil
# (both come with macOS). The app is signed ad hoc (no Apple account):
# macOS asks once before opening it — see the README.
set -euo pipefail

bin=$1
version=$2
out=$3
root=$(cd "$(dirname "$0")/.." && pwd)
app="$out/Qell.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/qell"
sed "s/@VERSION@/$version/g" "$root/packaging/Info.plist" > "$app/Contents/Info.plist"

iconset=$(mktemp -d)/qell.iconset
mkdir -p "$iconset"
src="$root/crates/qell-app/assets/icon/qell-1024.png"
for size in 16 32 128 256 512; do
  sips -z $size $size "$src" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z $double $double "$src" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/qell.icns"

codesign --force --deep --sign - "$app"
(cd "$out" && ditto -c -k --keepParent Qell.app "Qell-$version-macos-universal.zip")
echo "$out/Qell-$version-macos-universal.zip"
