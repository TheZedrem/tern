---
status: approved     # draft | approved | implemented
phase: 5
tasks: [T-064, T-065, T-066, T-067, T-068, T-069, T-070, T-071]
related: [spec 0001, spec 0004, spec 0005, spec 0009, spec 0013, OQ-031 ✅, OQ-032 ✅]
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
nothing touching a terminal before the user confirmed it.

## Scope

**In scope:**

- Plugin folders, the manifest, enabling and rights per plugin
- The runtime: background and on-demand processes, JSON lines over stdio
- Events and actions, version 1 of the protocol (`api: 1`)
- Confirmation of every action that writes to or reads from a terminal
- A Plugins tab on the settings screen: enable, rights, the plugin's own settings, its log
- Panel contents: what a plugin can put into a sidebar panel

**Out of scope:**

- Where panels are drawn, and the SSH hosts as the first built-in panel: the app sidebar's own spec (T-064); panels wait for it
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
  mode: background                # or on-demand
  rights: [pane_info, terminal_write]
  events: [pane_opened, pane_closed]
  commands: [{ id: containers, title: "Docker: show containers" }]
  panels: [{ id: containers, title: Containers, icon: container }]
  settings: [{ key: socket, label: Docker socket, type: text }]
  ```

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
      rights: [pane_info, terminal_write]
      events: [pane_opened, pane_closed]
      settings: { socket: /var/run/docker.sock }
  ```

  The rights and events in force are the granted ones that the manifest
  still declares. One a new version adds stays off until the user ticks
  it, and an event that isn't ticked is never sent.
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
- **Two modes, one protocol.** A `background` plugin is started once and
  kept running; an `on-demand` plugin is started once per event and exits.
  Both get events as one JSON object per line on stdin and write actions
  as one JSON object per line on stdout. stderr is the plugin's log,
  never parsed. Stdin rather than a parameter avoids Windows' 32 k
  command-line limit and argv quoting, and lets a plugin switch modes
  without a code change (OQ-032).
- **EOF on stdin means stop [load-bearing].** Tern never signals a plugin
  any other way before it kills it. An on-demand plugin gets EOF right
  after its one event: it finishes its actions and exits. A background
  plugin gets EOF when it is disabled or Tern quits: it ends its own
  children and exits within 2 s, or Tern kills the PID it spawned.
- **Events** (Tern → plugin), each `{"api":1,"event":…,"settings":{…},…}`:
  `start`, `command`, `panel_opened`, `panel_closed`, `panel_action` (a
  button or row was clicked), `panel_input` (the panel's input field was
  submitted), `pane_opened`, `pane_closed`, `settings_changed`,
  `action_result`, `output`. `pane_opened` and `pane_closed` are sent only when
  the manifest lists them under `events` and the user ticked them. The
  others answer the plugin's own commands, panels, settings and actions,
  so they reach every enabled plugin. Panes are opaque IDs unless `pane_info` is granted, which adds
  title, cwd, profile and host.
- **Actions** (plugin → Tern), each `{"action":…,"id":…}`:

  | Action | Does | Right | Confirmed |
  |---|---|---|---|
  | `set_panel` | Replaces a panel's items: text, rows with buttons, buttons, one input field, a log | — | no |
  | `append_log` | Appends text to a panel's log (streaming) | — | no |
  | `open_panel` | Shows one of its panels; never takes keyboard focus | — | no |
  | `notify` | A notice; one at a time per plugin | — | no |
  | `set_settings` | Replaces the plugin's dynamic settings fields | — | no |
  | `fill_command` | Types text at a pane's prompt, without Enter | `terminal_write` | yes |
  | `run_command` | Types text and Enter | `terminal_write` | yes |
  | `read_output` | Sends the pane's selection, else its last `lines` lines (at most 1,000), as an `output` event | `terminal_read` | yes |
  | `open_tab` | Opens a tab running an argv, e.g. `docker exec -it web sh` | `open_tab` | yes |

