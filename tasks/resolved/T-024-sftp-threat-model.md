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

## Result (2026-11-06)

[Threat model 0003](../../spec/threat-model/0003-sftp.md), written
together with spec 0008's switch from a split pane to a sidebar that
each SSH pane opens for itself and that shows only while that pane is
focused (owner, 2026-11-06). Folded into spec 0008: the per-pane sidebar
and the host named in the header and in every confirmation (TM-sftp-01),
Windows name rules and `Zone.Identifier` (-04, -07), a fixed list of types
never opened (-05), the pre-upload change check (-09), `0600` copies
(-13), stderr shown capped and stripped (-16), the 50,000-entry and 30 s
limits (-17). Residual risks await the owner in OQ-023.
