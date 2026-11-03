# AGENTS.md — zed-term

Humans prompt, guide and review; agents write most of the code. This repo
works **spec-first**: nothing gets built that isn't pinned in `spec/` and
represented in `tasks/`. Read this file fully before your first task.

## What zed-term is

A desktop terminal emulator with built-in SSH/SFTP and one-click AI coding
agents. Think Tabby's feature set with the boot time of a native terminal.
It's built on Tauri 2 (Rust + system webview), `portable-pty` and xterm.js,
for developers who open a terminal fifty times a day and notice every
100 ms.

## What we can never compromise on

- **A prompt in under 1 s, with ~200 ms of it ours.** Slow boot is why this product exists; the system webview eats the rest (OQ-009). Every
  feature that isn't the first prompt is lazy-loaded or runs after idle. The
  boot path is defined in [spec 0001](spec/0001-startup-performance.md), and
  adding anything to it requires a TTFP measurement in the commit.
- **The shell owns the keyboard.** Plain `Ctrl+<letter>` / `Alt+<letter>`
  belong to readline, vim and emacs. App shortcuts use `Ctrl+Shift` (`Cmd` on
  macOS) by default. A terminal that eats `Ctrl+R` is broken.
- **Nothing leaves the machine unless the user starts it.** No telemetry, no
  network at startup, no CDNs. Secrets go only in the OS keychain.
  Scrollback is never written to disk.
- **Linux and Windows both work, always.** A feature that only works on the
  dev's own OS isn't done. macOS only needs to compile.

## Owner's note

Simple beats clever: the simplest design that meets the budget wins, and a
design that needs a paragraph to justify itself is the wrong one. YAGNI
unless a spec says otherwise. Types do the heavy lifting: no `any` in TS,
inferred types over annotations, typed IPC wrappers over string `invoke`s.
Tests are focused on behavior, not smoke for its own sake. Comments explain
*why* at function and module level, and stay in sync with the code. These
are good defaults. If one fights the task, say so and ask.

## The ways to hurt this project (worst first)

1. **A static import on the boot path.** Anything `src/main.tsx` imports,
   directly or transitively, lands in the entry chunk that every launch
   parses. Use `import()` for features. `pnpm build` enforces the 150 KB
   gzip budget. Don't raise the budget to make a build pass; ask first.
2. **Blocking the Tauri main thread.** Non-`async` Tauri commands run on the
   main thread. A blocking PTY write (a large paste into a full buffer)
   freezes the whole UI. Commands touching PTYs or files are `async` or hand
   off to a thread.
3. **Killing the wrong process.** Never `pkill zed-term` / `pkill -f`. The
   owner may be daily-driving a zed-term build, and that kills every shell
   they have open. Kill only a PID you captured at spawn.
4. **Logging terminal bytes.** PTY output carries passwords, tokens and
   private data. Never log it, even at debug level, and never persist it.
5. **Solid isn't React.** Components run once. Destructuring props or reading
   a signal outside a tracking scope silently freezes the UI. Use
   `props.x` and `splitProps`.
6. **Channels die with the page.** A dev reload (HMR) orphans every pane's
   Channel. Panes re-attach via `pty_attach`; never assume one Channel lives
   for the PTY's whole lifetime ([architecture](spec/architecture.md#key-decisions-and-their-costs)).
7. **ConPTY is not a Unix PTY.** Windows rewrites output and redraws on
   resize. Resize, cursor and exit behavior must be checked on Windows, not
   inferred from Linux.

## Operations

These commands exist once T-001 lands. Until then, `docs/development.md`
lists the system prerequisites.

| Command | What it does |
|---|---|
| `pnpm install` | Install frontend dependencies |
| `pnpm tauri dev` | Run the app with HMR. The TTFP badge is shown in a corner |
| `pnpm build` | Frontend build plus the entry-chunk budget check |
| `pnpm typecheck` | `tsc --noEmit` |
| `cargo test --manifest-path src-tauri/Cargo.toml` | Rust unit tests |
| `pnpm tauri build --no-bundle` | Release binary at `src-tauri/target/release/zed-term` |
| `pnpm bench:startup` | Median/p90 TTFP over 10 release launches (spec 0001) |

Verify UI changes in the running app (`pnpm tauri dev`), not just in tests.
Report the TTFP the badge shows whenever you touched the boot path.

## Architecture in one paragraph

Rust (`src-tauri/`) owns processes and files: PTYs, config, themes, session
state, and later SSH/keychain. The SolidJS frontend (`src/`) owns
presentation: xterm.js views, tabs, splits, palette. They talk only through
typed Tauri commands, plus one binary Channel per pane carrying PTY output.
The stack and every decision live in
[`spec/architecture.md`](spec/architecture.md). Don't
restate them elsewhere; link.

| Path | What it is |
|---|---|
| `spec/` | **Forward-looking**: architecture, feature specs (`NNNN-*.md`), `open-questions.md` |
| `tasks/` | The backlog: one file per task, resolved files move to `tasks/resolved/` (format: `tasks/README.md`) |
| `docs/` | **Current state**: dev setup, conventions. Links to `spec/`, never restates decisions |
| `.claude/skills/` | Project skills (see below) |
| `src-tauri/` | Rust backend (after T-001) |
| `src/` | SolidJS frontend. `main.tsx` is the boot path, `features/*` are lazy (after T-001) |

## Loop protocol

One task per loop. Never start work that isn't in `tasks/`.

1. **Pick a task:** an unblocked `status: now` task, unless the human named one.
2. **Read** the specs it links, plus `spec/architecture.md` for anything touching the stack. A linked spec still in `status: draft` needs the owner's approval first (see Specs).
3. **Plan** briefly in the conversation. Architecture-relevant deviations stop here (see decision rules).
4. **Implement** directly on `main` (personal project: no branches or PRs).
5. **Test.** Run the Rust tests and the typecheck, and verify in the running
   app. Per the owner's rule, tests for new behavior are written by a
   separate agent with fresh context against the spec's acceptance criteria,
   so they test the agreed behavior rather than mirroring the implementation.
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
- **Stop and ask** before: new dependencies (crate or npm), anything added
  to the boot path, new Tauri capabilities/permissions, outbound network
  calls, writing outside the app's config/data dirs, and history rewrites.
- Bugs you find but don't fix: file a `type: bug` task.

## Ground rules (binding)

The privacy and data rules are in
[`spec/architecture.md` §Ground rules](spec/architecture.md#ground-rules-binding),
the trust boundaries in [§Security](spec/architecture.md#security-binding).
Each spec's **Security & privacy** section says which of them it touches.

## Definition of Done

- Implemented per the spec, on Linux, with Windows behavior considered (and checked when the task touches PTY, keys or paths).
- Tests written and passing; `pnpm typecheck` and `pnpm build` (budget) green.
- TTFP reported if the boot path was touched.
- Docs and specs updated where reality diverged, and the backlog updated.

## Skills

| Skill | Use when |
|---|---|
| `write-spec` (`.claude/skills/write-spec/`) | Writing or revising a feature spec. Load it before writing; it owns the spec template |

## Conventions

Summary; full text in [`docs/conventions.md`](docs/conventions.md):
Conventional Commits straight to `main`; pnpm with a committed
lockfile; `cargo fmt` and `clippy` clean; no Tailwind arbitrary values.
