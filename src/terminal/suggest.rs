//! Inline command suggestions (spec 0021, flows 3 to 5): what's typed at
//! a local shell's prompt, and the rest of the newest past command that
//! starts with it. The shell only says that a prompt begins, so where its
//! input starts is read off the cursor at the first key after that
//! (OQ-041).

use super::{State, TerminalPane};
use crate::history::{Recorder, ShellKind};
use alacritty_terminal::event::Notify;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};
use gpui::{App, Context, Window};

/// Where a pane's shell is with its prompt.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Prompt {
    /// A command runs, or the shell reports no prompts.
    #[default]
    Away,
    /// The shell began a prompt and nothing was typed at it yet.
    Fresh(ShellKind),
    /// Input starts `start` cells into the cursor's line. An offset, not
    /// a grid point, so it holds when the line moves: a cleared screen,
    /// a resize, ConPTY's repaint.
    Input { shell: ShellKind, start: usize },
}

/// The row the cursor's line starts on, following wrapped rows up, and
/// how many cells into that line the cursor is. `None` if the line starts
/// above the screen: the scrollback is never read, and an offset counted
/// from the top row would point into the middle of the line.
fn cursor_offset<T>(term: &Term<T>) -> Option<(Line, usize)> {
    let grid = term.grid();
    let cursor = grid.cursor.point;
    let columns = grid.columns();
    let last = Column(columns - 1);
    let mut first = cursor.line;
    let wraps = |line: Line| grid[line][last].flags.contains(Flags::WRAPLINE);
    while first.0 > 0 && wraps(Line(first.0 - 1)) {
        first = Line(first.0 - 1);
    }
    // Only the wrap flag of the last scrollback row is looked at.
    if first.0 == 0 && grid.history_size() > 0 && wraps(Line(-1)) {
        return None;
    }
    let rows = usize::try_from(cursor.line.0 - first.0).ok()?;
    Some((first, rows * columns + cursor.column.0))
}

/// What's typed between `start` and the cursor, if a suggestion may show
/// after it (spec 0021, flow 3): the view is at the bottom of the main
/// screen, something other than spaces is typed, and the cursor's cell
/// and the rest of its row are blank. The last also steps aside for a
/// shell that draws suggestions of its own there.
pub fn typed<T>(term: &Term<T>, start: usize) -> Option<String> {
    let grid = term.grid();
    if grid.display_offset() != 0 || term.mode().contains(TermMode::ALT_SCREEN) {
        return None;
    }
    let (first, end) = cursor_offset(term)?;
    let cursor = grid.cursor.point;
    let columns = grid.columns();
    let blank = (cursor.column.0..columns).all(|column| {
        let cell = &grid[cursor.line][Column(column)];
        cell.c == ' ' && cell.zerowidth().is_none()
    });
    if !blank || end <= start {
        return None;
    }
    let mut text = String::new();
    for offset in start..end {
        let line = Line(first.0 + i32::try_from(offset / columns).ok()?);
        let cell = &grid[line][Column(offset % columns)];
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        text.push(cell.c);
        text.extend(cell.zerowidth().into_iter().flatten());
    }
    text.contains(|c: char| !c.is_whitespace()).then_some(text)
}

/// A suggestion's rest as it's typed into the shell: without control
/// characters [load-bearing], so accepting can't press Enter or send an
/// escape sequence (TM-escape-17).
fn accepted_text(rest: &str) -> String {
    rest.chars().filter(|c| !c.is_control()).collect()
}

/// Whether `bytes` typed at a prompt leave it: Enter or `Ctrl+C` (spec
/// 0021, flow 5).
fn leaves_prompt(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .any(|byte| matches!(byte, b'\r' | b'\n' | 0x03))
}

impl TerminalPane {
    /// Takes the prompt the shell reported since the last look.
    pub(super) fn sync_prompt(&mut self) {
        if let State::Running(running) = &self.state
            && let Some(shell) = running.reported.take_prompt()
        {
            self.prompt = Prompt::Fresh(shell);
            self.snippet_reported(shell);
        }
    }

