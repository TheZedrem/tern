---
status: next
blocked-by: []
---
# T-011: Verify the scaffold, tabs and splits on Windows 11

T-010 and T-003 were built and verified on Linux (Wayland) only; their
Windows code paths compile only in theory. On a Windows 11 machine: build
release, run `zed-term --bench-startup 10` and record the numbers, then
check by hand that the default shell is found (pwsh, powershell, cmd),
typing works including AltGr characters on a German layout (`@`, `{`, `€`),
resizing the window with `vim` open redraws cleanly under ConPTY, and
`exit 0` closes while `exit 3` stays open. For T-003: the tab and split
keys work and `Alt+1`–`9` and `Alt+arrows` hit no window-menu accelerator,
dragging a divider redraws both panes cleanly, closing a pane ends its
shell, and a split opens in the home directory (there is no `/proc` cwd).
For T-005: the config is read from `%APPDATA%\zed-term\config.yaml`, saving
it from VS Code and from Notepad reloads it within a second, and a profile
with `command: [htop]`-style program names resolves through `PATH` and
`PATHEXT`. For T-004: `Ctrl+Shift+P` opens the palette, and `Ctrl+=`,
`Ctrl+-` and `Ctrl+0` change the font size on a US and on a German layout
(GPUI's Windows key mapper rewrites bindings per layout). Fix what fails.
Area: src.

Spec: ../spec/0001-startup-performance.md (AC8)
Spec: ../spec/0002-terminal-pane.md (spawn, input, resize, exit on Windows)
Spec: ../spec/0003-tabs-splits-and-session-restore.md (keys, resize, close)
Spec: ../spec/0004-commands-keybindings-palette.md (palette and font keys)
Spec: ../spec/0005-config-and-themes.md (paths, watcher, PATHEXT)
