---
status: draft        # draft | approved | implemented
phase: 5
tasks: [T-064, T-080, T-069]
related: [spec 0003, spec 0004, spec 0006, spec 0008, spec 0009, spec 0013, spec 0016, spec 0017, threat model 0005, OQ-036]
---

# Spec: App sidebar

Pins the app sidebar: one sidebar per window that hosts panels, where it
sits, how panels are listed and switched, what is saved, and its first
built-in panel, the SSH hosts. Spec 0016 pins what a plugin's panel
contains and does; this spec pins the sidebar that draws it. The choices
the owner hasn't made yet are in OQ-036.

## Problem & goal

The SSH hosts are only reachable through a picker that closes after one
pick, and plugins (spec 0016) have panels with nowhere to be drawn. Both
need a place that stays open beside the terminals without getting in the
shell's way.

Done means: one key shows a sidebar listing the SSH hosts, a double click
opens one, plugins' panels appear in the same sidebar through the same
registration the hosts panel uses, and none of it costs boot time or takes
a keystroke meant for the shell.

## Scope

**In scope:**

- The app sidebar: placement, show and hide, width, the panel strip, the panel header
- What is saved of it, and what a relaunch brings back
- The panel extension point, used by built-in and plugin panels alike
- The SSH hosts panel: list, filter, open in a new tab
- How plugin panels are hosted: order, header, icon, the stopped state, the log, when `open_panel` is carried out
- Commands `sidebar.toggle` and `sidebar.focus`, and the setting `app_sidebar`

**Out of scope:**

- Panel items and their behavior (`set_panel`, command buttons, bounds): [spec 0016](0016-plugin-system.md)
- The pane sidebar (files and port forwards of one SSH pane): specs 0008, 0013 and 0017, unchanged. It doesn't become a panel here: it belongs to a pane, this one to the window
- Adding, editing or grouping hosts in the panel; a context menu (split with host, forget password): *Later*. The picker and the settings screen do these today
- Local profiles in the hosts panel: never; `Ctrl+Shift+T` lists them
- A palette picker for panels, reordering or hiding panels, collapsing host groups, dragging a host into a split: *Later*
- More icons than the embedded set: *Later* (each one adds to the binary)

## Design

### Affected areas

