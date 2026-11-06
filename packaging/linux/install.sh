#!/usr/bin/env bash
# Builds the release binary and links it into the user's bin folder, so
# `tern` runs this checkout's build. Re-run after pulling to update.
# Usage: packaging/linux/install.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

bin_dir=${XDG_BIN_HOME:-$HOME/.local/bin}
link=$bin_dir/tern

cargo build --locked --release

# Never replace a real file: that's likely a packaged Tern the user installed.
if [[ -e $link && ! -L $link ]]; then
    echo "$link exists and isn't a symlink; remove it first." >&2
    exit 1
fi

mkdir -p "$bin_dir"
ln -sfn "$PWD/target/release/tern" "$link"
echo "Linked $link -> $PWD/target/release/tern"

# Docks and taskbars find a window's icon through the desktop file named
# after its app ID. Exec gets the absolute link path, since a launcher's
# PATH may not include $bin_dir.
data_dir=${XDG_DATA_HOME:-$HOME/.local/share}
sed "s|^Exec=tern$|Exec=$link|" packaging/linux/de.zedrem.tern.desktop |
    install -Dm644 /dev/stdin "$data_dir/applications/de.zedrem.tern.desktop"
install -Dm644 assets/icon.png "$data_dir/icons/hicolor/512x512/apps/de.zedrem.tern.png"
echo "Installed the desktop entry and icon in $data_dir"

case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "Note: $bin_dir isn't on your PATH." >&2 ;;
esac
