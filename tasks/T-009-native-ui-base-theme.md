---
status: later
blocked-by: [T-005]
---
# T-009: Match the system UI with a native base theme

Detect the desktop environment (Windows, macOS, GNOME, KDE) and pick a
matching base theme for the app chrome and settings UI (e.g. Fluent,
macOS, Adwaita, Breeze). The user can still override it in config. Needs a
spec first. Detection runs on the boot path, so it must be near-free:
`target_os` at compile time, plus `XDG_CURRENT_DESKTOP` on Linux, with no
D-Bus or portal calls before the first prompt. Area: src-tauri, src.

Spec: ../spec/architecture.md
