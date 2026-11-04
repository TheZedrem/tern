# AGENTS.md — zed-term

Humans prompt, guide and review; agents write most of the code. This repo
works **spec-first**: nothing gets built that isn't pinned in `spec/` and
represented in `tasks/`. Read this file fully before your first task.

## What zed-term is

A desktop terminal emulator with built-in SSH/SFTP and one-click AI coding
agents. Think Tabby's feature set with the boot time of a native terminal.
It's pure Rust on GPUI, Zed's GPU UI framework, with `gpui-component` for
widgets and `alacritty_terminal` for the terminal itself, for developers
who open a terminal fifty times a day and notice every 100 ms.

## What we can never compromise on

- **A prompt in under 500 ms.** Slow boot is why this product exists. A
  GPUI spike measured 333 ms warm; a webview version measured 1.07 s and
  was dropped (OQ-010, OQ-013). Every feature that isn't the first frame initializes
  after it. The boot path is defined in
  [spec 0001](spec/0001-startup-performance.md), and adding anything to it
  requires a TTFP measurement in the commit.
- **The shell owns the keyboard.** Plain `Ctrl+<letter>` / `Alt+<letter>`
  belong to readline, vim and emacs. App shortcuts use `Ctrl+Shift` (`Cmd` on
  macOS) by default. A terminal that eats `Ctrl+R` is broken.
- **Nothing leaves the machine unless the user starts it.** No telemetry, no
  network at startup, no CDNs. Secrets go only in the OS keychain.
  Scrollback is never written to disk.
- **Modern platforms only, and every one of them works.** Linux means
  Wayland and Windows means Windows 11. X11, XWayland and Windows 10 are not
  supported, and we take no dependency on deprecated or end-of-life
  technology. A feature that only works on the dev's own OS isn't done.
  macOS only needs to compile.

## Owner's note

Simple beats clever: the simplest design that meets the budget wins, and a
design that needs a paragraph to justify itself is the wrong one. YAGNI
unless a spec says otherwise. Types do the heavy lifting: enums and action
types over stringly-typed IDs, inferred types over annotations. Tests are
focused on behavior, not smoke for its own sake. Comments explain *why* at
function and module level, and stay in sync with the code. These are good
defaults. If one fights the task, say so and ask.

## The ways to hurt this project (worst first)

1. **Work before the first frame.** Anything `main.rs` does before the
   window's first frame is on every launch. Watchers, theme detection,
   network, extra font loading: all after the first frame. The dev badge
   shows the TTFP of every run; watch it.
2. **Blocking the UI thread.** GPUI's foreground thread runs every view
   update and render. File IO, PTY writes and anything that can wait go to
   the background executor or a thread; a render only locks a pane's grid
   long enough to snapshot the visible rows.
3. **Killing the wrong process.** Never `pkill zed-term` / `pkill -f`. The
   owner may be daily-driving a zed-term build, and that kills every shell
   they have open. Kill only a PID you captured at spawn.
4. **Logging terminal bytes.** PTY output carries passwords, tokens and
   private data. Never log it, even at debug level, and never persist it.
5. **Dropping GPUI's `wayland` feature.** `gpui-pre-platform` builds fine
   without it and then runs headless: no window, no error. The feature list
   in `Cargo.toml` is load-bearing.
6. **Per-frame work that scales with scrollback.** Only visible rows are
   shaped; never iterate the whole grid or re-shape unchanged rows.
7. **Bumping GPUI or its widgets in passing.** `gpui-pre` and
   `gpui-component` are exact-pinned and move together; the API churns
   weekly. A bump is its own commit with a TTFP measurement (architecture
   §Key decisions).
8. **ConPTY is not a Unix PTY.** Windows rewrites output and redraws on
   resize. Resize, cursor and exit behavior must be checked on Windows, not
   inferred from Linux.

## Operations

| Command | What it does |
|---|---|
| `cargo run` | Run the app (debug). The TTFP badge is shown in a corner |
| `cargo test` | Unit tests |
| `cargo fmt && cargo clippy --all-targets -- -D warnings` | Must be clean before a commit |
| `cargo build --release` | Release binary at `target/release/zed-term` |
| `target/release/zed-term --bench-startup 10` | Cold run plus median/p90 TTFP over 10 warm launches (spec 0001) |

Verify UI changes in the running app (`cargo run`), not just in tests.
Report the TTFP the badge shows whenever you touched the boot path.

## Architecture in one paragraph

