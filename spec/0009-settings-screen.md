---
status: approved     # draft | approved | implemented
phase: 1
tasks: [T-017, T-011]
related: [spec 0004, spec 0005, spec 0006, OQ-010 ✅]
---

# Spec: Settings screen

Pins a screen for changing `config.yaml` without opening the file: font,
theme, cursor and behavior, keybindings and profiles, applied live through
the config's own reload.

## Problem & goal

The config file is the source of truth (spec 0005), but not everyone wants
to edit YAML to change a font or add a host. A settings screen should
change the same file the way a careful person would: only the value
changed, comments and layout kept.

Done means: every setting of spec 0005, the keybindings and the profiles
can be changed from the screen, the change applies within a second, and
`config.yaml` differs only in the lines of what changed.

## Scope

**In scope:**

- "Open settings" (`settings.open`) in the palette; no default key (no `Shift` combination for it works on every layout, OQ-014)
- Sections: Font (family from the installed fonts, size, line height), Theme, Cursor, Behavior (`scrollback`, `close_on_exit`, `copy_on_select`, `open_links`, `restore_session`, `default_profile`), Keybindings, Profiles
- Keybindings: each command with its key; record a new one, or reset it to the default
- Profiles: add, edit, delete local and SSH profiles (spec 0005, 0006 fields)

**Out of scope:**

- Editing themes' colors: *Later*; custom themes stay files
- Settings stored anywhere but `config.yaml`: never (spec 0005)

## Design

### Affected areas

- `src/settings/` (the screen), `src/config.rs` (in-place edits, generalizing `save_theme_default`), the palette's commands (spec 0004)

### Model

- **The file is the state [load-bearing]:** the screen reads its values from
  the loaded config and writes only to `config.yaml`; the config watcher
  reloads it and everything applies through spec 0005's reload, as if it
  had been edited by hand. The screen keeps no copy of its own.
- **In-place edits [load-bearing]:** a change rewrites only the lines of
  its key (or, for a profile, of that profile's block), adding the key in
  block style where it's missing; every other byte of the file stays. A
  key written in a way the edit can't change safely (flow style, anchors,
  a value over several lines) isn't written: a notice names the key to
  edit by hand. As `save_theme_default` does today: the file is read again
  right before writing, and left alone if it keeps changing (spec 0005,
  flow 5).
- **No secrets [load-bearing]:** the profile form has no password field.
  Passwords stay in the keychain, offered at connect time (spec 0006).
- **Always complete [load-bearing]:** every setting a user can set in
  `config.yaml` has its field here. A task that adds or changes one adds
  its field in the same change (AGENTS.md, Definition of Done; the owner's
  standing rule). Commands come from the command list and profiles from
  the config, so new ones appear by themselves.

### Flows

1. **Open:** the palette command, or the gear in the tab list's corner
   (after `+` in a bar, at the bottom of a side list), shows the screen as a panel over the
   window; Esc or its close button leaves it. It shows the config as
   loaded, and follows reloads while open. Like Tabby's, it has tabs down
   its left side (General, Appearance, Terminal, Keyboard, Profiles, and Import from spec 0014), each
   grouping its settings under headings, and the app's version below
   them; the panes below take no clicks
   while it's open. Its header's "Open config folder" button (also the
   palette's `config.openFolder`) shows `config.yaml` in the OS's file
   manager, creating it first like "Open config file" does.
2. **Change a value:** a field's change is written when it's committed
   (Enter, leaving the field, picking from a list), then applies live.
   An invalid value can't be committed: the field says why.
3. **Keybinding:** "Record" takes the next key combination as the
   command's key and writes it under `keybindings:`; a default it takes
   over goes to the shell, under spec 0004's rules. "Reset" removes the
   entry.
4. **Profiles:** "Add" opens an empty profile form (type local or SSH);
   its directory field has a "Browse…" button that opens the OS's folder
   picker (the XDG portal on Linux) and fills in the pick, `~` for home;
   saving appends its block under `profiles:`. Editing rewrites its block;
   deleting it asks first and removes its block only.
5. **Failure:** a write that fails or is refused shows a notice and leaves
   the field showing what the file holds.
6. **Search:** the screen opens with the keyboard in its search field.
   A query shows every setting, keybinding and profile across all tabs
   whose label, group, tab or keywords (like "zoom" for the font size or
   "url" for links) have words starting with each of its words, under
   "Tab · Group" headings; an empty query shows the tab again.

### Startup impact

None: the screen is built when it first opens.

### Platform notes

The installed fonts come from GPUI's text system on each platform. Paths in
profiles (`cwd`, `identity_file`) are written as typed, `~` included
(spec 0005). Windows keys follow GPUI's Windows key mapper (T-011).

### Admin area

None: Tern is a local, single-user app with no admin area.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| Settings and profiles: hosts, users, key paths | `config.yaml`, as today | What the user configures | The user's own device; no one else processes it | Until the user changes it |

Nothing new is stored or sent. No trust boundary is crossed: the screen
writes what the user types into their own config, and the config loader's
checks apply to it as to any hand edit.

## Acceptance criteria

1. Changing the font size on the screen resizes every pane within a second, and `config.yaml` differs only in the `size:` line; comments and order stay.
2. A theme picked on the screen applies live and is what `theme.default` says in the file.
3. Recording `Ctrl+Shift+K` for "Split right" makes it split right, writes it under `keybindings:`, and `Ctrl+Shift+D` then reaches the shell.
4. An SSH profile added on the screen shows in the host picker; deleting it removes only its lines from the file.
5. Editing `config.yaml` in an editor while the screen is open shows the edit on the screen, and a later change on the screen keeps it.
6. A `font: { size: 13 }` in flow style gets a notice naming `font.size` and isn't rewritten.
7. The profile form has no password field, and nothing the screen writes contains a secret.
8. The screen works by keyboard alone: Tab moves through the fields, and Esc leaves the screen.

## Open questions

- None.
