---
status: approved       # draft | approved | implemented
phase: 3
tasks: [T-109, T-110]
related: [spec 0002, spec 0005, spec 0009, spec 0016, spec 0021, OQ-042]
---

# Spec: Offer the shell snippet in WSL

When a local pane on Windows runs a WSL shell that doesn't load Tern's
shell-integration snippet, Tern offers to add it. This spec pins down
how a WSL session is noticed, what the offer does when accepted, and how
a "no" is remembered (owner's request, 2026-11-24).

## Problem & goal

Command history suggestions (spec 0021) and cwd tracking (spec 0002) need
the snippet sourced from the shell's rc file. Nothing sets that up, and
inside WSL the line is awkward: the snippet lives on the Windows side
under `/mnt/c/…`. A user who types `wsl` sees no suggestions and no hint
why.

Done means: opening WSL in a pane without the snippet shows one bar that
adds it on a click, and saying no means Tern never asks again.

## Scope

**In scope:**

- Noticing a WSL session in a local pane on Windows, started by a
  profile or by typing `wsl` in another shell.
- A bar on that pane with **Add**, **Not now** and **Never ask**.
- Adding the source line for bash and zsh, through the pane's own shell.
- The `snippet_offer` setting that remembers **Never ask**.

**Out of scope:**

- Offering the snippet for native PowerShell, or for bash and zsh on
  Linux: *Later*. The bar and the setting are built so that these only
  add a way to notice the shell and a line to type.
- SSH panes: never. Tern doesn't change files on a remote host.
- fish and other shells inside WSL: *Later*, with their snippets
  (spec 0021).
- Remembering the answer per distro: *Later*.

## Design

### Affected areas

- New `src/terminal/offer.rs`: noticing WSL and the bar's state.
- `src/terminal/marks.rs`: already tells which shell kind reported a
  prompt.
- Config `snippet_offer`, `src/default-config.yaml`, the settings screen
  (spec 0009).
- `windows-sys`: one more feature for the process list. No new crate.

### Model

- **Offer state, per pane and in memory:** none, noticed (WSL runs,
  waiting to see whether the snippet reports), shown, or answered.
- **`snippet_offer`:** `ask` (the default) or `never`.
- **Snippet path:** `shell-integration\tern.bash` or `tern.zsh` next to
  `tern.exe`, where the MSI puts them. A build without that folder (a
  `cargo run` from a clone uses the clone's `shell-integration/`) and no
  snippet found means no offer.

### Flows

1. **Noticing.** Only on Windows, in a local pane, with
   `snippet_offer: ask`, and at most once per pane. A WSL session is
   noticed when either holds (looked at when the pane's program starts
   and on each title change, until WSL is found):
   - the pane's profile runs `wsl.exe` (or `wsl`). `bash.exe` doesn't
     count: that is Git Bash far more often;
   - the pane's title changes and its program's process tree then holds
     a `wsl.exe`. The process list is read once per title change, on a
     background thread, never on a timer (OQ-042).
2. **Waiting for the snippet.** After noticing, Tern waits two seconds.
   If a prompt report of kind bash or zsh arrived in that pane by then,
   the snippet is loaded and nothing shows. **[load-bearing]** The offer
   never shows in a pane whose WSL shell already reports.
3. **Offering.** A bar at the pane's bottom, like the plugin Allow bar
   (spec 0016), says "Add Tern's shell integration to this WSL shell?
   It enables command suggestions." and shows the exact line Tern would
   type. Buttons: **Add**, **Not now**, **Never ask**. The bar takes no
   keys from the shell, and goes away when the pane's program exits.
4. **Add.** Tern types one line into the pane's shell and presses
   Enter. The line starts with a space, so shells that skip such lines
   keep it out of their history. It:
   - turns the snippet's Windows path into a WSL path with `wslpath`,
     so a custom automount root works;
   - picks `~/.zshrc` in zsh and `~/.bashrc` otherwise;
   - appends the source line unless the rc file already names the
     snippet;
   - sources the snippet at once, so suggestions work in this session.

   **[load-bearing]** Tern changes the rc file only this way, through
   the user's shell after a click, and shows the line first. If no
   prompt report arrives within five seconds, a notice says the shell
   integration didn't load and points to the README's section, which
   has the line to add by hand.
5. **Not now.** The bar closes. The pane isn't asked again; a new WSL
   pane is.
6. **Never ask.** Tern writes `snippet_offer: never` to `config.yaml`
   through the checked write path the settings screen uses, and no pane
   is asked again. The settings screen's row turns it back to `ask`. A
   failed write shows a notice and counts as **Not now**.
7. **The shell isn't at a prompt.** If the user clicks **Add** while a
   program other than the shell reads the keyboard (vim, a pager), the
   line lands there. The bar's text says to answer at the prompt; Tern
   can't tell without the snippet.

### Startup impact

Nothing. Noticing starts with a pane's first title change or its
profile's program name, both after the first frame for restored panes'
bars: the bar is never part of the first frame, and the process list is
read on a background thread. Linux and macOS compile none of it.

### Platform notes

- **Windows 11:** the whole feature. WSL 2 and WSL 1 both run through
  `wsl.exe`. A distro whose default prompt doesn't set the title and
  that was started by typing `wsl` goes unnoticed (OQ-042). ConPTY
  doesn't change what's typed.
- **Linux (Wayland):** no WSL, so no offer. The config key loads and is
  shown on the settings screen everywhere, so a shared `config.yaml`
  stays valid.
- macOS only needs to compile.

### Security & privacy

- **Stored:** `snippet_offer` in `config.yaml`. One line in the user's
  `~/.bashrc` or `~/.zshrc` inside WSL, written by the user's own shell
  after **Add**. This is a write outside Tern's config and data dirs,
  made only on an explicit click (owner's request, 2026-11-24).
- **Trust boundary:** Tern types into a PTY (architecture §Security 1).
  The typed line is fixed text plus the snippet's own path, single-quoted;
  nothing from terminal output or the title goes into it. The title only
  triggers the check, and a forged title can at most show the bar in a
  pane that really runs `wsl.exe`.
- **Process list:** read locally to look for `wsl.exe` under the pane's
  program. Nothing from it is kept or logged.
- **Network:** none.

## Acceptance criteria

1. On Windows 11, with no Tern line in WSL's `~/.bashrc`: typing `wsl`
   in a PowerShell pane shows the bar within a few seconds. So does
   opening a profile that runs `wsl.exe`.
2. **Add** appends exactly one source line to `~/.bashrc` (to `~/.zshrc`
   in zsh), and without restarting the shell, a command run next is
   suggested when its start is typed again.
3. Clicking **Add** in a second WSL pane, or with the line already in
   the rc file, doesn't add a second line.
4. With the snippet already sourced, opening WSL shows no bar.
5. **Never ask** sets `snippet_offer: never` in `config.yaml`. After a
   restart, no WSL pane shows the bar. Setting it to `ask` on the
   settings screen brings the offer back.
6. **Not now** closes the bar for that pane only; a new WSL pane shows
   it again.
7. No bar ever shows in an SSH pane, in a pane that never ran WSL, or
   on Linux.
8. While the bar shows, every key still reaches the shell, and the warm
   TTFP median is within noise of a build without the feature.

## Open questions

None. OQ-042 (noticing `wsl` typed into another shell) is resolved as
written in flow 1.
