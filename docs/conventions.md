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

## File size and ownership

A file should be small enough to read whole, because agents read whole
files and every extra thousand lines costs every later task.

- **About 800 lines of non-test code is the ceiling.** A change that
  pushes a file past it splits the file in the same change, or in a
  `refactor` commit right before it. Tests in `#[cfg(test)]` don't count.
- **Split by responsibility, not by size.** A file holds one area
  (closing, session saving, drag and drop), never `misc.rs` or `part2.rs`.
- **One type's `impl` can span files.** A child module sees its parent's
  private fields, so a view's methods for one feature go in
  `<module>/<feature>.rs` as their own `impl` block. Nothing becomes `pub`
  just to move.
- **A new feature gets its own module, not new methods on a host view.**
  `Workspace`, `Browser`, `TerminalPane` and `SettingsView` wire features
  together; the feature's state and logic live in its own module, ideally
  as its own GPUI entity that the host owns and renders.
- **Tests stay next to the code they test** and move with it.

## CI

GitHub Actions on Linux and Windows runners (`.github/workflows/ci.yml`):
fmt, clippy, tests and a release build on every push to `main`, every pull
request and as the first stage of a `v*` tag's release, where
`release.yml` builds packages only once the checks pass. Actions are
pinned to commit SHAs. CI doesn't replace the local checks before a
commit; it catches what only fails on the other OS.

## Releases

Before tagging a release, run `--bench-startup 10` on the release build
and add the numbers to [`ttfp.md`](ttfp.md), so drift across releases is
visible and not just per commit.
