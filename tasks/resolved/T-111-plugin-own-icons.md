---
status: now
blocked-by: []
---
# T-111: Let a plugin bring its own panel icon

A panel's `icon` in `plugin.yaml` may name an SVG file in the plugin's
folder (`icon: docker.svg`). Tern checks it at discovery (plain shapes
only, at most 16 KiB) and draws its shape in the strip in the colour of
its own icons; a file that fails the check lists the plugin as unusable
with the reason. The Docker plugin gets its own icon — src.
Spec: ../spec/0018-app-sidebar.md (Design, Icons; TM-plug-24)
