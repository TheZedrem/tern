---
status: next
blocked-by: []
---
# T-051: Split the oversized modules by responsibility

Five files grew past the size rule in `docs/conventions.md` §Rust (about
800 lines of non-test code) because every feature appended to the view
that hosts it. Big files cost agents whole-file reads, make edits collide
and hide which part of a view owns what. Split them before the plugin
phases add more. Area: src.

**How.** Move code, don't change it. Rust lets one type's `impl` span
several files: a child module of the type's module sees its private
fields, so `impl Workspace` blocks move into `layout/<area>.rs` without
touching visibility. Tests move with the code they test. One commit per
file (`refactor(<scope>): split <file> by responsibility`), each with
`cargo fmt`, `clippy`, `cargo test` green, the same test count before and
after, and a look at the running app. No TTFP measurement unless boot
code moves (only `layout/mod.rs`'s `Boot` does, and it stays put).

Non-test lines today, and the split (method names are the current ones):

1. **`layout/mod.rs`, 3,317 lines.** `Workspace` hosts every feature.
   - `mod.rs` keeps `Workspace`'s fields, `new`, `Boot`, the action
     wiring and `Render`.
   - `session.rs`: `save_soon`, `save_on_exit`, `save_last`, `snapshot`,
     `saved_leaf`, `restored_files`, `restored_profile`.
   - `files.rs`: `add_files`, `files_event`, `run_file`, `open_files`,
     `switch_files_target`, `open_files_in_new_tab`, plus the SFTP sidebar
     (`toggle_sftp`, `render_sftp`, `drag_sftp_edge`).
   - `profiles.rs`: palette and profile picking (`open_palette`,
     `show_palette`, `palette_event`, `close_palette`, `render_palette`,
     `pick_host`, `pick_split_profile`, `new_tab_with_profile`,
     `open_profile_tab`, `split_with`, `forget_host`, `select_theme`,
     `save_theme`).
   - `reload.rs`: `start_reloads`, `ReloadJob`, `Reloaded`, `apply_reload`,
     `apply_settings`, and the settings screen hooks (`open_settings`,
     `settings_event`, `write_change`, `pick_setting`, `render_settings`).
   - `closing.rs`: `Closing`, `closing_leaves`, `edits_of`, `close`,
     `close_now`, `ask_close`, `answer_close`, `finish_closing`,
     `after_close`, `close_pane`, `close_tab`.
   - `splits.rs`: `split`, `has_room`, drag and drop (`drag_over_pane`,
     `drop_on_pane`, `pane_to_tab`, `drag_divider`, the drag types),
     `render_leaf`, `render_node`.
   - `chrome.rs`: `render_tabs`, `render_side_tabs`, `render_title_bar`,
     `title_bar_space`, `window_controls`, `settings_button`, renaming.
2. **`files/browser.rs`, 1,935 lines.** One `impl Browser` of 1,350 lines
   plus a 230-line `Render`. Split like `browser/edits.rs` already is:
   `browser/navigate.rs` (tree, path field, connection), `browser/
   transfers.rs` (jobs, uploads, downloads, drops), `browser/changes.rs`
   (rename, delete, new folder, the `Naming` and `Ask` flows),
   `browser/menu.rs` (the key and context menus), `browser/render.rs`.
3. **`terminal/mod.rs`, 1,897 lines.** `impl TerminalPane` is 990 lines.
   Out: the prompt answering (`PromptHost`, `answer_prompts`,
   `save_secret`, `Offers`, `Lookup`) to `terminal/answers.rs`; title and
   exit helpers (`clean_title`, `clean_prompt`, `after_exit`,
   `exit_notice`) to `terminal/exit.rs`; the search bar and IME input
   handler to `search.rs` and `input.rs`, which exist already.
4. **`settings/mod.rs`, about 1,700 lines.** `Render` alone is 540 lines.
   One file per page under `settings/pages/` (general, appearance, keys,
   profiles, import), each with its rows and render; the import cards
   (`ImportCard`, `card_lines`) go with the import page.
5. **`config.rs`, 1,413 lines** (3,310 with tests). Becomes `config/`:
   `mod.rs` the types and `Config`'s methods, `load.rs` the lenient reader
   (`Attempt`, `MapReader`, the `*Reader`s, `parse`, `load`), `edit.rs`
   the in-place edits (`save_edit`, `with_theme_default`, the spot
   finders), `watch.rs` the `Watcher`. The tests split along the same
   lines.

Done when no file under `src/` has more than about 800 non-test lines, or
the task says why one stays bigger, and `spec/architecture.md` §Code
layout names the new files where it lists them.

Spec: ../../spec/architecture.md
