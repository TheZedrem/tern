# Conventions

Repo-wide working rules. Architecture decisions are **not** here; see
[`spec/architecture.md`](../spec/architecture.md).

## Commits: Conventional Commits (mandatory)

```
<type>(<scope>): <description>

feat(pane): add search bar
fix(pty): kill children on quit
perf(boot): spawn shell before webview loads
docs, chore, refactor, test, perf, ci, build
```

- Imperative mood, lowercase description, no trailing period.
- Scope = feature or module (`boot`, `pty`, `pane`, `layout`, `palette`,
  `config`, `theme`, `spec`), not a filename.
- Add a body only for what the diff can't show (the why, an invisible
  constraint).

## Workflow

Personal project: work is committed straight to `main`, with no branches
or PRs. One task = one or a few focused commits. When a commit changes the
boot path, put the TTFP before/after in its body.

## Versions and dependencies

- Start new dependencies at current stable. Adding any dependency is a
  stop-and-ask (see `AGENTS.md`), because every crate costs compile time and
  every npm package risks the boot path.
- pnpm with a committed `pnpm-lock.yaml`; cargo with a committed `Cargo.lock`.

## Rust

- `cargo fmt` and `cargo clippy -- -D warnings` clean.
- Commands are thin: parse arguments, call a plain module function, map
  errors to `String`. Logic lives in modules that are testable without a
  window.
- Commands touching PTYs or the filesystem are `async` (never block the main
  thread).

## TypeScript / SolidJS

- No `any`. Prefer inferred types; derive unions from data (command IDs from
  the registry).
- No string-literal `invoke` in components. Go through the typed wrappers in
  `src/ipc.ts`.
- Feature code under `src/features/<name>/` is only ever reached through
  `import()`.

## Tailwind

- **No arbitrary values.** Use the spacing scale (quarter steps included),
  theme tokens with opacity modifiers, and the semantic type scale.
  Genuinely unmappable one-offs go into a scoped rule in the stylesheet.
  Arbitrary *variants* (`data-[state=open]:…`) are fine.

## CI

GitHub Actions, Linux and Windows runners. Not set up yet: task T-007.
