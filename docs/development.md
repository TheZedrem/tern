# Development

How to get a working dev setup. Day-to-day commands are in
[`AGENTS.md` §Operations](../AGENTS.md#operations).

## Prerequisites

- Rust (stable, edition 2024) with `rustfmt` and `clippy`
  - **Fedora:** `sudo dnf install rust cargo rustfmt clippy`
  - elsewhere: `rustup component add rustfmt clippy`
- **Linux:** a Wayland session (X11 is not supported) and a Vulkan driver.
  The build links only `libxkbcommon`; the Wayland client and the Vulkan
  loader are loaded at runtime.
  - **Fedora:** `sudo dnf install libxkbcommon-devel vulkan-loader mesa-vulkan-drivers`
  - **Debian/Ubuntu:** `sudo apt install libxkbcommon-dev libvulkan1 mesa-vulkan-drivers`
- **Windows 11:** the MSVC toolchain (Visual Studio Build Tools).
- **macOS:** Xcode Command Line Tools (compile-only platform).

## First run

```sh
cargo run
```

The TTFP badge in the window corner is the startup time of this run. See
[spec 0001](../spec/0001-startup-performance.md) for what it measures, and
use `cargo build --release && target/release/zed-term --bench-startup 10`
for numbers you can compare.

## Tests

`cargo test` runs everything that needs no window. `cargo test -- --ignored`
adds the live startup test, which opens a real window and needs a Wayland
session.

## Where things are written at runtime

| File | Path (Linux / Windows) |
|---|---|
| Config | `~/.config/zed-term/config.yaml` / `%APPDATA%\zed-term\config.yaml` |
| User themes | `…/zed-term/themes/*.yaml` |
| Session state | `~/.local/share/zed-term/session.json` / `%LOCALAPPDATA%\zed-term\session.json` |
