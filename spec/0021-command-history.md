---
status: approved       # draft | approved | implemented
phase: 3
tasks: [T-105, T-106, T-107, T-108]
related: [spec 0001, spec 0002, spec 0004, spec 0009, OQ-008, OQ-041, threat model 0002]
---

# Spec: Command history suggestions

Tern learns the commands the user runs in local shells and, as they type
at a prompt, shows the newest past command that starts with what they've
typed as grey text after the cursor. Right arrow accepts it. This spec
pins down how commands are detected (through the opt-in shell-integration
snippets from spec 0002), what gets stored and where, when a suggestion
shows, and how accepting it stays out of the shell's way. It amends the
snippets and ground rule 5 (architecture §Ground rules) with the owner's
decisions of 2026-11-24: history goes in a file, local panes only, Right
accepts.

## Problem & goal

Retyping the same `git`, `cargo` and `kubectl` lines is the most common
thing a terminal user does. Windows Terminal, Warp and fish suggest past
commands inline, while bash and cmd-era PowerShell setups don't. Tern
should suggest them in every local shell the user wires up, without taking
any key the shell uses and without storing what the user didn't mean to keep.

Done means: with the snippet sourced, typing the start of a past command
in bash, zsh or PowerShell 7 shows the rest in grey, Right inserts it,
and the history survives a restart unless the user turned that off.

## Scope

**In scope:**

- Detecting finished commands in local panes through the bash, zsh and
  pwsh snippets.
- One history per shell kind, kept in a file (or only in memory, or off).
- An inline grey suggestion at the prompt, accepted with Right.
- The `command_history` setting and a **Clear command history** command.

**Out of scope:**

- SSH panes, and SSH sessions started inside a local pane: *Later* (the
  remote host is untrusted and needs its own history per host).
- Accepting one word at a time, a list of several suggestions, or fuzzy
  matching: *Later*.
- Importing the shell's own history files (`~/.bash_history`,
  PSReadLine's): *Later*.
- fish, cmd.exe and nushell snippets: *Later*. fish already suggests on
  its own.

## Design

### Affected areas

- `shell-integration/tern.{bash,zsh,ps1}`: two new reports.
- `src/terminal/osc7.rs`, which becomes the scanner for every
  shell-integration report (renamed to `marks.rs`).
- New `src/history/`: the store, matching and the file.
- `src/terminal/suggest.rs`: drawing and accepting the suggestion.
- Config `command_history`, the settings screen (spec 0009), and the
  `accept_suggestion` and `clear_command_history` commands (spec 0004).

### Model

- **Entry:** shell kind (`bash`, `zsh` or `pwsh`, as the snippet reports
  it) and the command text. Nothing else is stored: no time, cwd, exit
  code or pane.
- **History:** for each shell kind, a list of distinct commands, newest
  first, capped at 5 000. Running a command again moves it to the front.
- **Prompt state, per pane and in memory:** the grid point where the
  current prompt's input starts, or none while a command runs.

### Reports from the snippets

Each snippet sends two reports, as one Tern-private OSC sequence. The
number and encoding are implementation details shared by Tern and the
snippets, which ship together.

- **Input start**, at the very end of each prompt, so the input begins
  right after it.
- **Last command**, at each prompt: the shell's newest history entry, its
  history number and the shell kind, percent-encoded like OSC 7. Asking
  the shell's own history means its opt-outs apply (`HISTCONTROL`,
  `HIST_IGNORE_SPACE`). The history number lets Tern skip a repeat when
  the user only pressed Enter.

Rules:

- **Token [load-bearing]:** Tern puts a random token (one per app run)
  into every local pane's environment as `TERN_HISTORY_TOKEN`. It comes
  from std's `RandomState`, which std already seeds from the OS when boot
  builds its first `HashMap`. That way the token costs no new syscall or
  IO, and panes spawned at boot get it too (owner's decision,
  2026-11-24).
  On Windows it's also added to `WSLENV`, so WSL shells receive it. Each
  report carries the token, and a report without the matching token is
  dropped. That way a `cat`ed file, a git log or a remote host can't plant
  a command or move the input start. SSH profile panes get no token, so
  everything they report is dropped.
