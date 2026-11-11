---
status: later
blocked-by: [T-066]
---
# T-067: Plugins tab on the settings screen

A Plugins tab listing every found plugin, under the warning to install
plugins only from a trusted source (OQ-031): enable it after a
confirmation that repeats the warning, disable it, tick its
rights and events, see its command line and the note that it runs as the user, change
its settings (manifest fields and the ones it sends with `set_settings`),
restart it, and read its log. Unusable plugins show their reason
(spec 0016, flows 2 and 8). Area: src.

Spec: ../spec/0016-plugin-system.md
Spec: ../spec/0009-settings-screen.md
