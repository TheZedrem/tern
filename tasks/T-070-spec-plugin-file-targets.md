---
status: later
blocked-by: [T-066]
---
# T-070: Write the spec for file-browser targets from plugins

Spec how a plugin supplies a target to the one file browser (S3, SMB;
architecture §Key decisions, "One file browser, many targets"): the
target's operations and capability flags as events and actions on spec
0016's protocol, how transfers stream, and which rights it takes. This is
the `display_browser` of the owner's idea (2026-11-11). Written with the
`write-spec` skill. Area: meta.

Spec: ../spec/0016-plugin-system.md
Spec: ../spec/0013-files-panel.md
