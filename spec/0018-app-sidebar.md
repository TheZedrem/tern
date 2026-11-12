---
status: draft        # draft | approved | implemented
phase: 5
tasks: [T-064, T-080, T-069]
related: [spec 0003, spec 0004, spec 0006, spec 0008, spec 0009, spec 0013, spec 0016, spec 0017, threat model 0003, threat model 0005, OQ-036, OQ-038]
---

# Spec: App sidebar

Pins the sidebar: one per window that hosts panels, where it sits, how
panels are listed and switched, what is saved, and its two built-in
panels, Files and SSH hosts. It replaces the per-pane sidebar of spec
0008: the file browser and the port forwards become its Files panel, and
it stays open when the user switches tabs. Spec 0016 pins what a plugin's
panel contains and does; this spec pins the sidebar that draws it. The
choices the owner hasn't made yet are in OQ-036 and OQ-038.

## Problem & goal

The file sidebar belongs to one SSH pane, so it disappears on every
switch to another tab. The SSH hosts are only reachable through a picker
that closes after one pick, and plugins (spec 0016) have panels with
nowhere to be drawn. All three need one place that stays open beside the
terminals without getting in the shell's way.

Done means: `Ctrl+Shift+S` shows a sidebar that stays open across tabs
and comes back on the panel it last showed, Files the first time. Files
shows the active pane's files and forwards, SSH hosts opens a host on a
double click, plugins' panels appear through the same registration the
built-in panels use, and none of it costs boot time or takes a keystroke
meant for the shell.

## Scope

**In scope:**

- The sidebar: placement, show and hide, width, the panel strip, the panel header
- What is saved of it, and what a relaunch brings back
- The panel extension point, used by built-in and plugin panels alike
- The Files panel: the file browser and port forwards of the active pane, moved here from the pane's own sidebar
- The SSH hosts panel: list, filter, open in a new tab
- How plugin panels are hosted: order, header, icon, the stopped state, the log, when `open_panel` is carried out
- Commands `sidebar.toggle` (which takes over `Ctrl+Shift+S` from `sftp.toggle`) and `sidebar.focus`, and the setting `app_sidebar`, which `sftp_sidebar` becomes

**Out of scope:**

- Panel items and their behavior (`set_panel`, command buttons, bounds): [spec 0016](0016-plugin-system.md)
- What the file browser and the port forwards show and do: specs 0008, 0013 and 0017. Only where they are drawn and what opens them changes
- Port forwards as a panel of its own instead of the switch inside Files: *Later*
- Adding, editing or grouping hosts in the panel; a context menu (split with host, forget password): *Later*. The picker and the settings screen do these today
- Local profiles in the hosts panel: never; `Ctrl+Shift+T` lists them
- A palette picker for panels, reordering or hiding panels, collapsing host groups, dragging a host into a split: *Later*
- More icons than the embedded set: *Later* (each one adds to the binary)

## Design

### Affected areas

