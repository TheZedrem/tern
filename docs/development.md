# Development

How to get a working dev setup. Day-to-day commands are in
[`AGENTS.md` §Operations](../AGENTS.md#operations).

## Prerequisites

- Rust (stable, edition 2024) with `rustfmt` and `clippy`
  - **Fedora:** `sudo dnf install rust cargo rustfmt clippy`
  - elsewhere: `rustup component add rustfmt clippy`
- **Linux:** a Wayland session (X11 is not supported) and a Vulkan driver.
  The build links only `libxkbcommon`, but GPUI's font loader also needs
  fontconfig's pkg-config file at build time (the binary doesn't link it).
  The Wayland client and the Vulkan loader are loaded at runtime.
  - **Fedora:** `sudo dnf install libxkbcommon-devel fontconfig-devel vulkan-loader mesa-vulkan-drivers`
  - **Debian/Ubuntu:** `sudo apt install libxkbcommon-dev libfontconfig-dev libvulkan1 mesa-vulkan-drivers`
- **Windows 11:** the MSVC toolchain (Visual Studio Build Tools).
- **macOS:** Xcode Command Line Tools (compile-only platform). GPUI
  compiles its shaders with `xcrun metal` at build time, which only full
  Xcode ships (`xcodebuild -downloadComponent MetalToolchain`). Without it,
  build with `--features runtime-shaders`, which compiles them at every
  launch instead and costs startup time; `packaging/macos/install.sh` does
  this on its own, with a warning.

## First run

```sh
cargo run
```

The TTFP badge in the window corner is the startup time of this run. See
[spec 0001](../spec/0001-startup-performance.md) for what it measures, and
use `cargo build --release && target/release/tern --bench-startup 10`
for numbers you can compare. With `TERN_BENCH_SESSION=<session.json>`
the bench restores that session instead of starting one pane, to measure a
restored layout (spec 0003, AC7).

Sessions are saved in `~/.local/share/tern/session.json` (Linux). To
try one without touching yours, point `XDG_DATA_HOME` elsewhere.

To try the title bar Tern draws on GNOME (spec 0003) on any other
Wayland compositor, start it with `TERN_CLIENT_DECORATIONS=1`.

To try a config without touching your own, point `XDG_CONFIG_HOME`
elsewhere (Linux): `XDG_CONFIG_HOME=/tmp/zt cargo run` reads and watches
`/tmp/zt/tern/config.yaml`.

To run this checkout as `tern` from anywhere, `packaging/linux/install.sh`
(or `packaging/macos/install.sh`) builds the release binary and links it into
`${XDG_BIN_HOME:-~/.local/bin}`. The macOS script also writes an ad-hoc
signed `~/Applications/Tern.app` with a copy of the binary. Re-run it after
pulling to update.

## Tests

`cargo test` runs everything that needs no window. `cargo test -- --ignored`
adds the live startup test, which opens a real window and needs a Wayland
session.

## Where things are written at runtime

| File | Path (Linux / Windows) |
|---|---|
| Config | `~/.config/tern/config.yaml` / `%APPDATA%\tern\config.yaml` |
| User themes | `…/tern/themes/*.yaml` |
| Session state | `~/.local/share/tern/session.json` / `%LOCALAPPDATA%\tern\session.json` |

## Linux packages

A `v*` tag matching `Cargo.toml`'s version builds the `.deb`, `.rpm` and
AppImage in CI (`.github/workflows/release.yml`) once the CI checks pass,
and attaches them to a GitHub release. To build them locally after `cargo build --release`:

```sh
cargo install --locked cargo-deb cargo-generate-rpm   # once
cargo deb --no-build         # target/debian/tern_<version>-1_amd64.deb
cargo generate-rpm           # target/generate-rpm/tern-<version>-1.x86_64.rpm
packaging/linux/appimage.sh <path to appimagetool>   # target/packages/Tern-<version>-x86_64.AppImage
```

`appimagetool` comes from
[AppImage/appimagetool](https://github.com/AppImage/appimagetool/releases);
it downloads the AppImage runtime while it runs. A local build links
against your distro's glibc, so its packages only install on that distro
version or newer. The release packages are built on Ubuntu 24.04 and need
glibc 2.39.
