---
status: approved     # draft | approved | implemented
phase: 1
tasks: [T-004]
related: [spec 0002, spec 0003, spec 0005]
---

# Spec: Commands, keybindings and command palette

Pins the single registry of app actions that keyboard shortcuts, the
command palette and menus all go through. Phase 2/3 add hosts and agents to
the same palette.

## Problem & goal

Ease of use means every action can be found (palette) and is fast (shortcut)
without stealing keys the shell needs.

Done means: every Phase 1 action is a registered command, reachable from the
palette, and bindable in `config.yaml`.

## Scope

**In scope:**

- Command registry (id, title, handler, optional enabled-check)
- Default keybindings per platform; user overrides in config
- Command palette with fuzzy search, showing each command's binding
- A palette provider interface. Phase 1's first provider lists profiles ("Open profile: Htop", spec 0005); Phase 2/3 extend it with hosts and agents

**Out of scope:**

- Chorded / leader-key bindings: *Later*
- Binding a key to a specific profile ("open htop"): *Later*; profiles open from the palette
- Native OS menu bar: *Later*

## Design

### Affected areas

- `src/commands/` (registry, keymap, palette)

### Model

Commands are GPUI actions, one type each (`NewTab`, `SplitRight`,
`Search`, `OpenConfig`, …), each with a stable ID (`tab.new`,
`pane.splitRight`) used in config and shown in the palette. A typo in a
default binding is a compile error. Config-provided bindings are parsed at
runtime, and unknown IDs produce a notice. Bindings live in key contexts:
`Workspace` for app actions, plus `Palette`, `Search` and `TextField` for
focused inputs. The terminal's own context binds nothing that a shell needs.

**Default bindings (Linux/Windows; macOS swaps `Ctrl+Shift` → `Cmd`):**

| Action | Keys |
|---|---|
| Command palette | `Ctrl+Shift+P` |
| Connect to host (spec 0006) | `Ctrl+Shift+H` |
| New tab / close pane | `Ctrl+Shift+T` / `Ctrl+Shift+W` |
| Next / previous tab | `Ctrl+Tab` / `Ctrl+Shift+Tab` |
| Go to tab 1–9 | `Ctrl+Shift+1` … `Ctrl+Shift+9` |
| Split right / down | `Ctrl+Shift+D` / `Ctrl+Shift+E` |
| Focus pane | `Alt+←↑→↓` |
| Zoom pane | `Ctrl+Shift+Z` |
| Copy / paste / search | `Ctrl+Shift+C` / `Ctrl+Shift+V` / `Ctrl+Shift+F` |
| Font size + / − / reset | `Ctrl+Shift+=` / `Ctrl+Shift+-` / `Ctrl+Shift+0` |

**[load-bearing]** Defaults never bind `Ctrl` or `Alt` plus a letter, digit
or punctuation key without `Shift`, because those belong to the shell and
to readline/emacs/vim (`Ctrl+R`, `Alt+B`, `Alt+3` digit argument, `Ctrl+-`
undo). The user may override this in config.

Digit shortcuts must not depend on the keyboard layout: `Ctrl+Shift+1`
works even where `Shift+1` types `!` or another symbol.

### Flows

1. **Key event:** GPUI matches it against the bindings of the focused
   element's key contexts, innermost first. On a match the action runs and
   the key is consumed; otherwise the focused pane receives it and sends it
   to the shell (spec 0002). When a text field (palette, search, tab
   rename) has focus, its `TextField` context binds plain
   `Ctrl+C`/`Ctrl+X`/`Ctrl+V` to that field's copy, cut and paste.
2. **Palette:** opens centered over the focused pane. Typing fuzzy-filters
   titles. `↑↓` moves the selection, `Enter` runs the command, `Esc` closes.
   Focus returns to the pane it came from.
3. **Disabled command** (e.g. "Zoom pane" with one pane): it is shown dimmed
   in the palette and is a no-op from its shortcut.
4. **Invalid binding in config** (unknown command, unparsable key): ignore
   that entry, keep the defaults for the rest, and show a notice.

### Startup impact

The keymap is built from config on the boot path (a few hundred entries at
most). The palette's entry list and matcher state are built on first open.

### Platform notes

Windows 11: `Alt` combinations can collide with window-menu accelerators, so
verify `Alt+arrows` there. Non-US layouts (German, French) put symbols on
`Shift+digit`; check `Ctrl+Shift+1` on one. macOS: `Cmd` variants only.

### Security & privacy

- Stored: nothing. No recently-used-command history in Phase 1.
- Trust boundaries: none crossed. Palette entries (including profile names
  from config) render as plain text.

## Acceptance criteria

1. `Ctrl+Shift+P` opens the palette on the next frame. Typing `splr` ranks "Split right" first.
2. The palette shows the current binding next to each command, including user overrides.
3. Rebinding `pane.splitRight` to `Alt+Shift+D` in config works after hot reload, and the old key goes to the shell.
4. A binding for unknown command `tab.explode` produces a notice, and all other bindings still work.
5. With default config, `Ctrl+R` in bash reaches readline (reverse search), `Alt+B` moves back a word, and `Ctrl+-` undoes the last edit.
6. On a German keyboard layout, `Ctrl+Shift+2` goes to tab 2.
7. Every Phase 1 action from specs 0002/0003/0005 appears in the palette.

## Open questions

- Fuzzy matcher: a small hand-written subsequence scorer vs a crate (`nucleo`). Prefer hand-written unless ranking is poor.