- New `src/sidebar/` (`mod.rs` the sidebar and its panel registry, `hosts.rs` the SSH hosts panel, `files.rs` the Files panel); new `src/layout/sidebar.rs` (the workspace's wiring: commands, placement, saving), which replaces the per-pane sidebar host in `src/layout/files.rs`; `src/session.rs`; `src/commands/`; config, `src/settings/pages/appearance.rs`, `src/default-config.yaml`
- With T-080: `spec/architecture.md` §Code layout gains `sidebar/` and its session row in §Data stores gains the sidebar's state
- With T-080, the specs this one overrides are amended: spec 0008 (`sftp_sidebar` is now `app_sidebar`; the sidebar is the window's, not a flag of each pane; `sftp.toggle` is an old name for `sidebar.toggle`; the width is saved; flow 1 and AC3), spec 0017 (Esc no longer closes the sidebar), spec 0004's key table, and threat model 0003's TM-sftp-15 wording

### Model

- **One per window, not per pane [load-bearing].** Open or closed, the
  width and the shown panel belong to the window. Switching tabs or panes
  never opens, closes or switches it, and focus inside it doesn't change
  which pane is active. There is no second, per-pane sidebar.
- **Placement [load-bearing].** On the `app_sidebar` side (`right`, the
  default, or `left`; the new name of spec 0008's `sftp_sidebar`), as a
  column
  between the top and bottom edges of the pane area: a top or bottom tab
  bar, and the title bar where Tern draws one, span the whole window
  above or below it. From the window's edge inwards: the tab list
  (`tab_bar: left | right`), the sidebar, the panes. A zoomed pane keeps
  the sidebar; the settings screen draws over it.
- **Show and hide.** `sidebar.toggle`, or a button beside the settings
  gear in the tab bar or tab list. Closed, nothing of it is drawn: no
  rail. Showing or hiding it resizes the panes like any divider drag.
- **Width.** 260 px until dragged by its inner edge, from 200 px up to
  half the window, clamped again when the window shrinks.
- **Panel [load-bearing].** The one extension point: an ID
  (`files`, `hosts`, or `plugin.<plugin>.<panel>`; an enum in code), a title, an
  icon, the owning plugin's name if any, and a view that draws the body.
  The sidebar draws the strip, the header and the edge, and knows nothing
  of files, hosts or plugins; the built-in panels register themselves
  exactly as a plugin panel does, with no code path of their own in the
  sidebar.
- **Order [load-bearing].** Files, SSH hosts, then plugin panels by
  plugin name (ignoring case), each plugin's in manifest order. A plugin
  can't choose its place.
- **Strip.** A row of icon buttons at the top,
  wrapping when it's full, the shown panel's selected. A button's tooltip
  is the panel's title, for a plugin panel followed by `(Plugin <name>)`.
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
- **Saved [load-bearing].** `session.json` gains an optional `sidebar:
  { open, width, panel }`; a file without it means closed, 260 px,
  `files`, so the format's version stays. The panel shown last is the one
  the sidebar opens on next, in this run and the next; Files is where it
  starts. Saved by spec 0003's debounced save. With `restore_session:
  false` the sidebar starts closed, on Files.
- **Files panel [load-bearing].** Draws what the pane's sidebar drew
  (spec 0008; the Files and Port forwards switch of spec 0017), always
  for the window's *active pane*, never a stale one, its header naming
  `user@host` (TM-sftp-01, -15, TM-fwd-07). Sessions stay the panes':
  switching back to a pane shows its folder, transfers and edits where
  they were. A pane's SFTP session is created by an act of the user's on
  the sidebar (opening it on Files, or switching to Files) while that
  pane is active, or by the panel's "Show files" button. Switching to an
  SSH pane that has no session, and a relaunch, connect nothing: the
  panel shows `user@host` and that button (OQ-038). On a local pane it
  says there are no files to show yet; T-049 puts the Local target there.
- **SSH hosts panel.** The hosts of `ssh::hosts`, the picker's list
  (spec 0006, Model), under their groups as headings, each row its label
  and, dimmed, its destination. A filter field on top narrows by label,
  host and group with the picker's matcher. The list is read off the UI
  thread when the panel is shown, on F5 and on a config reload; nothing
  watches `~/.ssh/config`. Listing never runs `ssh -G` (TM-askpass-20).

### Module surface

- `sidebar::Sidebar` (a GPUI entity): `register(Panel)`, `remove(&PanelId)`, `show(&PanelId)`, `toggle()`, `focus()`, and its `open`, `width` and shown panel for the session. Emits `Shown(PanelId)` and `Hidden(PanelId)`; T-069 turns them into `panel_opened` and `panel_closed`
- `sidebar::Panel { id, title, icon, plugin, view }`, `sidebar::PanelId` (`Files | Hosts | Plugin { plugin, panel }`)
- `sidebar::files::FilesPanel`: draws the active pane's browser and forwards, which stay in `src/files/` and `src/forwards/`
- `sidebar::hosts::HostsPanel`: emits `Open(host id)`; the workspace opens it with the picker's own `open_profile_tab`
- `Workspace` hosts the sidebar through `layout/sidebar.rs`; it doesn't own panels (AGENTS.md, hazard 9)
- Commands, in the palette and bindable (spec 0004): `sidebar.toggle` ("Toggle sidebar", `Ctrl+Shift+S`, `Cmd+S` on macOS) and `sidebar.focus` ("Focus sidebar", no default key). `sftp.toggle` leaves the palette but stays an accepted name for `sidebar.toggle` in `keys:`, so existing configs keep working (T-049). `pane.portForwards` (spec 0017) opens the sidebar on Files with Port forwards shown
- Setting `app_sidebar: left | right` (default `right`), on the settings screen's Appearance page where `sftp_sidebar` was, with its template line in `src/default-config.yaml` in place of the old one (spec 0009)
- **Migration [load-bearing].** `sftp_sidebar` is read as the old name of `app_sidebar`, with no notice; if a file has both, `app_sidebar` wins. Tern never rewrites the file for this by itself: the next time the settings screen saves the side, it writes `app_sidebar` and removes the `sftp_sidebar` line. So a config with `sftp_sidebar: left` keeps its sidebar on the left

### Flows

1. **Toggle:** `sidebar.toggle` shows the sidebar on the panel it last
   showed (Files the first time), or hides it, whatever panel it shows.
   The keyboard stays where it was; hiding with focus inside gives it to
   the active pane.
2. **Focus:** `sidebar.focus` opens the sidebar if it's closed and puts
   the keyboard in the shown panel (Files: the listing; hosts: the filter
   field). Pressed
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
   leaves, Files is shown.
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
   comes back as `files`: showing it would send `panel_opened` and start
   the plugin, and only a ticked `start` may start a plugin at launch
   (spec 0016). Files connects nothing at launch (Model).
8. **Switching panes:** with Files shown, focusing another pane or tab
   redraws the panel for that pane in the same frame: its session if it
   has one, the "Show files" button if it is an SSH pane without one, the
   local text otherwise. The sidebar stays open and on Files; other
   panels don't change at all. A pane's prompts still open that pane's
   dialog (spec 0008, flow 1).
9. **No room:** in a window too narrow for 200 px beside the panes'
   minimum (spec 0003), the sidebar isn't drawn; its flag stays, and it
   returns when the window grows.

### Startup impact

Nothing before the first frame. The session, which boot already reads,
says whether the sidebar was open and how wide. If it was, the first
frame lays the panes out with that width taken and draws the sidebar's
background empty, so nothing resizes a pane afterwards. The sidebar's
entity, its panels and the host list (a read of `~/.ssh/config`, only if
SSH hosts is the shown panel) are made after the first frame, the list
off the UI thread. No plugin starts and no connection is made because of
it (flow 7). T-080 reports `--bench-startup 10` with the
sidebar saved open against closed.

### Platform notes

- Windows 11: ConPTY redraws on every resize, so showing or hiding the
  sidebar repaints each visible shell, as today; check that no rows go
  stale. `~/.ssh/config` is `%USERPROFILE%\.ssh\config`.
- Linux (Wayland): where Tern draws the title bar (GNOME), it stays on
  top across the window, above the sidebar.
- macOS: compiles; `Cmd+S` by spec 0004's swap.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| The sidebar's open flag, width and shown panel ID | `session.json` (spec 0003) | Bring it back | The user's own device; no one else processes it | Overwritten on save |
| The host list | Memory only, read from `config.yaml` and `~/.ssh/config` | Show the hosts | Same | Until the next read |

No host name, panel content or log reaches `session.json`. Nothing is
logged. Showing the hosts panel opens no connection and runs no program;
only Enter or a double click on a host starts `ssh`. The Files panel
stores what spec 0008 already stores and connects only as the Model
says: never by a pane switch, never at launch.

Trust boundaries: none new. The sidebar draws what crosses spec 0016's
Tern ↔ plugin boundary, and keeps
[threat model 0005](threat-model/0005-plugins.md)'s mitigations:

- TM-plug-01: the header's `Plugin <name>` line, built-in panels first,
  icons only from the embedded set, titles by spec 0003's title rules.
- TM-plug-02: no focus by `open_panel`, never a prompt in the sidebar.
- TM-plug-23: commands never cut by the width, and the 0.5 s after
  `open_panel`.
- TM-sftp-15, TM-fwd-07 (threat models 0003, 0004): Files draws only the
  active pane's own session and forwards, and names `user@host`.
- Host labels come from the user's own files and render as plain text by
  the same title rules.

## Acceptance criteria

1. With a shell focused, `Ctrl+Shift+S` shows the sidebar on Files the first time and `ls` + Enter typed right after runs in the shell; `Ctrl+Shift+S` again hides it. `Ctrl+R` with the sidebar open reaches the shell. Each toggle changes the shell's column count (`tput cols`; Linux and Windows) and leaves no stale rows.
2. With the sidebar open on SSH pane A's files, switching to a local tab and back keeps it open the whole time and shows A's folder and running transfers where they were, without reconnecting. Switching to SSH pane B, which has no session, shows B's `user@host` with "Show files" and starts no `ssh` process until it is clicked; A's listing is never drawn while B is active.
3. Showing SSH hosts, hiding the sidebar and showing it again shows SSH hosts; so does quitting and relaunching. With no `session.json` it opens on Files.
4. With `tab_bar: left` and `app_sidebar: left`, the order from the left is tab list, sidebar, panes; with `tab_bar: top` the tab bar spans above the sidebar. A config with only `sftp_sidebar: left` gives the same; changing the side on the settings screen moves an open sidebar without a restart and leaves `app_sidebar` and no `sftp_sidebar` in the file. The template in `src/default-config.yaml` has `app_sidebar` only. `sftp.toggle` is not in the palette, a `keys:` entry binding it toggles the sidebar, and `pane.portForwards` opens the sidebar on Files with Port forwards shown.
5. The hosts panel lists the same hosts under the same groups as `Ctrl+Shift+H`; typing part of a group name in the filter narrows it to that group; a host added to `~/.ssh/config` shows after F5. A double click, or Enter, on a host opens a connected tab named after it with the keyboard in it (Linux and Windows); single clicks start no `ssh` process.
6. `sidebar.focus` on SSH hosts puts typing into the filter field and none of it reaches the shell; Esc returns typing to the shell with the sidebar still open, in the Files panel and its Port forwards view too.
7. Quitting with the sidebar open at 320 px on an SSH pane's files and relaunching shows it at 320 px on the first frame and makes no connection; `~/.ssh/config` is not read before the first-frame mark, and `--bench-startup 10` is within noise of a run with it closed. `session.json` holds the flag, the width and a panel ID and no host name. With `restore_session: false` it starts closed.
8. A panel registered in a test through the same call the built-in panels use appears in the strip after them and shows its view; removing it while shown shows Files. `src/sidebar/mod.rs` names neither files, hosts nor plugins. Spec 0008's and spec 0017's acceptance criteria still pass, apart from the lines this spec overrides.
9. With plugins `alpha` and `Zeta` enabled (T-069), the strip reads Files, SSH hosts, alpha's panels, Zeta's in manifest order; a plugin panel's header reads `Plugin <name>`, one named `SSH Hosts` reads `Plugin SSH Hosts`, and `icon: no-such-icon` shows the default icon. A killed plugin's panel keeps its place and header and shows "stopped" with Restart.
10. After running a plugin's command with the sidebar closed, its `open_panel` shows its panel and the next keystrokes still reach the shell; a click on a command button within 0.5 s runs nothing. After the user closes the sidebar, a second `open_panel` with no act of the user's shows nothing and adds a line to the plugin's log. Quitting on a plugin's panel and relaunching shows Files and starts no plugin process. At 200 px a command button's whole command is readable, wrapped.

## Open questions

- OQ-036: the choices made here on the owner's behalf: the strip instead of an always-visible rail, the state in `session.json`, a plugin's panel not restored, one `open_panel` per act of the user's, and a double click to connect. Each has its conservative choice written in above.
- OQ-038: the choices that came with moving the file browser in: no connection by a pane switch, the `right` default kept, Port forwards kept as a switch inside Files, and Esc no longer closing the sidebar.