One Rust binary. `src/main.rs` is the boot path: check for Wayland, read
config and session, spawn the active tab's PTYs, start GPUI, open the
window. `alacritty_terminal` owns each pane's PTY and VT state on its own
thread; GPUI views draw the grid, tabs, splits and palette, and redraw only
when notified. The stack and every decision live in
[`spec/architecture.md`](spec/architecture.md). Don't restate them
elsewhere; link.

| Path | What it is |
|---|---|
| `spec/` | **Forward-looking**: architecture, feature specs (`NNNN-*.md`), `open-questions.md` |
| `tasks/` | The backlog: one file per task, resolved files move to `tasks/resolved/` (format: `tasks/README.md`) |
| `docs/` | **Current state**: dev setup, conventions. Links to `spec/`, never restates decisions |
| `.claude/skills/` | Project skills (see below) |
| `src/` | The crate. `main.rs` is the boot path; modules per `spec/architecture.md` §Code layout |
| `spikes/` | Throwaway prototypes, only on `spike/*` branches; `main` keeps the verdicts |

## Loop protocol

One task per loop. Never start work that isn't in `tasks/`.

1. **Pick a task:** an unblocked `status: now` task, unless the human named one.
2. **Read** the specs it links, plus `spec/architecture.md` for anything touching the stack. A linked spec still in `status: draft` needs the owner's approval first (see Specs).
3. **Plan** briefly in the conversation. Architecture-relevant deviations stop here (see decision rules).
4. **Implement** directly on `main` (personal project: no branches or PRs).
5. **Test.** Run the tests, fmt and clippy, and verify in the running app.
   Per the owner's rule, tests for new behavior are written by a separate
   agent with fresh context against the spec's acceptance criteria, so they
   test the agreed behavior rather than mirroring the implementation.
6. **Update the backlog.** Move the task to `tasks/resolved/`, file
   follow-ups as new tasks, and tick resolved OQs. When every task in a
   spec's `tasks:` list is resolved, set that spec to `status: implemented`.
7. **Summarize for review:** what changed, how it was verified, the TTFP if the boot path was touched, and what's still open.

## Specs

One spec per feature at `spec/NNNN-<name>.md`, written with the `write-spec`
skill, which owns the template. The frontmatter is what you act on:

- **`status`:** `draft` (being written, don't implement), `approved` (the
  owner signed off, buildable), `implemented` (all of its tasks are resolved).
  Only the owner moves a spec from `draft` to `approved`.
- **`phase`:** the roadmap phase in `spec/architecture.md`. Only the current
  phase gets specs.
- **`tasks`:** the backlog items that build it. Every task links back with a
  `Spec:` line, so the two always point at each other.
- **`related`:** other specs and OQ IDs to read alongside it.

Every spec fills in **Startup impact**, **Platform notes** and **Security &
privacy**, and ends with 6–10 acceptance criteria. Those criteria are what
the separate test agent tests against (loop step 5). If the implementation
has to diverge from an approved spec, update the spec in the same change
and say so in the summary.

## Decision rules

- **Architecture changes are never silent.** If a task pushes against
  `spec/architecture.md`, stop and ask. Once agreed, update that file in the
  same change.
- **Unknowns go to `spec/open-questions.md`** (next free OQ-nnn). Pick the
  most conservative choice that doesn't paint us into a corner, leave
  `// TODO(OQ-nnn)` at the site, and keep moving.
- **Stop and ask** before: new crates, anything added to the boot path,
  outbound network calls, writing outside the app's config/data dirs, and
  history rewrites. A crate that exists to support a deprecated platform
  is a no without asking.
- Bugs you find but don't fix: file a `type: bug` task.

## Ground rules (binding)

The privacy and data rules are in
[`spec/architecture.md` §Ground rules](spec/architecture.md#ground-rules-binding),
the trust boundaries in [§Security](spec/architecture.md#security-binding).
Each spec's **Security & privacy** section says which of them it touches.

## Definition of Done

- Implemented per the spec on Linux (Wayland), with Windows 11 behavior considered (and checked when the task touches PTY, keys or paths).
- Tests written and passing; `cargo fmt`, `clippy` and `cargo test` green.
- TTFP reported if the boot path was touched.
- Docs and specs updated where reality diverged, and the backlog updated.

## Skills

| Skill | Use when |
|---|---|
| `write-spec` (`.claude/skills/write-spec/`) | Writing or revising a feature spec. Load it before writing; it owns the spec template |

## Conventions

Summary; full text in [`docs/conventions.md`](docs/conventions.md):
Conventional Commits straight to `main`; cargo with a committed lockfile;
`cargo fmt` and `clippy` clean; no `unwrap` outside tests and the boot
path's "cannot continue" cases.
