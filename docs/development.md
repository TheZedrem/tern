# Development

How to get a working dev setup. Day-to-day commands are in
[`AGENTS.md` §Operations](../AGENTS.md#operations).

## Prerequisites

- Rust (stable, edition 2024) with `rustfmt` and `clippy`
  - **Fedora:** `sudo dnf install rust cargo rustfmt clippy`
  - elsewhere: `rustup component add rustfmt clippy`
- Linux build dependencies for `winit`/`wgpu`:
  - **Fedora:** `sudo dnf install libxkbcommon-devel wayland-devel libX11-devel libXcursor-devel libXrandr-devel libXi-devel mesa-vulkan-drivers`
  - **Debian/Ubuntu:** `sudo apt install libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev mesa-vulkan-drivers`
- **Windows:** the MSVC toolchain (Visual Studio Build Tools). ConPTY needs
  Windows 10 1809 or newer.
- **macOS:** Xcode Command Line Tools (compile-only platform).

## First run

```sh
cargo run
```

The TTFP badge in the window corner is the startup time of this run. See
[spec 0001](../spec/0001-startup-performance.md) for what it measures, and
use `cargo build --release && target/release/zed-term --bench-startup 10`
for numbers you can compare.

## Where things are written at runtime

| File | Path (Linux / Windows) |
|---|---|
| Config | `~/.config/zed-term/config.yaml` / `%APPDATA%\zed-term\config.yaml` |
| User themes | `…/zed-term/themes/*.yaml` |
| Session state | `~/.local/share/zed-term/session.json` / `%LOCALAPPDATA%\zed-term\session.json` |
