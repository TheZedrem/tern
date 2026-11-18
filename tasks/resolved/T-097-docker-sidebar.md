---
status: resolved
blocked-by: []
---
# T-097: Show Docker containers and Compose projects for the active pane

Ship the optional sidebar plugin and the opt-in panel target event.
Check local Windows Docker Desktop, local Linux Engine and SSH to a Linux
Engine, including unavailable engines, refresh and pane changes.

Spec: ../spec/0016-plugin-system.md and ../spec/0018-app-sidebar.md

Verified on Windows 11 and WSL Linux. A native Windows test instance
rendered seven inventory rows from Docker Desktop. A loopback SSH
fixture authenticated with a temporary key on a non-default port and
ran the inventory commands against Docker in WSL. Workspace regressions
cover switching to an SSH pane, rejecting stale revisions, revoking
the grant and leaving ordinary plugin panels alone.
