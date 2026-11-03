# tasks/ — one file per task

No connected tracker: this folder **is the backlog**. One file per task,
`T-nnn-<slug>.md` — sequential IDs, never reused. Frontmatter holds the
state, the body says what to build and links the spec that holds the
acceptance criteria. Resolve a task by moving its file into `resolved/`.

With a connected tracker (see the Tracker note in `AGENTS.md`): the backlog
lives in the tracker. Files here are **optional per-issue implementation
notes** (`<ISSUE-ID>-<slug>.md`), created only when a task accumulates notes
worth versioning — approach sketches, file paths, breadcrumbs the tracker
should stay clean of. Status never lives in these files; move them to
`resolved/` when their issue closes.

Task file format (backlog mode):

```markdown
---
status: now          # now | next | later
blocked-by: []       # e.g. [T-012, T-014]
---
# T-nnn: <imperative summary>

What to build, from the user's perspective — <area: src-tauri / src / meta>.
Spec: ../spec/NNNN-<feature>.md
```

Add `type: bug` to the frontmatter for bugs; omit it otherwise.