    /// Sends the user's input to the program. The first input at a new
    /// prompt pins where its input starts, before the shell echoes it.
    /// The caller holds no lock on the terminal.
    pub(super) fn send(&mut self, bytes: Vec<u8>) {
        self.sync_prompt();
        let State::Running(running) = &self.state else {
            return;
        };
        if let Prompt::Fresh(shell) = self.prompt {
            self.prompt = match cursor_offset(&running.term.lock()) {
                Some((_, start)) => Prompt::Input { shell, start },
                None => Prompt::Away,
            };
        }
        if leaves_prompt(&bytes) {
            self.prompt = Prompt::Away;
        }
        running.notifier.notify(bytes);
    }

    /// The rest of the newest past command that starts with what's typed
    /// at the prompt (spec 0021, flow 3). Reads the input's rows only.
    pub(super) fn suggestion(&self, cx: &App) -> Option<String> {
        let Prompt::Input { shell, start } = self.prompt else {
            return None;
        };
        if self.exited || self.preedit.is_some() || self.search.is_some() || self.dialog.is_some() {
            return None;
        }
        let State::Running(running) = &self.state else {
            return None;
        };
        let recorder = cx.try_global::<Recorder>()?;
        let typed = typed(&running.term.lock(), start)?;
        recorder.suggest(shell, &typed)
    }