- **Confirmation [load-bearing].** A confirmed action shows a bar on its
  target pane (the action's `pane`, else the focused terminal pane) naming
  the plugin and showing exactly what will be typed, run or sent, with
  Allow and Deny. The bar never takes keyboard focus: typing, Enter
  included, keeps going to the shell, so a keystroke already under way
  can't confirm. Allow and Deny are clicks, or the palette commands
  `plugin.allowRequest` / `plugin.denyRequest` (no default keys). One
  request per plugin is pending at a time; more are refused. Each request
  is confirmed on its own, every time.
- **Untrusted text [load-bearing].** Everything a plugin sends is shown as
  plain text with control characters removed. `fill_command` and
  `run_command` text containing a control character (newline, ESC, …) is
  refused, so the bar shows all of what would run.
- **Commands.** A manifest command has the ID `plugin.<plugin>.<command>`
  and is one generic GPUI action carrying both names. It is in the palette
  and bindable like any command (spec 0004). The keymap accepts any
  `plugin.*` ID without reading a manifest; a key whose plugin or command
  is missing shows a notice when pressed.
- **Bounds.** A line over 1 MiB is dropped. A panel's log keeps its last
  10,000 lines, in memory only. Each plugin's reader hands lines over a
  bounded channel, so a flood blocks the plugin, not the UI. An on-demand
  run is killed after 30 s; on-demand events of one plugin run one after
  another.

### Module surface

- `plugins::Host`: owns the found plugins and their processes; `dispatch(plugin, Event)`; hands parsed `Action`s to the workspace on the foreground thread. `Event` and `Action` are enums (serde-tagged), not strings
- Threads: per running plugin one stdout reader and one stderr reader; spawning and stdin writes on the background executor
- `plugins::PanelModel`: a panel's items, drawn by the app sidebar (T-064)
- `Workspace` hosts confirmations through `plugins/confirm.rs`; it doesn't own them (AGENTS.md, hazard 9)

### Flows

1. **Discovery**, after the first frame, off the UI thread: read each
   folder's manifest. A broken manifest, an `id` that differs from its
   folder, or an unknown `api` lists the plugin as unusable with the
   reason; it is never started.
2. **Enable:** the Plugins tab shows the plugin's command line, rights and
   events; the user enables it, confirms the trusted-source warning, and
   ticks rights and events; a `background` plugin
   starts and gets `start`. Disabling closes its stdin and, after 2 s,
   kills the PID Tern spawned.
3. **Event:** background: write the line. On-demand: spawn, write, close
   stdin, read actions until exit. A spawn that fails (missing program, no
   execute bit) shows a notice and marks the plugin stopped.
4. **Action arrives:** unknown action, malformed line, missing right or a
   panel not in the manifest: dropped, with a line in the plugin's log,
   not a notice per line. Otherwise carried out or queued for confirmation.
5. **Confirmation:** Allow carries it out; Deny, a second pending request,
   or the target pane closing first drops it. The plugin gets
   `action_result` (`allowed`, `denied`, `refused`, `no_pane`) with the
   action's `id`; for on-demand that is a new run. `read_output`'s bar
   previews the exact text; the `output` event carries that text and
   nothing else.
6. **Plugin exits or crashes** (background): one notice, its panels show
   "stopped" with Restart, and the Plugins tab shows its log. No automatic
   restart.
7. **Quit:** close every stdin, wait up to 2 s in total, kill what's left
   by captured PID. Never by name.
8. **Settings:** manifest fields (`text`, `bool`, `number`, `choice`) and
   `set_settings` fields are drawn on the plugin's page and written under
   `plugins.<id>.settings` through spec 0009's in-place edits; a change
   sends `settings_changed`.

### Startup impact

Nothing. The boot path reads no manifest and starts no plugin: `plugins:`
is parsed with the config it is already reading, and discovery starts
after the first frame on a background thread. T-066 reports
`--bench-startup` with three background plugins enabled against none.

### Platform notes

- Windows 11: spawn with `CREATE_NO_WINDOW` so no console flashes. Scripts
  aren't executable by themselves, so `run` names the interpreter
  (`[python, main.py]`); a manifest may give `run` per OS. Paths in events
  are native (`C:\…`).
- Linux: the program needs its execute bit; a missing one is the spawn
  failure of flow 3.
- Both: killing the spawned PID leaves a plugin's own children running. A
  plugin must end them when its stdin closes (Open questions).
- macOS: compiles; same code path as Linux.

### Security & privacy

- **Stored:** `plugins.<id>` in `config.yaml` (enabled, rights, settings;
  no secrets). Panel contents, logs and stderr live in memory only. Tern
  logs no event or action payload, and text sent by `read_output` is never
  kept after delivery.
- **Trust boundaries:** a new one, Tern ↔ plugin process. Plugin output is
  untrusted input, like escape sequences (§Security 1): plain text only,
  and it never reaches a PTY or a command line without the confirmation
  above (§Security 2). Terminal output crosses to a plugin only per
  confirmed `read_output`.
- **Ground rules:** Tern itself sends nothing. A plugin can, as soon as it
  starts after the first frame: enabling it, after the trusted-source
  warning, is the user's decision (ground rule 1 as amended by OQ-031).
- The STRIDE model is T-065 and comes before any code.

## Acceptance criteria

1. A folder with a valid manifest dropped into the plugins folder shows up on the Plugins tab as disabled, and its program has not run (a plugin that creates a file on start created none). Enabling it asks for confirmation with the trusted-source warning and its command line; cancelling leaves it disabled and not started.
2. With three background plugins enabled, no plugin process exists before the first-frame mark, and `--bench-startup 10` is within noise of a run with none.
3. A manifest command appears in the palette under its title; running it puts exactly one JSON line with `"event":"command"` on the plugin's stdin. For an on-demand plugin, one process starts per event and sees EOF after the line.
4. `run_command` shows a bar on the target pane with the exact text. Typing `ls` and Enter while it shows runs `ls` in the shell and confirms nothing. Deny writes nothing to the PTY; Allow types the text and Enter; the plugin gets `action_result` either way (Linux and Windows).
5. With `terminal_write` not ticked, `run_command` shows no bar, the plugin gets `refused`, and its log names the missing right. With `pane_opened` declared but not ticked, opening a pane sends the plugin nothing.
6. After `read_output`, the plugin receives nothing until Allow; the `output` text equals the bar's preview byte for byte; with Deny it gets `denied` and no text.
7. `fill_command` with a newline or ESC in its text is refused, and panel text containing escape sequences renders as plain text.
8. A plugin writing 100,000 `append_log` lines a second leaves typing in a pane responsive, its panel shows the last 10,000 lines, and Tern's memory stays flat.
9. Killing a background plugin's process shows one notice and "stopped" with Restart on its panels; other plugins and panes carry on. A manifest with `api: 2` is listed as unusable and never started.
10. Quitting Tern leaves no plugin process behind on Linux and Windows, and enabling a plugin on Windows flashes no console window.

## Open questions

- A plugin's own child processes outlive a kill of its PID. Is the EOF rule enough for a plugin that ignores it, or does Tern kill the whole tree (a process group on Linux, a Job Object on Windows)? T-065 decides with the threat model.
