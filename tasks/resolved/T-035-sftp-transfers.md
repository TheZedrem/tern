---
status: now
blocked-by: []
---
# T-035: SFTP transfers and file changes

Step 2 of spec 0008. Download (to Downloads, Download to…), upload (picked
files and drops from the file manager onto the folder shown or a folder
row), both in 256 KiB chunks with progress and cancel, overwrite
confirmations naming the host; rename, delete and new folder; the remote
name escaping and, on Windows, `Zone.Identifier` on downloads;
`sftp_click: download`. Carries TM-sftp-03, -04, -07 (downloads), -08,
-10, -11 and -18. Area: src.

Spec: ../../spec/0008-sftp-panel.md (flows 4–6; AC4, 5, 8's download half, 9)

## Result (2026-11-06)

Boot path: three more key bindings in the sidebar's context (Delete, F2,
Escape in the name field); nothing else. Release `--bench-startup 10`
ran while the machine was at load average 38 from other work: warm median
377.8 ms, then 302.6 ms (budget 500 ms), against 155.8 ms for T-023 on a
quiet machine. Worth re-measuring when it's idle.

Built: `src/sftp/transfer.rs` (downloads and uploads as tasks on the
connection's thread, 64 KiB requests with 16 in flight, written to a
`.tern-part` file and renamed into place; a replaced remote file lends its
permissions; cancel and failures remove the part; `Zone.Identifier` on
Windows), new requests and events in `client.rs` (cancel, rename, delete,
new folder, `Exists`, `Changed`), `local_name`/`local_path` in `mod.rs`
(the escaping of spec 0008 §Security), and in the sidebar: an entry
context menu (Open or Download / Download to…, Rename, Delete), Upload
and New folder buttons, drops on the listing or a folder row, the
replace/delete questions naming the host, the name field (Enter, Esc,
F2), Delete key, and the transfer list with progress, cancel and failures.
Existence is checked on the connection's thread, never the UI's.

Diverged from the spec, which now says so: 64 KiB requests instead of
256 KiB chunks (OpenSSH answers at most 255 KiB, and a short answer stalls
the pipeline); the part files; delete removes empty folders only; a
dropped folder fails in the transfer list rather than as a notice.
`sftp_click` moved to T-036: until edits exist, a file's Enter or double
click downloads.

Verified: 500 tests, 25 new ones by a separate agent against the spec,
run against OpenSSH's `sftp-server`: byte-identical downloads and uploads,
progress, `Exists` both ways, replace keeping 0755, cancel removing parts
locally and remotely, a folder upload refused, rename/new folder/delete,
a non-empty folder kept, a link to a folder removed as a link, and every
hostile name landing inside its folder. They found a Windows-only panic in
the device-name check (a stem ending in a non-ASCII letter) and a failed
download reporting before its part was removed; both fixed.

Not verified: the window (the owner looks), file dialogs on COSMIC (the
portal's KDE drop-in), Windows (T-011). A pane closed mid-upload can't
remove its remote `.tern-part`: its connection is gone.