    /// Types the shown suggestion into the shell, as text and without
    /// Enter [load-bearing] (spec 0021, flow 4; TM-escape-17). Returns
    /// whether there was one; if not, the key is the shell's.
    pub fn accept_suggestion(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        if !self.focus.is_focused(window) {
            return false;
        }
        self.sync_prompt();
        let Some(rest) = self.suggestion(cx) else {
            return false;
        };
        self.send(accepted_text(&rest).into_bytes());
        self.keep_cursor_shown();
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::pty::GridSize;
    use crate::terminal::tests::term_with;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Scroll;
    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

    const COLS: u16 = 20;
    const ROWS: u16 = 5;

    /// A terminal showing `before`, where the first key at the prompt
    /// pins the input start (as `send` does), and that start.
    fn pinned(before: &[u8]) -> (Term<VoidListener>, usize) {
        let term = term_with(COLS, ROWS, before);
        let (_, start) = cursor_offset(&term).expect("the prompt's line starts on screen");
        (term, start)
    }

    /// Has `term` parse what its program printed next.
    fn print(term: &mut Term<VoidListener>, bytes: &[u8]) {
        Processor::<StdSyncHandler>::new().advance(term, bytes);
    }

    /// What's read as typed once the shell drew `prompt`, the first key
    /// pinned the input start, and the shell then printed `echo`.
    fn typed_at(prompt: &[u8], echo: &[u8]) -> Option<String> {
        let (mut term, start) = pinned(prompt);
        print(&mut term, echo);
        typed(&term, start)
    }

    /// Spec 0021, flow 3 and AC1: "The typed text runs from the input
    /// start to the cursor", so the prompt's own text is no part of it.
    #[test]
    fn the_typed_text_runs_from_the_input_start_to_the_cursor() {
        assert_eq!(typed_at(b"$ ", b"git s").as_deref(), Some("git s"));
        assert_eq!(typed_at(b"PS C:\\> ", b"Get-Ch").as_deref(), Some("Get-Ch"));
    }

    /// Spec 0021, Model: the start is an offset into the cursor's line,
    /// so a prompt of several lines, below earlier output, counts only
    /// its last line.
    #[test]
    fn a_prompt_of_several_lines_counts_only_the_cursors_line() {
        let prompt = b"old output\r\n\r\nme@box ~/src\r\n> ";
        assert_eq!(typed_at(prompt, b"cargo b").as_deref(), Some("cargo b"));
    }

    /// Spec 0021, Model: only wrapped rows count as one line. Output that
    /// wrapped and then ended with a newline, right above the prompt,
    /// isn't part of the prompt's line.
    #[test]
    fn wrapped_output_above_the_prompt_is_no_part_of_its_line() {
        let (term, start) = pinned(b"0123456789012345678901234\r\n$ ");
        assert_eq!(start, 2);
        assert_eq!(cursor_offset(&term), Some((Line(2), 2)));
    }

    /// Spec 0021, Model ("its wrapped rows counted as one"): input that
    /// runs onto a second row is still read whole.
    #[test]
    fn input_that_wraps_onto_a_second_row_reads_as_one_line() {
        let command = "echo one two three four";
        assert!(command.len() + 2 > usize::from(COLS));
        assert_eq!(
            typed_at(b"$ ", command.as_bytes()).as_deref(),
            Some(command)
        );
    }

    /// Spec 0021, Model: the start still holds when the first key came
    /// while the prompt's line had already wrapped (text typed ahead, or
    /// a long prompt).
    #[test]
    fn a_start_on_the_second_row_of_a_wrapped_line_still_holds() {
        let prompt = b"a-very-long-prompt-of-25> ";
        let (mut term, start) = pinned(prompt);
        assert_eq!(start, prompt.len());
        print(&mut term, b"git s");
        assert_eq!(typed(&term, start).as_deref(), Some("git s"));
    }

    /// Spec 0021, Model: the start is "not a grid point, so it still
    /// holds after `Ctrl+L`": the screen is cleared and the same line
    /// redrawn at the top.
    #[test]
    fn the_start_still_holds_after_the_screen_is_cleared_and_redrawn() {
        let (mut term, start) = pinned(b"one\r\ntwo\r\nthree\r\n$ ");
        print(&mut term, b"git s");
        assert_eq!(typed(&term, start).as_deref(), Some("git s"));
        print(&mut term, b"\x1b[H\x1b[2J$ git s");
        assert_eq!(typed(&term, start).as_deref(), Some("git s"));
        print(&mut term, b"t");
        assert_eq!(typed(&term, start).as_deref(), Some("git st"));
    }

    /// A grid `cols` wide, as a resize hands it to the terminal.
    fn width(cols: u16) -> GridSize {
        GridSize {
            cols,
            rows: ROWS,
            cell_width: 8,
            cell_height: 16,
        }
    }

    /// Spec 0021, Model: the start still holds after a resize. A wider
    /// window puts input that had wrapped back on one row.
    #[test]
    fn the_start_still_holds_after_a_wider_window_unwraps_the_input() {
        let command = "echo one two three four";
        let (mut term, start) = pinned(b"$ ");
        print(&mut term, command.as_bytes());
        term.resize(width(40));
        assert_eq!(typed(&term, start).as_deref(), Some(command));
    }

    /// Spec 0021, Model: the start still holds after a resize. A narrower
    /// window wraps the input onto more rows, all still on screen.
    #[test]
    fn the_start_still_holds_after_a_narrower_window_wraps_the_input() {
        let (mut term, start) = pinned(b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n$ ");
        print(&mut term, b"echo one two three");
        term.resize(width(8));
        assert_eq!(typed(&term, start).as_deref(), Some("echo one two three"));
    }

    /// Spec 0021, flow 5: "Input whose first rows have scrolled off the
    /// top of the screen gets no suggestion, because the scrollback is
    /// never read." With the prompt on the top row, a narrower window
    /// moves the line's first rows into the scrollback; a piece from the
    /// middle of the command must not be read as what's typed.
    #[test]
    fn input_whose_first_rows_a_resize_moved_off_screen_reads_as_nothing() {
        let (mut term, start) = pinned(b"$ ");
        print(&mut term, b"echo one two three");
        term.resize(width(8));
        assert_ne!(term.grid().history_size(), 0, "nothing moved off screen");
        assert_eq!(typed(&term, start), None);
        assert_eq!(cursor_offset(&term), None);
    }

    /// Spec 0021, flow 5, as above: the same for input longer than the
    /// screen, whose first row scrolled off as it was typed. No start
    /// is pinned there either.
    #[test]
    fn input_longer_than_the_screen_reads_as_nothing() {
        let (mut term, start) = pinned(b"$ ");
        let fits = "a".repeat(usize::from(COLS) * usize::from(ROWS) - 3);
        print(&mut term, fits.as_bytes());
        assert_eq!(typed(&term, start).as_deref(), Some(fits.as_str()));
        print(&mut term, b"bb");
        assert_eq!(typed(&term, start), None);
        assert_eq!(cursor_offset(&term), None);
    }

    /// Spec 0021, flow 4 and TM-escape-17 [load-bearing]: accepting
    /// "writes only the printable rest of the entry as typed text, drops
    /// control characters and never sends Enter". Neither a line break,
    /// `Ctrl+C` nor an escape sequence survives.
    #[test]
    fn accepted_text_drops_every_control_character() {
        assert_eq!(accepted_text("tatus\r"), "tatus");
        assert_eq!(accepted_text("\ntatus\r\n; rm -rf ~\n"), "tatus; rm -rf ~");
        assert_eq!(accepted_text("a\x1b[201~b\x1bOCc"), "a[201~bOCc");
        assert_eq!(accepted_text("a\tb\x03c\x04d\x00e\x08f\x7fg"), "abcdefg");
        // C1 controls too: CSI and NEL as single characters.
        assert_eq!(accepted_text("a\u{9b}b\u{85}c"), "abc");
        assert_eq!(accepted_text("\r\n\x1b\x03"), "");
        let every_control: String = (0..0x20u8).chain([0x7f]).map(char::from).collect();
        assert_eq!(accepted_text(&every_control), "");
    }

    /// Spec 0021, flow 4: what's printable is typed as it is, spaces,
    /// quotes and non-ASCII text included.
    #[test]
    fn accepted_text_keeps_printable_text_unchanged() {
        for rest in [
            "tatus",
            " --short  -b",
            "echo \"a b\" 'c' $HOME | grep ~ \\ `x`",
            "echo äöü ß 中文 é 🙂",
            "",
        ] {
            assert_eq!(accepted_text(rest), rest);
        }
    }

    /// Spec 0021, flow 3: "the prompt was reported and something was
    /// typed at it".
    #[test]
    fn nothing_typed_reads_as_nothing() {
        assert_eq!(typed_at(b"$ ", b""), None);
        // Backspaced over everything typed: the cursor is at the start again.
        assert_eq!(typed_at(b"$ ", b"gi\x08\x08\x1b[K"), None);
    }

    /// Spec 0021, flow 3: "the typed text has at least one non-space
    /// character".
    #[test]
    fn only_spaces_typed_reads_as_nothing() {
        assert_eq!(typed_at(b"$ ", b"   "), None);
        assert_eq!(typed_at(b"$ ", b"  g").as_deref(), Some("  g"));
    }

    /// Spec 0021, flow 3 and AC2: "every cell after the cursor on its row
    /// is blank". After `Left` the cursor sits on typed text, so nothing
    /// is suggested and Right is the shell's.
    #[test]
    fn a_cursor_moved_left_into_the_input_reads_as_nothing() {
        assert_eq!(typed_at(b"$ ", b"git s\x1b[D"), None);
        assert_eq!(typed_at(b"$ ", b"git status\x1b[4D"), None);
        // Back at the end, it reads again.
        assert_eq!(
            typed_at(b"$ ", b"git s\x1b[D\x1b[C").as_deref(),
            Some("git s")
        );
    }

    /// AC8: a shell that draws its own grey prediction after the cursor
    /// (PSReadLine, zsh-autosuggestions) gets no second one from Tern.
    #[test]
    fn a_shells_own_prediction_after_the_cursor_reads_as_nothing() {
        assert_eq!(typed_at(b"$ ", b"git s\x1b[90mtatus\x1b[0m\x1b[5D"), None);
    }

    /// Spec 0021, Platform notes: "A zsh `RPROMPT` fills the cells after
    /// the cursor, so no suggestion shows on its row", however far right.
    #[test]
    fn text_at_the_right_edge_of_the_cursors_row_reads_as_nothing() {
        assert_eq!(typed_at(b"% ", b"\x1b7\x1b[1;19Hok\x1b8git s"), None);
    }

    /// AC10 and TM-escape-18: nothing is read on the alternate screen
    /// (vim, `less`), whatever stands at the cursor's row there.
    #[test]
    fn the_alternate_screen_reads_as_nothing() {
        let (mut term, start) = pinned(b"$ ");
        print(&mut term, b"vim x\r\n\x1b[?1049h\x1b[Hsome file text");
        assert_eq!(typed(&term, start), None);
        // Back on the main screen at a prompt, it reads again.
        print(&mut term, b"\x1b[?1049l\r\n$ git s");
        assert_eq!(typed(&term, start).as_deref(), Some("git s"));
    }

    /// AC10: nothing is read while scrolled up in the scrollback, and it
    /// is again once the view is back at the bottom.
    #[test]
    fn a_view_scrolled_up_in_the_scrollback_reads_as_nothing() {
        let (mut term, start) = pinned(b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n$ ");
        print(&mut term, b"git s");
        term.scroll_display(Scroll::Delta(2));
        assert_ne!(term.grid().display_offset(), 0, "not scrolled");
        assert_eq!(typed(&term, start), None);
        term.scroll_display(Scroll::Bottom);
        assert_eq!(typed(&term, start).as_deref(), Some("git s"));
    }

    /// Spec 0021, flow 3 (exact match): a wide character takes two cells
    /// but is one character of the typed text, or no entry would match.
    #[test]
    fn a_wide_character_is_read_once() {
        assert_eq!(
            typed_at(b"$ ", "echo 中文 a".as_bytes()).as_deref(),
            Some("echo 中文 a")
        );
    }

    /// As above, where a wide character didn't fit the row's last cell
    /// and went to the next row: the cell left empty isn't typed text.
    #[test]
    fn a_wide_character_pushed_to_the_next_row_is_read_once_and_without_a_gap() {
        // "$ " and 17 letters leave one cell on the first row.
        let command = format!("{}中x", "a".repeat(17));
        assert_eq!(
            typed_at(b"$ ", command.as_bytes()).as_deref(),
            Some(command.as_str())
        );
    }

    /// A combining mark stays with its letter, so the text matches the
    /// command as it was recorded.
    #[test]
    fn a_combining_mark_is_read_with_its_letter() {
        let command = "echo e\u{301}t";
        assert_eq!(
            typed_at(b"$ ", command.as_bytes()).as_deref(),
            Some(command)
        );
    }

    /// Spec 0021, flow 5: text typed ahead while a command ran is echoed
    /// before the first key, so it lies before the input start and "a
    /// suggestion then completes only what was typed after it".
    #[test]
    fn text_typed_ahead_lies_before_the_input_start() {
        assert_eq!(typed_at(b"$ git ", b"st").as_deref(), Some("st"));
    }

    /// Spec 0021, flow 5: "Enter, `Ctrl+C` or a new prompt's report
    /// clears the input start". A paste that holds a line break counts
    /// like Enter.
    #[test]
    fn enter_and_ctrl_c_leave_the_prompt() {
        for bytes in [&b"\r"[..], b"\n", b"\x03", b"ls\r", b"echo a\necho b"] {
            assert!(leaves_prompt(bytes), "{bytes:?}");
        }
    }

    /// Spec 0021, flow 5: typing, editing and moving the cursor stay at
    /// the prompt, so the input start is kept.
    #[test]
    fn typing_and_cursor_keys_stay_at_the_prompt() {
        let stay: [&[u8]; 9] = [
            b"git s",
            b" ",
            b"\t",
            b"\x7f",
            b"\x08",
            // Right and Left, normal and application cursor keys.
            b"\x1b[C",
            b"\x1bOC",
            b"\x1b[D",
            "中文é".as_bytes(),
        ];
        for bytes in stay {
            assert!(!leaves_prompt(bytes), "{bytes:?}");
        }
    }
}
