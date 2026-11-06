---
status: now
blocked-by: []
---
# T-023: SFTP sidebar, connection and listing

Step 1 of spec 0008. The sidebar (`sftp.toggle`, `Ctrl+Shift+S`;
`sftp_sidebar: left | right`, inside a tab list on the same side), open or
closed per SSH pane and shown only while that pane is focused, with its
width dragged on the inner edge. One session per SSH pane on its own
thread running `russh-sftp` over `ssh -s <host> sftp` on a current-thread
tokio runtime, prompts through askpass as in spec 0006, failures with
"Connect again". The flat listing with its always-editable path field,
starting in the pane's reported cwd, else home. Adds the new crates
(`russh-sftp`, `tokio` as a direct dependency) and `sftp_sidebar` on the
settings screen. Carries TM-sftp-01, -06, -14 to -17 and -19. Area: src.

Spec: ../spec/0008-sftp-panel.md (flows 1, 2, 7; AC1–3, 10's failure half)
