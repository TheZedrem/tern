# Conventions

Repo-wide working rules. Architecture decisions are **not** here; see
[`spec/architecture.md`](../spec/architecture.md).

## Commits: Conventional Commits (mandatory)

```
<type>(<scope>): <description>

feat(pane): add search bar
fix(pty): kill children on quit
perf(boot): spawn shell before the window opens
docs, chore, refactor, test, perf, ci, build
```

- Imperative mood, lowercase description, no trailing period.
- Scope = feature or module (`boot`, `pty`, `pane`, `layout`, `palette`,
  `config`, `theme`, `spec`), not a filename.
- Add a body only for what the diff can't show (the why, an invisible
  constraint). When a commit changes the boot path, put the TTFP
  before/after in its body.

## Workflow

Personal project: work is committed straight to `main`, with no branches
or PRs. One task = one or a few focused commits. Throwaway prototypes live
on `spike/*` branches (see the `prototype` skill); `main` keeps only the
verdict.

## Versions and dependencies

- Start new dependencies at current stable. Adding any crate is a
  stop-and-ask (see `AGENTS.md`), because every crate costs compile time and
  can sneak onto the boot path.
- cargo with a committed `Cargo.lock`.

## Rust

- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` clean.
- Modules are plain Rust that is testable without a window; GPUI views are
  a thin layer that reads state and dispatches actions.
- No `unwrap`/`expect` outside tests, except on the boot path where the
  alternative is "cannot run at all" (e.g. no window). Everything else
  returns `Result<T, String>` with a message a user can read.
- The UI thread never blocks: file IO and PTY writes go to threads or the
  pane's event loop.
- Types over string IDs. A command is a GPUI action type; a theme name we
  define and a profile type are enums. serde maps them to the strings in
  `config.yaml`.

## CI

GitHub Actions on Linux and Windows runners (`.github/workflows/ci.yml`),
run only as the first stage of a `v*` tag's release: `release.yml` builds
packages once the checks pass. Commits to `main` don't run CI, so fmt,
clippy and tests run locally before every commit.
