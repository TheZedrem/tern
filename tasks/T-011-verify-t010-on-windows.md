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
(GPUI's Windows key mapper rewrites bindings per layout). For T-012: an
SSH profile connects through the built-in `ssh.exe`, a dropped connection
(exit 255) offers Enter to reconnect, and with the OpenSSH Client feature
turned off the pane says how to turn it on. For T-013: the host picker
lists the hosts of `%USERPROFILE%\.ssh\config`, including ones from an
`Include` with backslashes in its path. For T-014 (threat model 0001,
TM-askpass-01 and -02): `ssh.exe` asks through the app (the host-key
dialog appears, with No focused); the pipe's DACL (`D:P(A;;GA;;;OW)`)
lets only the user open it; starting a second zed-term opens its own pipe;
a squatted pipe name fails the start (`FILE_FLAG_FIRST_PIPE_INSTANCE`)
and a helper pointed at a pipe of another process exits 1; the server's
nonblocking pipe reads notice a helper that ssh ended; a remembered
password logs in through Credential Manager (AC10); and a key passphrase
is filled in, which depends on how `ssh.exe` writes `~` and `\` into its
prompt. For T-020: a tab dropped on a side of a pane and a pane dragged by
its handle back to the tab bar both move, the handle shows on hover, and a
moved pane repaints cleanly after ConPTY's resize redraw. For T-002:
`Ctrl+Shift+C`/`V` copy and paste, a bracketed paste reaches pwsh as one
paste, the search bar finds text in the scrollback, a click on an
`https` link opens the browser (Ctrl+click with `open_links: ctrl_click`),
the wheel scrolls the scrollback and `less`, an IME (Japanese or Chinese)
composes at the cursor, and closing the window leaves no `pwsh` or
`conhost` behind. For T-006: a relaunch restores the tabs, splits and the
window size. For T-017: the settings screen changes the font size in
`%APPDATA%\zed-term\config.yaml` in place (CRLF kept), and recording a
key works with the Windows key mapper. Fix what fails. Area: src.

Spec: ../spec/0001-startup-performance.md (AC8)
Spec: ../spec/0002-terminal-pane.md (spawn, input, resize, exit on Windows)
Spec: ../spec/0003-tabs-splits-and-session-restore.md (keys, resize, close)
Spec: ../spec/0004-commands-keybindings-palette.md (palette and font keys)
Spec: ../spec/0005-config-and-themes.md (paths, watcher, PATHEXT)
Spec: ../spec/0006-ssh-hosts.md (ssh.exe, reconnect, missing client)
Spec: ../spec/0007-drag-and-drop-splits.md (drag and drop, ConPTY repaint)
