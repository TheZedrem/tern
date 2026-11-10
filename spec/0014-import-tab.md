---
status: implemented  # draft | approved | implemented
phase: 2
tasks: [T-047]
related: [spec 0009, spec 0010, spec 0006]
---

# Spec: Import tab in settings

Pins an "Import" tab on the settings screen that imports the SSH hosts of
an installed MobaXterm or Tabby in one click, with spec 0010's parsers,
duplicate rules and in-place write. It takes up spec 0010's *Later* item
"an in-app import".

## Problem & goal

`tern --import-hosts <file>` works, but it asks a switching user to know
where MobaXterm or Tabby keeps its sessions and to open a shell for a
one-off. Both apps keep them at a fixed place per user, so Tern can find
them and the import becomes one button.

Done means: on a machine with MobaXterm or Tabby installed, Settings →
Import shows each app with how many new hosts it would bring, one click
adds them to `config.yaml` as spec 0010 does, and the tab says what it
did.

## Scope

**In scope:**

- A sixth settings tab, "Import", after Profiles, with one card per source: MobaXterm, Tabby
- Finding each source's file at its default place, with a count of what an import would do
- "Import" per card: one click, no confirmation dialog (it only appends, and duplicates are never imported)
- "Choose file…" per card for a file elsewhere (portable installs, exports), via the OS file picker
- An inline result on the card: imported, duplicates, skipped by reason

**Out of scope:**

- Picking single hosts or a preview list: the CLI's `--dry-run` and editing `config.yaml` cover it; *Later* if users ask
- Other sources: PuTTY and WinSCP in spec 0015; SecureCRT, Termius *Later*, one spec each
- Passwords, vaults, jump hosts, non-SSH sessions: never, as spec 0010
- Watching the source files or re-importing on change: never; the user starts every import

## Design

### Affected areas

- `src/settings/` (the `Page::Import` tab and its cards), `src/import/` (a
  library entry beside `run` that returns the plan and a structured
  report instead of printing), `src/layout/` only for the write, which goes
  through the same `config::save_edit` path

### Model

- **Default places:** both are under `dirs::config_dir()` (no new crate).
  MobaXterm: `MobaXterm\MobaXterm.ini`, Windows only (the installed
  edition; a portable one keeps its INI next to its exe and goes through
  "Choose file…"). Tabby: `tabby/config.yaml` on every platform
  (`%APPDATA%`, `~/.config`, `~/Library/Application Support`).
- **One source of truth [load-bearing]:** reading, format detection,
  duplicate rules, IDs and the in-place append are spec 0010's code. The
  tab adds no parsing and no second write path; a CLI import and a click
  on the same file give the same `config.yaml`.
- **A card's state:** not installed (no file at the default place), found
  with a count (N new, M duplicates, K skipped), up to date (0 new),
  unreadable (the error, one line), or done (the result of the last
  import). The chosen file replaces the default one for that card until
  the screen closes; it isn't remembered.
- **Format per card:** a file chosen on the MobaXterm card that turns out
  to be a Tabby config (or the reverse) is imported as what it is, and the
  card says so; detection is by content, as in spec 0010.

### Flows

1. **Open the tab:** the cards show "Looking…" and both default files are
   read and planned on the background executor [load-bearing: never on
   the UI thread]; each card fills in when its file is done.
2. **Not installed:** the card says "Not found at <path>" and offers
   "Choose file…" only. On Linux the MobaXterm card says MobaXterm runs on
   Windows and offers "Choose file…" for an exported `.mxtsessions`.
3. **Found:** "N new hosts in M groups" with an "Import" button, plus the
   duplicate and skipped counts. With 0 new, the button is disabled and
   the card says "Everything is already in Tern".
4. **Import:** the click re-reads the file and re-plans against the
   config as it is now (`config::save_edit`, read right before writing),
   so an edit since the count can't produce duplicates. Success shows
   "Imported N hosts" on the card with the duplicates and skipped counts;
   the config watcher reloads and the hosts are in the host picker and on
   the Profiles tab. The card then shows the up-to-date state.
5. **Choose file…:** the OS file picker (XDG portal on Linux); the pick
   runs flow 1 for that card. Cancelling changes nothing.
6. **Failure:** unreadable, over 10 MB, not a known format, or a write
   refused by `save_edit` (flow style `profiles:`, file keeps changing):
   the card shows the error in one line and `config.yaml` is unchanged.
   A config with notices doesn't block the count, only a write that
   wouldn't read back (spec 0010, `Plan::apply`).
7. **Search:** the tab's cards are found by "import", "mobaxterm",
   "tabby", "hosts", like any other row (spec 0009, flow 6).

### Startup impact

None: the tab and its file reads exist only once the settings screen is
open on it. Nothing is read at boot.

### Platform notes

- Windows 11: both cards can find a file; MobaXterm's CP1252 and Windows
  key paths are handled as in spec 0010.
- Linux (Wayland): the Tabby card can find a file; the MobaXterm card is
  "Choose file…" only. The file picker is the XDG portal, as the profile
  form's "Browse…" already uses (`cx.prompt_for_paths`).
- macOS: compiles; the Tabby path resolves under Application Support.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| Hosts, users, key paths, group names | Read from the source file; written to `config.yaml`; shown on the card as counts | The import the user clicks | The user's own device; no one else processes it | As any profile (spec 0005) |

Nothing new is stored: the chosen path lives only in the open screen.
The source file is untrusted input, as in spec 0010: every value passes
the config's checks, nothing in it is run, and passwords, MobaXterm
credentials and Tabby's vault are never written, shown or sent to the
keychain [load-bearing]. Reading the default places when the tab opens
is reading the user's own files on their own action; nothing leaves the
machine.

## Acceptance criteria

1. On Windows with `%APPDATA%\MobaXterm\MobaXterm.ini` holding SSH sessions in folders, the Import tab shows the MobaXterm card with the right count of new hosts without any click, and one click on "Import" adds them to `config.yaml` grouped as `Folder / Subfolder`.
2. With `<config_dir>/tabby/config.yaml` present (Windows and Linux), the Tabby card counts and imports its SSH profiles; its local and serial profiles are counted as skipped.
3. Clicking "Import" a second time imports nothing: the card shows 0 new and the button is disabled, and `config.yaml` is byte for byte unchanged.
4. A file with saved passwords or a Tabby vault leaves no secret in `config.yaml`, on the screen, or in the keychain.
5. With neither app installed, both cards say "Not found at <path>" and "Choose file…" picks an `.mxtsessions` export that then imports like the default file.
6. A Tabby `config.yaml` chosen on the MobaXterm card imports as Tabby and the card says so.
7. A malformed or over-10 MB file shows a one-line error on its card and leaves `config.yaml` unchanged.
8. Importing the same file from the tab and with `tern --import-hosts` into two copies of the same config gives byte-identical results.
9. Opening the Import tab on a 10 MB source file keeps the screen responsive (the read and plan happen off the UI thread), and a normal launch's TTFP is unchanged.

## Open questions

- None.
