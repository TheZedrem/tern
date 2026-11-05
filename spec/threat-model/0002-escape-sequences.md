---
status: draft
date: 2026-11-05
author: Claude (T-021), for review by the owner
related: [spec 0002, spec 0003, architecture §Security 1 and 2, T-002, OQ-017]
---

# Threat model: escape sequences from programs and remote hosts

## 1. Component overview

What terminal output can make zed-term do. Anything that prints into a
pane is untrusted: a local program, a file someone `cat`s, a git log with
crafted messages, and every remote host behind `ssh`, jump hosts included.
`alacritty_terminal` (vte 0.15) parses the bytes into the grid and raises
events; zed-term decides what each one may do. T-002 builds most of what
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

vte 0.15 has no OSC 7 handler, so cwd reports need a scanner of zed-term's
own on the PTY bytes (T-002); spec 0002 is corrected to say so. The
replies alacritty_terminal writes back are fixed strings and numbers: no
title report (CSI 21 t) and no setting reports that echo text.

## 2. Data classification

| Data | Category (GDPR) | Special constraints | Storage | Retention |
|---|---|---|---|---|
| Terminal output: titles, link targets, cwd reports | Art. 4(1) where it names people, hosts or paths; untrusted | Plain text only; never logged (architecture §Ground rules) | Memory; title and cwd per pane in `session.json` (spec 0003) | Pane lifetime; session overwritten on save |
| Clipboard and primary selection | Art. 4(1), may hold secrets (copied passwords, tokens) | Never sent to a program by an escape sequence | The OS's | The OS's |
| Pasted text | As the clipboard | Reaches the program only as the user's paste | Not stored | Not stored |

No sector law applies, and nothing leaves the device except what the
user's own programs send.

## 3. STRIDE walkthrough

Elements: output → parser (TB1), parser → pane actions (TB2), pane → OS
handler and clipboard (TB3), pane → program input (TB4).

| ID | STRIDE | Threat | Affected element | Impact | Likelihood | Mitigation | Status |
|---|---|---|---|---|---|---|---|
| TM-escape-01 | Spoofing | A title (OSC 0/2) poses as another host or as a system prompt in the tab bar, so the user types a secret into the wrong tab. | TB2 title | M | L | Titles are plain text on one line, capped at 200 characters, with control, invisible and direction characters stripped (spec 0003), saved and restored titles too. | mitigated (T-003, T-006) |
| TM-escape-02 | Spoofing | An OSC 8 link's text names one site while it points at another (phishing). | TB2 → TB3 link | H | M | A link opens only on Ctrl/Cmd+click, and hovering shows its real target as plain text; a host with non-ASCII letters is marked as such, since look-alike letters can't be told apart (spec 0002). | open (T-002) |
| TM-escape-03 | Elevation of privilege | A link with another scheme (`file:`, `javascript:`, `ms-msdt:`, an app's own handler like `vscode:`) runs code through the OS handler. | TB3 URL handler | H | M | Only `http` and `https` targets, checked case-insensitively on the parsed URL, are links at all; nothing else reaches the OS handler [load-bearing]. They open through GPUI's `open_url`, never a shell. | open (T-002) |
| TM-escape-04 | Tampering | Output writes the clipboard (OSC 52) with a command the user pastes later. | TB2 → TB3 clipboard | H | M | `ClipboardStore` is ignored: clipboard writes from the terminal are off (architecture §Security 1, spec 0002). | mitigated (T-010); T-002 pins it with a test |
| TM-escape-05 | Information disclosure | Output asks for the clipboard (OSC 52 `?`), and the reply leaks what the user copied to the program or remote host. | TB3 → TB4 | H | M | `ClipboardLoad` is never answered [load-bearing]. | mitigated (T-010); T-002 pins it with a test |
| TM-escape-06 | Tampering | A query's reply carries text the program chose (a title report), and the shell runs it as typed input. | TB4 replies | H | L | Replies are fixed strings and numbers only: device attributes, status, cursor position, modes, sizes and the theme's colors as `rgb:` values. zed-term adds no reply that echoes terminal text. | mitigated (T-010); T-002 pins it with a test |
| TM-escape-07 | Tampering | Pasted text holds `ESC [201~`, ending bracketed paste early, so the rest runs as typed commands (pastejacking from a web page). | TB4 paste | H | M | Paste drops ESC and every other control character but tab, `\n` and `\r`, then wraps the text in `ESC [200~ … ESC [201~` when the program asked for bracketed paste (spec 0002). | open (T-002) |
| TM-escape-08 | Tampering | Without bracketed paste, each pasted line runs as it arrives. | TB4 paste | M | L | Current bash, zsh, fish and PowerShell turn bracketed paste on; without it zed-term pastes like xterm. A multi-line paste warning is *Later* (spec 0002). | accepted pending OQ-017 |
| TM-escape-09 | Spoofing | A program or remote host reports a cwd (OSC 7) so that new local panes start where it chose. | TB2 cwd | M | L | OSC 7 counts only in a local pane, as a `file://` URL whose host is empty, `localhost` or this machine's name, naming an existing local directory; an SSH pane's reports set no local cwd (spec 0002). | open (T-002) |
| TM-escape-10 | Denial of service | Huge OSC strings (titles, link targets, cwd reports), floods of links or title changes eat memory or CPU. | TB1, TB2 | M | L | Titles capped at 200 characters; link targets used only up to 2 KiB and cwd reports up to 4 KiB, longer ones ignored; only visible rows are drawn; title changes redraw once per frame and save debounced (spec 0003). | open (T-002) |
| TM-escape-11 | Information disclosure | Color, size and device queries tell the program or remote host the theme's colors, the window size and the terminal type. | TB4 replies | L | H | Needed by vim, tmux and others, and no personal data. | accepted pending OQ-017 |
| TM-escape-12 | Tampering | Copied text holds what the screen didn't show (zero-width or direction-override characters, text in the background color), so a pasted command does more than it looked. | TB3 copy | M | L | Copy drops zero-width and direction-override characters. Text drawn in the background color is copied as is: the terminal can't tell intent. | open (T-002); the rest accepted pending OQ-017 |
| TM-escape-13 | Elevation of privilege | The app runs a path or command named in output (a "link" to a script, a cwd that triggers code). | TB3 | H | L | Nothing from output is ever executed; only `http`/`https` links open (architecture §Security 2). | mitigated (by design) |

Repudiation has no row: zed-term keeps no record of terminal output, by
design (PTY bytes are never logged), so there is no trail to falsify, and
nothing here acts in anyone's name.

## 4. Mitigation tasks

- [ ] T-002 — builds TM-escape-02, -03, -07, -09, -10 and -12 as spec 0002
  now states them, and pins -04, -05 and -06 with tests

## 5. Accepted risks

Pending the owner's sign-off in OQ-017: TM-escape-08 (multi-line paste
without bracketed paste), TM-escape-11 (query replies), and the rest of
TM-escape-12 (text in the background color). Until then T-002 builds them
the way other terminals do, which is the conservative choice: refusing
them would break vim, tmux and plain pasting.
