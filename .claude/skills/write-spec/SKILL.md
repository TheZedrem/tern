---
name: write-spec
description: Write or revise a Tern feature spec in spec/. Use before implementing any behavior that isn't already pinned down in spec/, or when the user asks for a spec.
---

# Write spec

A spec pins down what a feature does before any code exists, so every
implementation pass makes the same decisions. It is short and
decision-dense. It isn't a design essay, and it isn't a transcript of the
code that will follow.

## Before writing

1. Read `spec/architecture.md` (stack, key decisions, ground rules, security) and every
   spec the feature touches. Startup-related work also needs
   `spec/0001-startup-performance.md`.
2. If the feature would contradict `architecture.md`, stop and ask the owner.
   Once agreed, update `architecture.md` in the same pass.
3. Only spec the current roadmap phase. Ideas for later phases go in a
   *Later* note or in `spec/open-questions.md`.

## File

`spec/NNNN-<kebab-name>.md`: the next free four-digit number (`ls spec/`;
never reuse one). Copy [`template.md`](template.md) and fill every section.
Delete a subsection only when it genuinely has nothing to say (e.g. no IPC).

## Writing it

- **Decisions at full precision, everything else as a sketch.** "Closing the
  last pane closes the tab" is one line, pinned. Don't write column-by-column
  tables or a full list of every command; code expresses those better, and
  they go stale the moment it ships.
- Mark rules that would be a bug to deviate from with **[load-bearing]**.
  Library mechanics belong in code comments, not specs.
- **Flows are numbered steps that include the failure paths:** bad input,
  missing file, dead process, Windows behaving differently. The failure
  paths are where the decisions are.
- **Startup impact is mandatory.** State what the feature adds to the boot
  path. The normal answer is "nothing, initialized on first use or after
  the first frame". Anything else needs a TTFP measurement and the owner's OK.
- **Platform notes are mandatory.** Linux (Wayland) and Windows 11 must both
  work, so name their differences (ConPTY, paths, keys, compositor
  decorations, GPU backend). macOS only needs to compile. Never add a code
  path for X11, XWayland or Windows 10.
- **Security & privacy:** list what gets stored, where, and for how long
  (or "nothing stored"), and which trust boundary from `architecture.md`
  §Security the feature crosses (or "none"). Check it against the ground
  rules: no secrets outside the keychain, no persisted terminal output, no
  network the user didn't start.
- Budget: about two screens. A feature that needs more is probably two
  specs.

## Acceptance criteria

6–10 observable checks a reviewer can run without interpreting intent. Each
one should catch a plausible failure. Don't restate the design.

BAD
> Resizing works well

GOOD
> Resizing the window while `vim` is open redraws vim at the new size with no stale rows (Linux and Windows)

## After writing

1. Leave `status: draft` and ask the owner to review the spec before any
   implementation starts.
2. Add the backlog tasks the spec implies to `tasks/` (format in
   `tasks/README.md`), and link them from the spec's frontmatter.
3. Add unresolved, load-bearing unknowns to `spec/open-questions.md` as the
   next free OQ-nnn, and reference them from the spec.
4. If implementation later diverges from the spec, update the spec in the
   same change. A stale spec is worse than none.
