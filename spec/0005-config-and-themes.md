---
status: approved     # draft | approved | implemented
phase: 1
tasks: [T-005, T-004, T-002, T-011, T-019]
related: [spec 0001, spec 0003, spec 0004]
---

# Spec: Config, profiles and themes

Pins the YAML config: its shape, **profiles** as the unit of "what a pane
runs", error handling, hot reload, and themes (built-in, theme files and
inline themes, all resolved to complete palettes at load time).

## Problem & goal

Settings live in one plain file people can version in their dotfiles. It
must apply without a restart and never break startup. Profiles make "open
htop", "open my server" and later "open Claude Code" the same one-click
action.

Done means: editing `config.yaml` changes the running app within a second, a
broken file never stops boot, local profiles open from the palette, and
switching themes previews live.

## Scope

**In scope:**

- `config.yaml` schema with defaults for every key
- Profiles of type `local` (command with args, cwd, env, theme)
- `default_profile`, plus "Open profile …" palette entries (spec 0004)
- Themes: built-ins, files in `themes/*.yaml`, inline `theme.custom`, partial themes, `system` light/dark
- Hot reload; "Open config file"; palette theme picker with live preview

**Out of scope:**

- What `type: ssh` profiles do, and the host picker: spec 0006 (the schema below shows them so the shape is fixed here)
- Per-profile fonts: *Later*
- The graphical settings page: spec 0009

## Design

### Affected areas

- `src/config/`, `src/theme.rs`, `themes/*.yaml` (built-ins, embedded), `src/commands/` (theme picker)

### Model

Every key is optional:

```yaml
default_profile: shell              # missing → shell, the login shell

profiles:                           # map key = stable profile ID
  shell:
    type: local                     # no command → the user's login shell
    env: { EDITOR: nvim }
  htop:
    name: Htop                      # display name; defaults to the ID
    type: local
    command: [htop]                 # program + args
    cwd: ~/projects                 # optional, default: home
  my-server:                        # spec 0006
    type: ssh
    host: myserver.example.com      # or a Host alias from ~/.ssh/config
    user: myuser                    # optional
    port: 22                        # optional
    identity_file: ~/.ssh/id_ed25519 # optional; omit to let ssh choose
    group: Production               # optional; host picker group, default "Hosts"
    theme: midnight

font: { family: null, size: 13, line_height: 1.2 }   # null → the bundled font
scrollback: 10000
cursor: { style: block, blink: true }
copy_on_select: false
open_links: ctrl_click              # ctrl_click | click (spec 0002, flow 7)
tab_bar: top                        # top | bottom | left | right (spec 0003)
sftp_sidebar: right                 # left | right (spec 0008)
sftp_click: edit                    # edit | download (spec 0008)
sftp_upload: manual                 # manual | save | focus (spec 0008)
files_executable: ask               # ask | run | edit (spec 0013)
new_tab: ask                        # what Ctrl+Shift+T opens: ask | default (spec 0004)
close_on_exit: clean                # always | clean | never
restore_session: true
keybindings: { "ctrl+shift+d": pane.splitRight }

theme:
  default: dark                     # dark | light | system | any theme name
  custom:                           # inline themes
    midnight:
      base: dark                    # missing colors come from here (default: dark)
      background: "#1e1e2e"
      foreground: "#cdd6f4"
      cursor: "#f5c2e7"
```

- **Profiles** are a map, so IDs are unique by construction and stay stable
  for `default_profile` and session restore. Parsing keeps file order,
  because the palette listing depends on it. A `shell` profile, the login
  shell, is always there without being configured, and it is what new
  tabs open unless `default_profile` names another; a configured profile
  with the ID `shell` replaces it. A profile's `theme` applies to that
  profile's panes only.
- **Typed per `type`** **[load-bearing]**: serde parses each profile as one
  variant (internally tagged on `type`). A bad profile is skipped with a
  notice naming its ID; the other profiles still load. Phase 1 launches only
  `local`.
- **Theme** = 16 ANSI colors, foreground, background, cursor, selection,
  UI accent. Its keys are `background`, `foreground`, `cursor`,
  `selection`, `accent`, `black` … `white` and `bright_black` …
  `bright_white`, each a quoted `"#rrggbb"`. A theme may be partial:
  missing colors come from `base`.
- **Theme lookup** **[load-bearing]**: inline `theme.custom` → `themes/<name>.yaml`
  → built-in. The first match wins. Built-ins are `dark` and `light`,
  complete palettes compiled into the binary: every partial theme resolves
  down to one, so they can't fail to load. A theme named like its base (an
  inline `dark` with no other `base`) builds on the next source down, so it
  can adjust the built-in.
- **Resolved at load** **[load-bearing]**: the UI only ever sees complete
  palettes, so the window's clear color is known before the window exists
  (spec 0001). The same palette is written into `gpui-component`'s theme,
  so widgets (tab bar, palette, toasts) match the terminal.
