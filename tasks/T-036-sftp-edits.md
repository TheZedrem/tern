---
status: next
blocked-by: [T-035]
---
# T-036: SFTP edits in the default app

Step 3 of spec 0008. Edit copies under `<data_local_dir>/tern/edit/`
(`0700`/`0600`, `Zone.Identifier` on Windows), opened with the OS's
default app unless the type is one the OS runs; the 1 s poll, upload
modes (`sftp_upload: manual | save | focus` plus the per-edit override),
the pre-upload change check, the edit rows with state and last upload,
the close-with-edits dialog on pane, tab and window close, deleting
copies when a session ends and sweeping a crashed run's after the first
frame. `sftp_click` and `sftp_upload` on the settings screen. Carries
TM-sftp-05, -07 (edit copies), -08, -09, -12 and -13. Area: src.

Spec: ../spec/0008-sftp-panel.md (flow 3; AC6, 7, 8, 10's close half)
