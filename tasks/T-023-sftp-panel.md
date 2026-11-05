---
status: later
blocked-by: [T-024]
---
# T-023: SFTP panel

Build spec 0008 once the owner approves it: the SFTP panel as a pane kind
of the split tree, its client thread running `russh-sftp` over
`ssh -s <host> sftp` on a current-thread tokio runtime, listing and
browsing, downloads and uploads with progress and cancel, rename, delete
and new folder, and prompts through askpass as in spec 0006. The new
crates (`russh-sftp`, `tokio` as a direct dependency) come with the
spec's approval. Area: src.

Spec: ../spec/0008-sftp-panel.md
