---
status: now
blocked-by: []
---
# T-036: SFTP edits in the default app

Step 3 of spec 0008. Edit copies under `<data_local_dir>/tern/edit/`
(`0700`/`0600`, `Zone.Identifier` on Windows), opened with the OS's
default app unless the type is one the OS runs; the 1 s poll, upload
modes (`sftp_upload: manual | save | focus` plus the per-edit override),
the pre-upload change check, the edit rows with state and last upload,
the close-with-edits dialog on pane, tab and window close, deleting
copies when a session ends and sweeping a crashed run's after the first
frame. `sftp_click` (default `edit`: until now a file's Enter or double
click downloads, T-035) and `sftp_upload` on the settings screen, and Edit
in an entry's context menu. Edits upload through T-035's part files, so
an upload that fails or is cancelled never leaves a half-written file on
the host. Carries
TM-sftp-05, -07 (edit copies), -08, -09, -12 and -13. Area: src.

Spec: ../../spec/0008-sftp-panel.md (flow 3; AC6, 7, 8, 10's close half)

## Result (2026-11-06)

Boot path: the window's should-close callback is registered with the
workspace, and the sweep is spawned after the first frame on the
background executor. Release `--bench-startup 10` at load average 2.7:
warm median 184.1 ms, cold 144.8 ms (budget 500 ms).

Built: `src/sftp/copies.rs` (the run's folder and lock, private folders,
`sweep` after the first frame of runs whose lock is free),
`src/sftp/view/edits.rs` (fetch to a private copy, open with the OS
unless `runs_when_opened`, a 1 s look at the copies off the UI thread,
`Edit::look` counting a save once it holds still, upload modes with a
per-edit switch, the pre-upload change check through `Upload::expect`
and the overwrite question, Upload / Open again / Close per edit, edits
kept across "Connect again"), `RemoteStat` on `TransferDone`, private
downloads in `transfer.rs`, `sftp_click` and `sftp_upload` in config, the
template and the settings screen (General → SFTP), and the workspace's
close question for pane, tab and window ("Upload and close" waits for the
uploads, then closes or says why not). The title bar's close button now
goes through it too.

Diverged from the spec, which now says so: the run folder and lock in
the copies' path, so the sweep never takes another open Tern's copies;
the look at the copies runs as a background task of the sidebar, not on
the connection's thread, so edits survive a reconnect; on Linux the
never-opened list also has Windows programs, which Wine would run.

Verified: 526 tests, 26 new ones by a separate agent against the spec
(the run check on both platforms, the sweep against held and free locks,
`Edit::look`, private copies against `sftp-server`, the conflict and its
confirmed overwrite, the two settings). No bugs found. A throwaway run
against `sftp-server` checked `0700`/`0600` and the conflict by hand.

Not verified: the window (the owner looks), Windows (T-011: PATHEXT,
`Zone.Identifier` on edit copies, a locked copy). The one test with a
private fetch leaves a free `<run>.lock` in the real data folder, which
the next launch sweeps.
