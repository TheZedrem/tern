---
status: later
blocked-by: []
---
# T-024: Threat model: SFTP panel

STRIDE model of spec 0008 before T-023 builds it: remote names and path
traversal on download and edit copies, overwrites, opening edit copies
with the OS's default app (types the OS would run), an edit's upload
(by click, on save or on focus) replacing a remote file, dropped local files, remote symlinks, oversized listings and replies from a hostile server, the panel's own ssh process and
askpass token, and files that are private user content. Fold the
mitigations into spec 0008; residual risks go to `spec/open-questions.md`
for the owner's sign-off. After the owner approves spec 0008. Area: meta.

Spec: ../spec/0008-sftp-panel.md
