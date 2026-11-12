---
status: approved     # draft | approved | implemented
phase: 5
tasks: [T-064, T-065, T-066, T-067, T-068, T-069, T-070, T-071]
related: [spec 0001, spec 0004, spec 0005, spec 0009, spec 0013, spec 0018, threat model 0005, OQ-031 ✅, OQ-032 ✅, OQ-035, OQ-037]
---

# Spec: Plugin system

Pins how a plugin runs, how Tern and a plugin talk, and what a plugin may
do: plugins are standalone programs that receive events as JSON and answer
with actions Tern carries out, the risky ones only after the user confirms.
It settles the runtime question in architecture §Roadmap (the owner's idea,
2026-11-11) and resolves OQ-031 and OQ-032.

## Problem & goal

Docker, S3 and an AI assistant are too niche to build in, and building them
in would cost everyone boot time. Users need a way to add them, and authors
need a way to write them in any language, including a shell script.

Done means: a program in a folder with a manifest can add palette commands
and sidebar panels, stream a log into its panel, and ask to type into or
read from a terminal, with nothing running before the user enabled it and
nothing touching a terminal before the user saw and approved it.

## Scope

**In scope:**

- Plugin folders, the manifest, enabling and rights per plugin
- The runtime: one process per plugin, started by its first event, JSON lines over stdio
- Events and actions, version 1 of the protocol (`api: 1`)
- The user's approval of everything that writes to or reads from a terminal: a confirmation per action, or a click on a button that shows its command
- A Plugins tab on the settings screen: enable, rights, the plugin's own settings, its log
- Panel contents: what a plugin can put into a sidebar panel

**Out of scope:**