- New `src/sidebar/` (`mod.rs` the sidebar and its panel registry, `hosts.rs` the SSH hosts panel); new `src/layout/sidebar.rs` (the workspace's wiring: commands, placement, saving); `src/session.rs`; `src/commands/`; config, `src/settings/pages/appearance.rs`, `src/default-config.yaml`
- The per-pane `Sidebar` in `src/layout/files.rs` is renamed `PaneSidebar`, so the two can't be confused
- With T-080: `spec/architecture.md` §Code layout gains `sidebar/` and its session row in §Data stores gains the sidebar's state; spec 0008's placement line and spec 0004's key table gain this spec's

### Model

- **One per window, not per pane [load-bearing].** Open or closed, the
  width and the shown panel belong to the window. Switching tabs or panes
  changes nothing in it, and focus inside it doesn't change which pane is
  active (as spec 0008 says of the pane sidebar).
- **Placement [load-bearing].** On the `app_sidebar` side (`left`, the
  default, or `right`), as a column between the top and bottom edges of
  the pane area: a top or bottom tab bar, and the title bar where Tern
  draws one, span the whole window above or below it. From the window's
  edge inwards: the tab list (`tab_bar: left | right`), the app sidebar,
  the pane sidebar, the panes. A zoomed pane keeps the sidebar; the
  settings screen draws over it.
- **Show and hide.** `sidebar.toggle`, or a button beside the settings
  gear in the tab bar or tab list. Closed, nothing of it is drawn: no
  rail. Showing or hiding it resizes the panes like any divider drag.
- **Width.** 260 px until dragged by its inner edge, from 200 px up to
  half the window, clamped again when the window shrinks.
- **Panel [load-bearing].** The one extension point: an ID
  (`hosts`, or `plugin.<plugin>.<panel>`; an enum in code), a title, an
  icon, the owning plugin's name if any, and a view that draws the body.
  The sidebar draws the strip, the header and the edge, and knows nothing
  of hosts or plugins; the hosts panel registers itself exactly as a
  plugin panel does, with no code path of its own in the sidebar.
- **Order [load-bearing].** Built-in panels first, then plugin panels by
  plugin name (ignoring case), each plugin's in manifest order. A plugin
  can't choose its place.
- **Strip.** With two or more panels, a row of icon buttons at the top,
  wrapping when it's full, the shown panel's selected. A button's tooltip
  is the panel's title, for a plugin panel followed by `(Plugin <name>)`.
  With one panel there is no strip.
- **Header [load-bearing].** Under the strip: the panel's title and a
  close button. A plugin's panel always carries a second line,
  `Plugin <name>`, the name taken from the Host's record of the plugin
  (spec 0016), never from an action; a built-in panel never has that
  line. So a plugin named `SSH Hosts` reads `Plugin SSH Hosts`
  (TM-plug-01).
- **Icons.** One of the icons the embedded bundle holds (the widget
  library's icon set), named in kebab-case. Any other name, or none, gets
  the default plugin icon. No image is read from a plugin.
- **Body.** The panel's view gets the rest of the height and scrolls as
  one column. Text wraps at the sidebar's width and is never cut: a
  command button shows all of its command at 200 px too (TM-plug-23). A
  log takes the height its panel has left (at least 120 px), scrolls by
  itself, follows its newest line while scrolled to the end, and lays out
  only its visible rows. A stopped plugin's panels stay listed in place
  under the same header, with spec 0016's "stopped" and Restart as the
  body.
- **The keyboard stays with the shell [load-bearing].** Opening, closing,
  switching, a plugin filling a panel and `open_panel` never move
  keyboard focus. It enters the sidebar only by a click on something in
  a panel that takes keys, or by `sidebar.focus`. The sidebar's key
  context binds plain keys (arrows, Enter, Esc, Tab, F5), so they exist
  only while focus is inside it. Esc gives the keyboard back to the
  active pane and leaves the sidebar open; closing the sidebar or losing
  the focused panel does the same.
- **Never a prompt [load-bearing].** No confirmation bar and no password,
  passphrase or host-key question is ever drawn in the sidebar: bars sit
  on panes (spec 0016), ssh's prompts open over the pane (spec 0006,
  flow 4). Whatever a panel asks for is not Tern asking (TM-plug-02).
- **Saved.** `session.json` gains an optional `sidebar: { open, width,
  panel }`; a file without it means closed, 260 px, `hosts`, so the
  format's version stays. Saved by spec 0003's debounced save. With
  `restore_session: false` the sidebar starts closed.
- **SSH hosts panel.** The hosts of `ssh::hosts`, the picker's list
  (spec 0006, Model), under their groups as headings, each row its label
  and, dimmed, its destination. A filter field on top narrows by label,
  host and group with the picker's matcher. The list is read off the UI
  thread when the panel is shown, on F5 and on a config reload; nothing
  watches `~/.ssh/config`. Listing never runs `ssh -G` (TM-askpass-20).

### Module surface

- `sidebar::Sidebar` (a GPUI entity): `register(Panel)`, `remove(&PanelId)`, `show(&PanelId)`, `toggle()`, `focus()`, and its `open`, `width` and shown panel for the session. Emits `Shown(PanelId)` and `Hidden(PanelId)`; T-069 turns them into `panel_opened` and `panel_closed`
- `sidebar::Panel { id, title, icon, plugin, view }`, `sidebar::PanelId` (`Hosts | Plugin { plugin, panel }`)
- `sidebar::hosts::HostsPanel`: emits `Open(host id)`; the workspace opens it with the picker's own `open_profile_tab`
- `Workspace` hosts the sidebar through `layout/sidebar.rs`; it doesn't own panels (AGENTS.md, hazard 9)
- Commands, in the palette and bindable (spec 0004): `sidebar.toggle` ("Toggle sidebar", `Ctrl+Shift+L`, `Cmd+L` on macOS) and `sidebar.focus` ("Focus sidebar", no default key)
- Setting `app_sidebar: left | right`, on the settings screen's Appearance page beside `sftp_sidebar`, with its template line in `src/default-config.yaml` (spec 0009)

### Flows

1. **Toggle:** `sidebar.toggle` shows the sidebar on the panel it last
   showed, or hides it. The keyboard stays where it was; hiding with focus
   inside gives it to the active pane.
2. **Focus:** `sidebar.focus` opens the sidebar if it's closed and puts
   the keyboard in the shown panel (hosts: the filter field). Pressed
   with focus inside, it gives the keyboard back to the active pane.
   Tab and Shift+Tab walk the strip, the header and the panel; on the
   strip, Left and Right move and Enter shows a panel.
3. **Switch:** a click on a strip button shows that panel. Focus stays in
   the pane if it was there, and moves to the new panel if it was in the
   old one. `Hidden` and `Shown` are emitted, also when the sidebar closes
   and opens.
4. **Open a host:** Up and Down select, in the filter field too; Enter or
   a double click opens the host in a new tab named after it and focuses
   that tab, exactly as the picker's Enter does (spec 0006, flows 1, 2).
   A single click only selects: nothing connects by a stray click. No
   hosts: the picker's "No hosts yet" text. An unreadable
   `~/.ssh/config` or `Include` shows the picker's notice, once per read.
5. **A panel arrives or leaves:** plugin panels are registered when
   discovery (spec 0016, flow 1) finds an enabled, usable plugin and
   removed when it is disabled or becomes unusable. If the shown panel
   leaves, the first panel is shown.
6. **`open_panel` [load-bearing]:** it opens the sidebar if needed and
   shows the panel, without focus. Tern carries out one per act of the
   user's: only if the user ran one of the plugin's commands or acted in
   one of its panels since its last one. Otherwise, and while the
   keyboard is in another panel, it is dropped with a line in the
   plugin's log, so a plugin can't reopen what the user closed, pop up at
   launch, or pull a panel away under typing. After an `open_panel`
   changed what the sidebar shows, the sidebar ignores clicks for 0.5 s,
   as a command button does after `set_panel` (TM-plug-23).
7. **Relaunch:** see Startup impact. A saved `panel` that is a plugin's
   comes back as `hosts`: showing it would send `panel_opened` and start
   the plugin, and only a ticked `start` may start a plugin at launch
   (spec 0016).
8. **No room:** in a window too narrow for 200 px beside the panes'
   minimum (spec 0003), the sidebar isn't drawn; its flag stays, and it
   returns when the window grows.

### Startup impact

Nothing before the first frame. The session, which boot already reads,
says whether the sidebar was open and how wide. If it was, the first
frame lays the panes out with that width taken and draws the sidebar's
background empty, so nothing resizes a pane afterwards. The sidebar's
entity, its panels and the host list (a read of `~/.ssh/config`) are made
after the first frame, the list off the UI thread. No plugin starts
because of it (flow 7). T-080 reports `--bench-startup 10` with the
sidebar saved open against closed.

### Platform notes

- Windows 11: ConPTY redraws on every resize, so showing or hiding the
  sidebar repaints each visible shell, as the pane sidebar does; check
  that no rows go stale. `~/.ssh/config` is `%USERPROFILE%\.ssh\config`.
- Linux (Wayland): where Tern draws the title bar (GNOME), it stays on
  top across the window, above the sidebar.
- macOS: compiles; `Cmd+L` by spec 0004's swap.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| The sidebar's open flag, width and shown panel ID | `session.json` (spec 0003) | Bring it back | The user's own device; no one else processes it | Overwritten on save |
| The host list | Memory only, read from `config.yaml` and `~/.ssh/config` | Show the hosts | Same | Until the next read |

No host name, panel content or log reaches `session.json`. Nothing is
logged. Showing the hosts panel opens no connection and runs no program;
only Enter or a double click on a host starts `ssh`.

Trust boundaries: none new. The sidebar draws what crosses spec 0016's
Tern ↔ plugin boundary, and keeps
[threat model 0005](threat-model/0005-plugins.md)'s mitigations:

- TM-plug-01: the header's `Plugin <name>` line, built-in panels first,
  icons only from the embedded set, titles by spec 0003's title rules.
- TM-plug-02: no focus by `open_panel`, never a prompt in the sidebar.
- TM-plug-23: commands never cut by the width, and the 0.5 s after
  `open_panel`.
- Host labels come from the user's own files and render as plain text by
  the same title rules.

## Acceptance criteria

1. With a shell focused, `Ctrl+Shift+L` shows the sidebar on SSH hosts and `ls` + Enter typed right after runs in the shell; `Ctrl+Shift+L` again hides it. `Ctrl+R` with the sidebar open reaches the shell. Each toggle changes the shell's column count (`tput cols`; Linux and Windows) and leaves no stale rows.
2. With `tab_bar: left`, `sftp_sidebar: left` and an SSH pane's sidebar open, the order from the left is tab list, app sidebar, pane sidebar, panes; with `tab_bar: top` the tab bar spans above the sidebar. Changing `app_sidebar` to `right` on the settings screen moves it to the right edge without a restart, and the key is in `src/default-config.yaml`.
3. The hosts panel lists the same hosts under the same groups as `Ctrl+Shift+H`; typing part of a group name in the filter narrows it to that group; a host added to `~/.ssh/config` shows after F5. While the panel is shown and hosts are clicked once, no `ssh` process exists.
4. A double click, or Enter, on a host opens a connected tab named after it with the keyboard in it, and a remembered password logs in as from the picker (Linux and Windows).
5. `sidebar.focus` puts typing into the filter field and none of it reaches the shell; Esc returns typing to the shell with the sidebar still open.
6. Quitting with the sidebar open at 320 px and relaunching shows it at 320 px on the first frame, `~/.ssh/config` is not read before the first-frame mark, and `--bench-startup 10` is within noise of a run with it closed. `session.json` holds the flag, the width and a panel ID and no host name. With `restore_session: false` it starts closed.
7. A panel registered in a test through the same call the hosts panel uses appears in the strip after it and shows its view; removing it while shown shows SSH hosts. `src/sidebar/mod.rs` names neither hosts nor plugins.
8. With plugins `alpha` and `Zeta` enabled (T-069), the strip reads hosts, alpha's panels, Zeta's in manifest order; a plugin panel's header reads `Plugin <name>`, one named `SSH Hosts` reads `Plugin SSH Hosts`, and `icon: no-such-icon` shows the default icon. A killed plugin's panel keeps its place and header and shows "stopped" with Restart.
9. After running a plugin's command with the sidebar closed, its `open_panel` shows its panel and the next keystrokes still reach the shell; a click on a command button within 0.5 s runs nothing. After the user closes the sidebar, a second `open_panel` with no act of the user's shows nothing and adds a line to the plugin's log. Quitting on a plugin's panel and relaunching shows SSH hosts and starts no plugin process.
10. At 200 px a command button's whole command is readable, wrapped; a panel log receiving 10,000 lines stays on its newest line, scrolls smoothly, and typing in a pane stays responsive.

## Open questions

- OQ-036: the choices made here on the owner's behalf: the strip instead of an always-visible rail, the state in `session.json`, a plugin's panel not restored, one `open_panel` per act of the user's, a double click to connect, and the keys. Each has its conservative choice written in above.
