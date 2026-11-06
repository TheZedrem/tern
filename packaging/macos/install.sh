#!/usr/bin/env bash
# Builds the release binary, links it into the user's bin folder so `tern`
# runs this checkout's build, and puts a Tern.app in ~/Applications for
# Spotlight and the Dock. Re-run after pulling to update.
# Usage: packaging/macos/install.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

bin_dir=${XDG_BIN_HOME:-$HOME/.local/bin}
link=$bin_dir/tern
app=$HOME/Applications/Tern.app
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)

# GPUI compiles its shaders with Xcode's `metal`. Without it, fall back to
# compiling them at every launch, which slows startup.
features=()
if ! xcrun -f metal >/dev/null 2>&1; then
    echo "Warning: Xcode's metal compiler isn't installed; using the fallback" \
        "that compiles shaders at launch (slower startup)." >&2
    features=(--features runtime-shaders)
fi
cargo build --locked --release "${features[@]}"

# Never replace a real file: that's likely a packaged Tern the user installed.
if [[ -e $link && ! -L $link ]]; then
    echo "$link exists and isn't a symlink; remove it first." >&2
    exit 1
fi
if [[ -e $app ]] && ! plutil -extract CFBundleIdentifier raw "$app/Contents/Info.plist" 2>/dev/null | grep -qx de.zedrem.tern; then
    echo "$app isn't Tern; remove it first." >&2
    exit 1
fi

mkdir -p "$bin_dir"
ln -sfn "$PWD/target/release/tern" "$link"
echo "Linked $link -> $PWD/target/release/tern"

# The app gets a copy, not a symlink: macOS finds the bundle (and its icon
# and bundle ID) from the executable's real path.
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/tern "$app/Contents/MacOS/tern"

iconset=$(mktemp -d)/tern.iconset
mkdir "$iconset"
for size in 16 32 128 256 512; do
    sips -z $size $size assets/icon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) assets/icon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/tern.icns"

cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Tern</string>
    <key>CFBundleIdentifier</key><string>de.zedrem.tern</string>
    <key>CFBundleExecutable</key><string>tern</string>
    <key>CFBundleIconFile</key><string>tern</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
# Ad-hoc signature over the whole bundle, so it isn't flagged as damaged.
codesign --force --sign - "$app"
echo "Installed $app"

case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "Note: $bin_dir isn't on your PATH." >&2 ;;
esac
