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

case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "Note: $bin_dir isn't on your PATH." >&2 ;;
esac
