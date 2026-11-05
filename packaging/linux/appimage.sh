#!/usr/bin/env bash
# Packs the release binary into target/packages/Tern-<version>-x86_64.AppImage
# (T-026). Run `cargo build --locked --release` first.
# Usage: packaging/linux/appimage.sh [path to appimagetool]
set -euo pipefail
cd "$(dirname "$0")/../.."

tool=${1:-appimagetool}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
dir=$(mktemp -d)/Tern.AppDir
icons=usr/share/icons/hicolor/512x512/apps

install -Dm755 target/release/tern "$dir/usr/bin/tern"
install -Dm644 packaging/linux/de.zedrem.tern.desktop -t "$dir/usr/share/applications"
install -Dm644 assets/icon.png "$dir/$icons/de.zedrem.tern.png"
install -Dm644 LICENSE "$dir/usr/share/doc/tern/LICENSE"
install -Dm644 assets/fonts/lilex/OFL.txt "$dir/usr/share/doc/tern/Lilex-OFL.txt"
# appimagetool takes the desktop file and icon from the AppDir's root.
ln -s usr/share/applications/de.zedrem.tern.desktop "$dir/"
ln -s "$icons/de.zedrem.tern.png" "$dir/"
ln -s usr/bin/tern "$dir/AppRun"

mkdir -p target/packages
ARCH=x86_64 "$tool" --no-appstream "$dir" "target/packages/Tern-$version-x86_64.AppImage"