- Where panels are drawn, and the SSH hosts as the first built-in panel: the app sidebar's own spec ([spec 0018](0018-app-sidebar.md)); panels wait for it
- File-browser targets from plugins (S3, SMB; the idea's `display_browser`): its own Phase 5 spec (T-070), because a target is a set of operations, not one action
- Theme plugins: data read by the boot path, not programs (architecture §Roadmap); their own Phase 5 spec
- `request_portforward`: with port forwarding, Phase 7
- Installing, updating and registries: Phase 6. Here, installing is copying a folder
- Settings of type secret, kept in the keychain: *Later*. Until then a plugin's settings hold no secrets, since they are stored in `config.yaml` (ground rule 3)
- "Always allow" for a confirmed action: never in `api: 1`
- Plugins drawing their own UI, or code loaded into Tern's process: never

## Design

### Affected areas

- `src/plugins/` (new: manifest, host, protocol, confirmations, panel model), `src/commands/` (plugin commands in the palette and keymap), `src/settings/pages/` (Plugins tab), config schema (`plugins:`), `src/default-config.yaml`

### Model

- **A plugin is a folder**: `<data_local_dir>/tern/plugins/<id>/` holding
  `plugin.yaml` and its program. Not the config dir: that one roams on
  Windows, and a program is per machine.
- **The manifest declares everything a plugin can ever do** (sketch; code
  owns the full schema):

  ```yaml
  api: 1
  id: docker                      # equals the folder name
  name: Docker
  version: 0.1.0
  run: [./tern-docker]            # argv; first entry relative to the folder or on PATH
  rights: [pane_info, terminal_write]
  events: [pane_opened, pane_closed]
  commands: [{ id: containers, title: "Docker: show containers" }]
  panels: [{ id: containers, title: Containers, icon: container }]
  settings: [{ key: socket, label: Docker socket, type: text }]
  ```

  A manifest over 64 KiB is unusable without being parsed. The IDs of a
  plugin, its commands, panels and settings are 1 to 64 of `a-z`, `0-9`,
  `-` and `_`. `run` starts with a program's name (looked up on `PATH`),
  an absolute path, or `./` and a path that stays inside the folder. Its `name` must
  differ, ignoring case, from every other plugin's and from `Tern`, or both
  plugins are listed as unusable. Its strings render by spec 0003's title
  rules, capped at 64 characters, and `icon` names one of Tern's built-in
  icons, any other value getting the default (TM-plug-01, -13).

- **Never through a shell [load-bearing].** `run` is spawned as an argv
  with the plugin's folder as its working directory. No `sh -c`, no
  `cmd /c`, and nothing from an event or action is ever put on a command
  line.
- **Disabled until enabled [load-bearing].** A found plugin is listed and
  not started. Enabling it, each right and each event are the user's
  choices on the settings screen, stored in `config.yaml`:

  ```yaml
  plugins:
    docker:
      enabled: true
      run: [./tern-docker]          # the command line the user confirmed
      rights: [pane_info, terminal_write]
      events: [pane_opened, pane_closed]
      settings: { socket: /var/run/docker.sock }
  ```

  The rights and events in force are the granted ones that the manifest
  still declares. One a new version adds stays off until the user ticks
  it, and an event that isn't ticked is never sent. `run` is the command
  line the enable confirmation showed. A manifest whose `run` differs from
  it is not started, and the Plugins tab asks for the confirmation again
  (TM-plug-12).
- **Tern gates the protocol, not the process [load-bearing].** A plugin is
  a program running as the user: Tern can't stop it from using the network
  or the disk, so rights cover only what goes through Tern (OQ-031).
- **Trusted sources only [load-bearing].** Tern warns the user to install
  plugins only from a source they trust, because a plugin is a standalone
  program and Tern can't guarantee its security. The warning stands at the
  top of the Plugins tab and in the confirmation that enabling a plugin
  asks for, which also shows the plugin's command line and folder. The
  confirmation comes every time a plugin goes from disabled to enabled, and
  can't be turned off.
- **One process per plugin, started by its first event [load-bearing].**
  Enabling starts nothing. Tern starts a plugin's process when an event is
  due for it and none is running, and writes that event as its first
  line. A plugin may exit whenever it likes: after exit status 0 its next
  event starts it again. So a script that reads one line, answers and
  exits is a complete plugin, and a program that keeps reading stays
  running; the manifest has no mode. Events are one JSON object per line
  on stdin, actions one JSON object per line on stdout. stderr is the
  plugin's log, never parsed. Stdin rather than a parameter avoids
  Windows' 32 k command-line limit and argv quoting (OQ-032).
- **One event at a time [load-bearing].** After writing an event, Tern
  writes the plugin's next one only once the plugin sent `ready` or
  exited. Tern never writes an event the plugin didn't ask for, so none
  sits unread in the pipe of a process that exits, and none is lost. A
  plugin that stays running sends `ready` whenever it can take another
  event, also before it waits for an `action_result`; a plugin that exits
  after each event never sends it. Each `ready` asks for one more event:
  a plugin that sends two gets two, and has to read both before it exits.
- **EOF on stdin means stop [load-bearing].** Tern never signals a plugin
  any other way before it kills it. A plugin gets EOF when it is disabled
  or Tern quits: it ends its own
  children and exits within 2 s, or Tern kills the process it spawned
  and everything that process started, only while it still holds that
  process's unreaped handle (OQ-035, TM-plug-11; Platform notes).
- **Events** (Tern → plugin), each `{"api":1,"event":…,"settings":{…},…}`:
  `start`, `command`, `panel_opened`, `panel_closed`, `panel_action` (a
  button or row was clicked), `panel_input` (the panel's input field was
  submitted), `pane_opened`, `pane_closed`, `settings_changed`,
  `action_result`, `output`. `start`, `pane_opened` and `pane_closed` are
  sent only when the manifest lists them under `events` and the user
  ticked them. `start` is for a plugin that has to run without being
  asked: it is sent once after discovery at each launch, and when the
  plugin is enabled. The
  others answer the plugin's own commands, panels, settings and actions,
  so they reach every enabled plugin. Panes are opaque IDs unless `pane_info` is granted, which adds
  title, cwd, profile and host.
- **Actions** (plugin → Tern), each `{"action":…,"id":…}`:

  | Action | Does | Right | Confirmed |
  |---|---|---|---|
  | `ready` | Lets Tern write the plugin's next event | — | no |
  | `set_panel` | Replaces a panel's items: text, rows with buttons, buttons, command buttons, one input field, a log | — | no |
  | `append_log` | Appends text to a panel's log (streaming) | — | no |
  | `open_panel` | Shows one of its panels; never takes keyboard focus. Carried out once per act of the user's on the plugin, else dropped with a log line ([spec 0018](0018-app-sidebar.md), flow 6) | — | no |
  | `notify` | A notice; one at a time per plugin | — | no |
  | `set_settings` | Replaces the plugin's dynamic settings fields | — | no |
  | `fill_command` | Types text at a pane's prompt, without Enter | `terminal_write` | yes |
  | `run_command` | Types text and Enter | `terminal_write` | yes |
  | `read_output` | Sends the pane's selection, else its last `lines` lines (at most 1,000), as an `output` event | `terminal_read` | yes |
  | `open_tab` | Opens a tab running an argv, e.g. `docker exec -it web sh` | `open_tab` | yes |

- **Confirmation [load-bearing].** A confirmed action shows a bar on its
  target pane (the action's `pane`, else the terminal pane focused when it
  arrived; the target never follows focus later). The bar names the
  plugin, the pane by its title (plus `user@host` for an SSH pane), and
  shows exactly what will be typed, run or sent, with Allow and Deny.
  Names come from the Host's record of which process asked, never from the
  action. The bar never takes keyboard focus: typing, Enter
  included, keeps going to the shell, so a keystroke already under way
  can't confirm. Allow ignores clicks for 0.5 s after its bar appears or
  moves. Allow and Deny are clicks, or the palette commands
  `plugin.allowRequest` / `plugin.denyRequest` (no default keys), which act
  on the focused pane's request. One
  request per plugin is pending at a time; more are refused. Each request
  is confirmed on its own, every time. After a Deny, the plugin's
  confirmed actions are refused until the user next runs one of its
  commands or acts in one of its panels. Each confirmed action adds a line
  to the plugin's log with its kind, target pane, outcome and time, never
  its text (TM-plug-03, -07, -09, -20).
- **Command buttons [load-bearing].** A button in `set_panel` may carry
  one `fill_command`, `run_command` or `open_tab`. Tern draws it as a
  command button: its label and, under it, all of the text or argv it
  carries, by the untrusted-text rules below, in the terminal's monospace
  font. The click is the approval: Tern carries the command out without
  asking the plugin again and without a bar, because the user saw what
  the click runs. It needs the same right as the action; without it the
  button is drawn disabled with the reason. A button can't name a pane:
  its text goes to the terminal pane that last had focus, and a click
  with no terminal pane open shows a notice. A command button ignores
  clicks for 0.5 s after its panel's contents changed, so a `set_panel`
  can't swap the command under a click. The click adds the same line to
  the plugin's log as a confirmed action and then sends `panel_action`
  like any button. `read_output` is never on a button, since what it
  would send isn't known before the click (TM-plug-23).
- **Untrusted text [load-bearing].** Everything a plugin sends is shown as
  plain text with control characters removed. `fill_command` and
  `run_command` text, on a bar or a command button, containing a control character (newline, ESC, …) or
  a format character (Unicode `Cf`: direction overrides, zero-width and
  tag characters) is refused, and so is text over 4 KiB. The bar wraps
  and shows all of the text, never truncated, so it shows all of what would run.
  `open_tab` shows each argv element separately under the same rules. On
  Windows, an `open_tab` whose program contains `"` or ends in `.bat` or
  `.cmd` is refused, since `cmd.exe` would re-parse its arguments.
  `read_output`'s bar states the line and character count above a
  scrollable preview (TM-plug-04, -05, -16).
- **Commands.** A manifest command has the ID `plugin.<plugin>.<command>`
  and is one generic GPUI action carrying both names. It is in the palette
  and bindable like any command (spec 0004). The keymap accepts any
  `plugin.*` ID without reading a manifest; a key whose plugin or command
  is missing shows a notice when pressed.
- **Bounds.** A line over 1 MiB is dropped. A panel's log keeps its last
  10,000 lines, in memory only. Each plugin's reader hands lines over a
  bounded channel, so a flood blocks the plugin, not the UI. Lines are
  parsed on the reader thread. The plugin's stderr log
  keeps its last 10,000 lines, each cut at 1,024 characters, in memory only. Events wait in a queue of
  256 per plugin; one that doesn't fit, because the plugin isn't reading
  its stdin or never sends `ready`, is dropped with a line in
  the plugin's log. Tern doesn't kill a plugin for being slow: one that
  stalls holds up only its own events, until the user disables it
  (TM-plug-10).

### Module surface

- `plugins::Host`: owns the found plugins and their processes; `dispatch(plugin, Event)`; hands parsed `Action`s to the workspace on the foreground thread. `Event` and `Action` are enums (serde-tagged), not strings
- `plugins::Host` also holds each plugin's event queue and its count of `ready`s, so one-at-a-time delivery lives in one place
- Threads: per running plugin one stdout reader, one stderr reader and one stdin writer, so a plugin that doesn't read never blocks the background executor; spawning on the background executor. A plugin inherits only its three pipes, never a PTY's descriptor or handle (TM-plug-14)
- `plugins::PanelModel`: a panel's items, drawn by the app sidebar (T-064)
- `Workspace` hosts confirmations through `plugins/confirm.rs`; it doesn't own them (AGENTS.md, hazard 9)

### Flows

1. **Discovery**, after the first frame, off the UI thread: read each
   folder's manifest. A broken or oversized manifest, an `id` that differs
   from its folder, a `name` taken by another plugin, or an unknown `api`
   lists the plugin as unusable with the reason; it is never started.
2. **Enable:** the Plugins tab shows the plugin's command line, rights and
   events; the user enables it, confirms the trusted-source warning, and
   ticks rights and events. No process starts, unless `start` is ticked:
   that event is then due. Disabling closes a running plugin's stdin and,
   after 2 s, kills the PID Tern spawned.
3. **Event:** it joins the plugin's queue. With no process running, Tern
   spawns one and writes the event; otherwise it writes it once the
   plugin sent `ready`. A spawn that fails (missing program, no
   execute bit) shows a notice and marks the plugin stopped.
4. **Action arrives:** unknown action, malformed line, missing right or a
   panel not in the manifest: dropped, with a line in the plugin's log,
   not a notice per line. Otherwise carried out or queued for confirmation.
5. **Confirmation:** Allow carries it out; Deny, a second pending request,
   or the target pane closing first drops it. The plugin gets
   `action_result` (`allowed`, `denied`, `refused`, `no_pane`) with the
   action's `id`, an event like any other: it waits for `ready`, and
   starts a plugin that has exited. `read_output`'s bar
   previews the exact text; the `output` event carries that text and
   nothing else.
6. **Plugin exits:** with status 0, nothing is shown: its panels keep
   their contents, and its next event starts it again. Any other exit is
   a crash: one notice, its panels show "stopped" with Restart, and the
   Plugins tab shows its log. No automatic restart: a stopped plugin's
   events are dropped, and its commands show a notice, until Restart
   (which sends that panel's `panel_opened` again) or until it is
   disabled and enabled.
7. **Quit:** close every stdin, wait up to 2 s in total, kill what's left
   by captured PID. Never by name.
8. **Settings:** manifest fields (`text`, `bool`, `number`, `choice`) and
   `set_settings` fields are drawn on the plugin's page and written under
   `plugins.<id>.settings` through spec 0009's in-place edits; a change
   sends `settings_changed` to a plugin that is running. One that isn't
   gets the new settings with its next event, as every event carries them. The page says these fields are saved in
   `config.yaml` as plain text and are no place for secrets (TM-plug-18).

### Startup impact

Nothing. The boot path reads no manifest and starts no plugin: `plugins:`
is parsed with the config it is already reading, and discovery starts
after the first frame on a background thread. T-066 reports
`--bench-startup` with three plugins enabled that get `start`, against none.

### Platform notes

- Windows 11: spawn with `CREATE_NO_WINDOW` so no console flashes. Scripts
  aren't executable by themselves, so `run` names the interpreter
  (`[python, main.py]`); a manifest may give `run` per OS. Paths in events
  are native (`C:\…`).
- Linux: the program needs its execute bit; a missing one is the spawn
  failure of flow 3.
- A kill ends the plugin's process tree (OQ-035). Linux: each plugin
  leads its own process group, killed with `killpg` while the leader is
  unreaped; a plugin that exited by itself leaves its children running.
  Windows: each plugin process has its own Job object with
  `KILL_ON_JOB_CLOSE`, so what it started also ends when it exits by
  itself and when Tern crashes. A child that leaves its group or job is
  beyond Tern (OQ-031).
- macOS: compiles; same code path as Linux.

### Security & privacy

- **Stored:** `plugins.<id>` in `config.yaml` (enabled, confirmed `run`,
  rights, settings; no secrets). Panel contents, logs and stderr live in memory only. Tern
  logs no event or action payload, and text sent by `read_output` is never
  kept after delivery.
- **Trust boundaries:** a new one, Tern ↔ plugin process. Plugin output is
  untrusted input, like escape sequences (§Security 1): plain text only,
  and it never reaches a PTY or a command line without the user's
  approval above, on a bar or a command button (§Security 2). Terminal output crosses to a plugin only per
  confirmed `read_output`.
- **Ground rules:** Tern itself sends nothing. A plugin can, as soon as it
  starts after the first frame: enabling it, after the trusted-source
  warning, is the user's decision (ground rule 1 as amended by OQ-031).
- STRIDE model: [threat model 0005](threat-model/0005-plugins.md). Its
  mitigations are in this spec; its residual risks await the owner in
  OQ-035.

## Acceptance criteria

1. A folder with a valid manifest dropped into the plugins folder shows up on the Plugins tab as disabled, and its program has not run (a plugin that creates a file on start created none). Enabling it asks for confirmation with the trusted-source warning and its command line; cancelling leaves it disabled and not started. A manifest with `api: 2` is listed as unusable and never started.
2. With three plugins enabled that get `start`, no plugin process exists before the first-frame mark, and `--bench-startup 10` is within noise of a run with none. An enabled plugin without `start` has no process until its first event.
3. A manifest command appears in the palette under its title; running it puts exactly one JSON line with `"event":"command"` on the plugin's stdin. Run twice in quick succession on a plugin that reads one line and exits 0, it starts two processes, the second after the first exited, each getting one line, with no notice. A plugin that stays running gets the second line only after it sent `ready`.
4. `run_command` shows a bar on the target pane with the exact text. Typing `ls` and Enter while it shows runs `ls` in the shell and confirms nothing. Deny writes nothing to the PTY; Allow types the text and Enter; the plugin gets `action_result` either way (Linux and Windows).
5. With `terminal_write` not ticked, `run_command` shows no bar, the plugin gets `refused`, and its log names the missing right. With `pane_opened` declared but not ticked, opening a pane sends the plugin nothing.
6. After `read_output`, the plugin receives nothing until Allow; the `output` text equals the bar's preview byte for byte; with Deny it gets `denied` and no text.
7. `fill_command` with a newline, ESC or a right-to-left override (U+202E) in its text is refused, and panel text containing escape sequences renders as plain text.
8. A plugin writing 100,000 `append_log` lines a second leaves typing in a pane responsive, its panel shows the last 10,000 lines, and Tern's memory stays flat.
9. Killing a running plugin's process shows one notice and "stopped" with Restart on its panels; other plugins and panes carry on. Quitting Tern leaves no plugin process behind on Linux and Windows, and starting a plugin on Windows flashes no console window.
10. A panel button carrying `run_command` shows all of its text under its label; one click types the text and Enter into the terminal pane that last had focus, with no bar, and the plugin gets `panel_action` (Linux and Windows). With `terminal_write` not ticked the button is disabled, and a click within 0.5 s of a `set_panel` that changed the panel runs nothing.

## Open questions

- Does a plugin that hears `pane_opened` learn about the panes already open when it was found or enabled? Until answered, no (OQ-037).
- The residual risks of threat model 0005 await the owner's sign-off in OQ-035; its process-tree question is answered.