- **Snippets stay silent without the token.** In another terminal or on
  a remote host they send only OSC 7, as they do today.
- **Never stored [load-bearing]:** commands that start with whitespace;
  span more than one line; are longer than 4 KiB; or contain, ignoring
  case, `password`, `passwd`, `secret`, `token`, `apikey` or `api_key`.
  This is PSReadLine's own rule for what it keeps out of its history file.
  Such a command never enters the history, in memory or on disk.
- **Tokens are detected [load-bearing]:** a command is also never stored
  when one of a small, fixed set of patterns finds a credential in it,
  wherever it sits in the line:
  - a JWT: three base64url parts joined by dots, the first starting
    with `eyJ`;
  - an `Authorization:` header or a `Bearer` or `Basic` value;
  - a known token prefix (GitHub `ghp_`, `gho_` and `github_pat_`, GitLab
    `glpat-`, Slack `xox?-`, AWS `AKIA`, OpenAI and Anthropic `sk-`, a
    PEM `-----BEGIN`);
  - a credential in a URL (`scheme://user:pass@`);
  - a run of 32 or more hex or base64 characters as a word of its own.

  The list is a constant in `history/`, with one test per pattern. Adding
  a pattern is a code change, not a setting. It errs toward dropping, so
  a few long hashes (a git SHA given in full, for example) go unrecorded.
  The patterns use the `regex` crate, already in the lockfile through
  `alacritty_terminal` and added as a direct dependency with the owner's
  OK, given with this spec's approval (2026-11-24).

### Flows

1. **Recording.** The pane's scanner sees a last-command report with the
   right token. Tern drops it if its history number matches the previous
   report from that pane, or if the never-stored rules match. Otherwise it
   puts the command at the front of that shell kind's history and, with
   `save`, appends it to the file on the history thread. A failed write
   shows one notice per run and recording continues in memory.
2. **Loading.** After the first frame, the history thread starts and
   reads the file. Nothing in `history/` runs before that frame. Reports
   that arrive earlier are queued in the pane and recorded once the
   thread runs. Lines it can't read are skipped. If the file holds more than
   twice the cap, it's rewritten through a temp file and a rename. Until
   loading finishes, suggestions come only from this run. A missing file
   is the normal empty case.
3. **Suggesting.** On each render of the focused pane, a suggestion shows
   only if all of these hold:
   - an input start is known for this prompt;
   - the pane is scrolled to the bottom and not on the alternate screen;
   - there's no IME preedit and no open search bar;
   - the typed text has at least one non-space character;
   - every cell after the cursor on its row is blank. This also makes
     Tern step aside for PSReadLine predictions and zsh-autosuggestions,
     which draw their own grey text there.

   The typed text runs from the input start to the cursor. The suggestion
   is the rest of the newest entry of the pane's last-reported shell kind
   that starts with it (exact, case-sensitive). The grey text is clipped
   at the row's right edge. Matching reads only the input's rows, never
   the scrollback.
4. **Accepting.** With a suggestion showing, `accept_suggestion` (default
   `right`) writes its remaining characters to the PTY as typed text, not
   as a bracketed paste. Control characters are dropped. **[load-bearing]**
   Accepting never sends Enter, and when no suggestion shows, Right goes
   to the shell unchanged.
5. **Leaving the prompt.** Enter, `Ctrl+C` or a new prompt's report
   clears the input start, so no suggestion shows while a command runs.
6. **Clearing.** **Clear command history** (palette and settings) empties
   every shell kind's history in memory and deletes the file. Setting
   `command_history` to `off` or `memory` deletes the file as well.

### Startup impact

