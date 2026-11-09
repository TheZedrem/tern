---
status: later
blocked-by: []
---
# T-040: A Windows-ready tern.exe

Launching `tern.exe` on Windows 11 opens Tern's window with its icon and no
console window, and runs without the VC++ runtime installed. Area: src.

- [x] `#![windows_subsystem = "windows"]` for release builds; `--bench-startup`
  and boot errors attach to the parent console (2026-11-09).
- `build.rs` embeds `assets/icon.ico` and version info on Windows only
  (needs a build-only crate; owner approves at spec review).
- `.cargo/config.toml`: `+crt-static` for `windows-msvc`.
- Check `cmd`, PowerShell and `ssh` in a pane, and report the Windows TTFP
  before and after.

Spec: ../spec/0012-windows-installer.md
