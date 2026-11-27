#!/usr/bin/env bash
# Pack the Linux build: binary, desktop entry, icon, licences.
#
#   scripts/package-linux.sh <qell binary> <version> <arch> <out dir>
set -euo pipefail

bin=$1
version=$2
arch=$3
out=$4
root=$(cd "$(dirname "$0")/.." && pwd)
name="qell-$version-linux-$arch"
dir="$out/$name"

rm -rf "$dir"
mkdir -p "$dir"
cp "$bin" "$dir/qell"
cp "$root/packaging/qell.desktop" "$dir/"
cp "$root/crates/qell-app/assets/icon/qell-256.png" "$dir/qell.png"
cp "$root/README.md" "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$dir/"
cat > "$dir/install.sh" <<'SH'
#!/usr/bin/env sh
# Install Qell for you (~/.local): the binary, its menu entry and icon.
set -e
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" "$HOME/.local/share/icons/hicolor/256x256/apps"
cp "$here/qell" "$HOME/.local/bin/qell"
cp "$here/qell.desktop" "$HOME/.local/share/applications/qell.desktop"
cp "$here/qell.png" "$HOME/.local/share/icons/hicolor/256x256/apps/qell.png"
echo "Installed to ~/.local/bin/qell (make sure ~/.local/bin is on your PATH)."
SH
chmod +x "$dir/install.sh" "$dir/qell"
tar -C "$out" -czf "$out/$name.tar.gz" "$name"
echo "$out/$name.tar.gz"
