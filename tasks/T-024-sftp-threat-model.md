---
status: later
blocked-by: []
---
# T-024: Threat model: SFTP panel

STRIDE model of spec 0008 before T-023 builds it: remote names and path
traversal on download, overwrites, remote symlinks, oversized listings
and replies from a hostile server, the panel's own ssh process and
askpass token, and files that are private user content. Fold the
mitigations into spec 0008; residual risks go to `spec/open-questions.md`
for the owner's sign-off. After the owner approves spec 0008. Area: meta.

Spec: ../spec/0008-sftp-panel.md
