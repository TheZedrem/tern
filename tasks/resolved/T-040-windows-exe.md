---
status: later
blocked-by: []
---
# T-040: A Windows-ready tern.exe

Launching `tern.exe` on Windows 11 opens Tern's window with its icon and no
console window, and runs without the VC++ runtime installed. Area: src.

- [x] `#![windows_subsystem = "windows"]` for release builds; `--bench-startup`
  and boot errors attach to the parent console (2026-11-09).
- [x] `build.rs` embeds `assets/icon.ico` and version info on Windows only,
  with `winresource` (owner's call, 2026-11-09).
- [x] `.cargo/config.toml`: `+crt-static` for `windows-msvc`; the release
  exe imports no VC++ or UCRT DLL.
- [x] TTFP on Windows 11, warm median of 10, back to back: 273.4 ms before,
  264.5 ms after (2026-11-09). The shell runs under a headless ConPTY
  `conhost`. Typing in `cmd`, PowerShell and `ssh` panes moved to T-052.

Spec: ../spec/0012-windows-installer.md
