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

Spec: ../spec/0008-sftp-panel.md (flows 4–6; AC4, 5, 8's download half, 9)
