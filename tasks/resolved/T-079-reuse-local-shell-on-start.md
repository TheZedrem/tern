---
status: next
blocked-by: []
---
# T-079: Focus a restored local shell instead of opening a fresh tab

With `open_new_tab: true`, launching Tern appends a fresh tab even when
the restored session already has a plain local shell, so tabs pile up
launch after launch. Focus that tab instead: one leaf running
`default_profile`, not split, not SSH, not a files pane; the rightmost
of several. It keeps its cwd and title and is the one shell spawned at
boot. Update the `open_new_tab` line in `src/default-config.yaml`,
spec 0005's config listing and the settings screen's description to
say so. Boot path: report TTFP (one spawn either way). Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md (flow 6 Reuse, AC 13)