- **Font** **[load-bearing]**: `font.family: null` (the default) uses Lilex,
  embedded in the binary and registered with GPUI's text system at boot. A
  named family must match an installed font's exact name, because GPUI has
  no generic names like `monospace`. If it isn't found, Lilex is used with
  a notice. `font.size` is clamped to 4–200 and `font.line_height` to
  0.5–4, so a nonsense value can't size the grid at billions of cells.
- **`system`** **[load-bearing]**: boot always renders `dark`, with no OS
  query on the boot path. After the first frame, the app reads the OS
  light/dark setting. If it's light, every pane using the default theme and
  the window background switch to `light`, the same way a hot reload does.
  Because detection is off the boot path, it may use the slower but correct
  source (the XDG settings portal on Linux, the registry on Windows).

Unknown keys produce a notice, not an error. A value of the wrong type
makes its top-level key fall back to its default (`font.size: large` resets
all of `font`), with a notice; the rest of the file still applies. A
profile or inline theme that doesn't fit is skipped on its own, and there
an unknown key counts as not fitting: a typo like `comand` would otherwise
silently run the login shell.

### Module surface

- `config::load(path) -> Loaded { config, notices }`, used at boot and by the watcher
- `theme::Library::new(custom, dir).resolve(name, &mut notices) -> Theme`; the picker's `theme::list()` arrives with T-004
- `config::ensure_exists(path)`: create the commented default if it's missing; the "Open config file" action then opens it with the OS default handler
- Hot reload uses the `notify` crate; a reload replaces the shared config and notifies the views that read it. It watches the config directory, its parent (so a directory created later is picked up) and, where `config.yaml` or a theme file is a link (into a dotfiles repo, say), the directory it points into

### Flows

1. **Boot:** read and parse, collecting notices. A missing file means
   defaults and no notice. An unknown `default_profile` or theme name falls
   back (`shell` / `dark`) with a notice.
2. **Open a profile** (palette entry, keybinding, or a new tab, which uses
   `default_profile`): spawn the profile's command in its cwd with its env
   merged over the inherited environment. A missing cwd falls back to home.
   A non-`local` profile in Phase 1 shows a notice and doesn't open.
3. **Hot reload:** the watcher on `config.yaml` and `themes/` starts after
   the first frame (spec 0001), or a second after boot for a program that
   prints nothing, and is debounced at 200 ms. Problems surface as notices
   (architecture §Cross-cutting rules). Font, themes, cursor, scrollback and keybindings apply live to
   all open panes. Profile changes apply to newly opened panes; running
   panes keep what they were started with.
4. **Theme picker:** a palette command lists all themes. Moving the
   selection previews live; `Enter` writes `theme.default`; `Esc` reverts.
   The write changes only that key and keeps comments and formatting.
5. **Write conflict** (the file changed between read and write): re-read and
   retry once. If it still conflicts, show a notice and don't write.

### Startup impact

One synchronous read of `config.yaml`, plus one small read per theme file a
boot pane uses. A named `font.family` adds no lookup of its own, because
GPUI has already loaded the system fonts. No writes, no watcher and no OS
theme query at boot. `system` detection runs
after the first frame. The picker state is built on first open.

### Platform notes

Paths come from `dirs`: `~/.config/tern/` on Linux, `%APPDATA%\tern\`
on Windows. `~` in `cwd`/`identity_file` expands on both. On Windows,
`command: [htop]` resolves through `PATH` and `PATHEXT`. File watchers on
Windows report editor save-via-rename differently, so test with VS Code and
Notepad.

### Security & privacy

- Stored: `config.yaml` and `themes/*.yaml`, written only by the user, or by
  the app on an explicit action (theme picker, "Open config file").
- **No secrets in config, ever.** There is no `password:` key. Phase 2
  passwords go to the keychain. A `password` key in a profile is rejected
  with a notice telling the user where secrets belong.
- Trust boundaries: none crossed. Profiles run commands only on explicit user action.

## Acceptance criteria

1. With no config file, the app boots to the login shell on the `dark` theme with the bundled font and creates no file.
2. "Open config file" creates a commented default that parses back to exactly the defaults.
3. Opening the `htop` profile from the palette runs htop in a new tab, in the profile's cwd, with its env applied.
4. A profile with `command: 42` is skipped with a notice naming it, and every other profile still opens.
5. An inline theme `midnight` with only three colors renders those three and takes the rest from `dark`.
6. With the same name defined inline, as a file and as a built-in, the inline one wins. Remove it and the file wins.
7. Changing `font.size` in an editor and saving updates every open pane within 1 s. Editing a color in `themes/<name>.yaml` for the active theme does the same.
8. In the theme picker, `Esc` leaves the file untouched, and `Enter` changes only `theme.default` with comments intact.
9. The window's first frame uses the resolved default theme's background (no white flash). With `system` on a light-mode OS, the first frame is `dark`, and the switch to `light` happens after the TTFP report.
10. `font.family: "Nonexistent Mono"` boots with the bundled font and a notice naming the family.

## Open questions

- Comment-preserving single-key write: if `serde-saphyr` can't round-trip, do a targeted line edit of `theme.default` (insert it if it's missing).