Startup is untouched **[load-bearing]**. Loading the file, matching,
recording and drawing suggestions all start after the first frame. Until
the file is loaded, suggestions come only from commands run in this
session. The only thing at spawn is the token's environment variable,
built from std's already-seeded hasher with no syscall or IO. The
implementing commit still records a TTFP measurement showing the median
is unchanged (spec 0001), because it touches the spawn code. The snippets add one built-in history
call and an encoding loop to each prompt in the shell, with no fork.

### Platform notes

- **Windows:** pwsh reads the last command from `Get-History` and adds
  its report to the prompt function the snippet already wraps. ConPTY may
  repaint the prompt line after a resize. The input start is a grid
  point, so the check whether the cells after the cursor are blank (flow
  3) must be confirmed on Windows, not assumed. WSL shells get the token
  through `WSLENV`. Their entries count as `bash` or `zsh`, the same as a
  native shell of that kind.
- **Linux:** bash takes the last entry from `history 1` in
  `PROMPT_COMMAND`. zsh uses `precmd` and `fc`. The input-start report is
  added to the prompt in each shell's non-printing brackets, so line
  editing doesn't miscount the prompt's width.
- **History file:** `<data_local_dir>/tern/history.jsonl`, one JSON
  object per line. On Linux it's created with mode `0600`. On Windows it
  sits in `%LOCALAPPDATA%`, which doesn't roam.
- macOS only needs to compile.

### Security & privacy

- **Stored:** the commands that pass flow 1, with their shell kind, in
  `history.jsonl`, until the user clears them or turns history off.
  Never scrollback or output **[load-bearing]**, and never logged. This
  amends ground rule 5 and the data stores table in architecture.md.
- **Trust boundary:** this crosses §Security 1, because the reports are
  escape sequences. The token keeps output from planting entries or moving
  the input start (flow 1). A program the user runs inherits the token
  and could still forge entries, but it already runs with the user's
  rights. A suggestion is drawn as plain text, inserted only on the
  user's key and never run (flow 4), and nothing ever executes a stored
  command. The amendment to threat model 0002 (T-105) records this as new
  `TM-escape` entries.
- **Secrets:** commands can contain secrets. The never-stored rules,
  the token patterns, the shell's own opt-outs, `command_history: memory | off` and **Clear
  command history** are the defenses. The settings row says the file is
  plain text.
- **Network:** none.

## Acceptance criteria

1. With the bash snippet on Linux and the pwsh snippet on Windows 11,
   run `git status`, then type `git s`. Grey `tatus` shows after the
   cursor. Right inserts it without running it, and Enter runs it.
2. With no suggestion showing, Right moves the cursor in readline after
   a `Left`, and in vim. Tab still completes in bash.
3. Neither of these changes the suggestions or `history.jsonl`: `cat` of
   a file holding a forged last-command report, or a command run on a
   remote host through `ssh` inside a local pane.
4. None of these is ever suggested or appears in `history.jsonl`:
   ` echo hi` (leading space), `mysql --password=hunter2`, a two-line
   command, `curl -H "Authorization: Bearer abc" …`, `export
   GH=ghp_<36 chars>`, and a command holding a JWT
   (`eyJhbGciOi….eyJzdWIi….sig`). `git show <full 40-char SHA>` isn't
   recorded either (the expected cost of erring toward dropping).
5. After a restart, a command from the previous run is suggested with
   `command_history: save`. With `memory` it isn't, and no file exists.
   With `off`, nothing is suggested or recorded.
6. **Clear command history** removes every suggestion in every open pane
   and deletes `history.jsonl`.
7. A command run in pwsh isn't suggested in a bash pane, and the reverse.
8. With PSReadLine's inline prediction or zsh-autosuggestions on, Tern
   draws no second grey text over the shell's own.
9. A `history.jsonl` of 20 000 lines with one corrupt line leaves the
   warm TTFP median within noise of a run without the file. Suggestions
   appear once it's loaded, and nothing crashes.
10. No suggestion shows inside vim or `less` (alternate screen), in an SSH
    profile pane, or while scrolled up in the scrollback.

## Open questions

- OQ-041: how the scanner, which runs before `alacritty_terminal` parses
  the bytes, pins the grid point of the input-start report.
