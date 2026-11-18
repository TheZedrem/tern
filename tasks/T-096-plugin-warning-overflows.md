---
status: next
blocked-by: []
type: bug
---
# T-096: The plugin warning runs off the settings page instead of wrapping

On the settings screen's Plugins page, the trust warning above "Open
plugins folder" ("A plugin is a program that runs as you, …") renders
as one line and is cut off at the page's right edge, so the part that
matters ("install plugins only from a source you trust") isn't visible.
Seen on Windows 11 (owner's screenshot, 2026-11-17). It should wrap to
the page width.

The intro is `div().text_sm().child(WARNING)` in `plugin_rows`
(`src/settings/pages/plugins.rs`), inside a flex column with no width
bound, so the text takes its unwrapped width. The enable dialog shows
the same `WARNING` and the command line the same way; check it wraps
too, and check other settings pages for intro or help text built the
same way. Area: src.

Spec: ../spec/0016-plugin-system.md
