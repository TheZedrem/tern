---
status: next
blocked-by: []
---
# T-062: Open a fresh tab on every launch

Launching Tern lands in a new tab running `default_profile` in the home
directory, after the restored tabs, instead of the last active one. The
restored tabs stay unspawned until first activated. `open_new_tab: false`
turns it off; the key goes on the settings screen's Behavior section and
into `src/default-config.yaml`. Asked for by the owner (2026-11-10).
Settle OQ-030 before building. Boot path: report TTFP. Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
