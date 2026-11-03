# Development

How to get a working dev setup. Day-to-day commands are in
[`AGENTS.md` §Operations](../AGENTS.md#operations).

## Prerequisites

- Rust (stable, edition 2024), Node (current LTS), pnpm
- Tauri's system dependencies:
  - **Fedora:** `sudo dnf install webkit2gtk4.1-devel openssl-devel libappindicator-gtk3-devel librsvg2-devel libxdo-devel`
  - **Debian/Ubuntu:** `sudo apt install libwebkit2gtk-4.1-dev build-essential libssl-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev`
  - **Windows:** Microsoft C++ Build Tools and WebView2 (preinstalled on Windows 11)
  - **macOS:** Xcode Command Line Tools
- Full list: <https://v2.tauri.app/start/prerequisites/>

## First run

```sh
pnpm install
pnpm tauri dev
```

The TTFP badge in the window corner is the startup time of this run. See
[spec 0001](../spec/0001-startup-performance.md) for what it measures, and
use `pnpm bench:startup` for numbers you can compare.

## Where things are written at runtime

| File | Path (Linux / Windows) |
|---|---|
| Config | `~/.config/zed-term/config.yaml` / `%APPDATA%\zed-term\config.yaml` |
| User themes | `…/zed-term/themes/*.yaml` |
| Session state | `~/.local/share/zed-term/session.json` / `%APPDATA%\zed-term\session.json` |
