//! The visible part of a terminal grid, turned into what the element
//! paints: background spans, text runs and the cursor, all in cell
//! coordinates. GPUI-free, so it is testable with a plain `Term`.

use super::colors::{self, rgb};
use crate::theme::Theme;
use alacritty_terminal::term::RenderableContent;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::Match;
use alacritty_terminal::vte::ansi::{CursorShape, Rgb};

/// What one frame of a pane draws, in cell coordinates (row 0 is the top
/// visible row).
#[derive(Debug, Default, PartialEq)]
pub struct Frame {
    pub backgrounds: Vec<Span>,
    pub runs: Vec<Run>,
    pub cursor: Option<Cursor>,
}

/// Consecutive cells on one row sharing a non-default background.
#[derive(Debug, PartialEq, Eq)]
pub struct Span {
    pub row: usize,
    pub col: usize,
    pub width: usize,
    pub color: Rgb,
}

/// Text drawn with one style, one glyph per cell. A wide character (CJK)
/// is always a run of its own with `width` 2.
#[derive(Debug, PartialEq, Eq)]
pub struct Run {
    pub row: usize,
    pub col: usize,
    /// Cells covered.
    pub width: usize,
    pub text: String,
    pub style: TextStyle,
    /// A single wide character; never extended.
    pub wide: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextStyle {
    pub color: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
}

/// A cursor drawn on top of the text. A focused block cursor isn't here: it
/// is drawn by recoloring its cell (cursor color behind, background color
/// for the glyph).
#[derive(Debug, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub width: usize,
    pub shape: CursorShape,
    pub color: Rgb,
}

/// What a frame highlights besides the selection: the search bar's matches
/// on screen, in reading order, and the one the view is on (spec 0002,
/// flow 6).
#[derive(Debug, Default)]
pub struct Marks {
    pub matches: Vec<Match>,
    pub current: Option<Match>,
}

/// Builds the frame for `content`. `focused` decides between a solid and a
/// hollow block cursor. Highlights go, strongest first: the current match
/// in the accent color, the selection, the other matches tinted.
pub fn build(content: RenderableContent<'_>, theme: &Theme, focused: bool, marks: &Marks) -> Frame {
    let default_bg = rgb(theme.background);
    let cursor_color = rgb(theme.cursor);
    let selection_color = rgb(theme.selection);
    let current_color = rgb(theme.accent);
    let match_color = colors::mix(current_color, default_bg, 0.35);
    let selection = content.selection;
    let mut next_match = 0;
    let offset = content.display_offset as i32;
    let cursor_point = content.cursor.point;
    let cursor_shape = match content.cursor.shape {
        CursorShape::Block if !focused => CursorShape::HollowBlock,
        shape => shape,
    };

    let mut frame = Frame::default();
    let mut current: Option<Run> = None;

    for indexed in content.display_iter {
        let cell = indexed.cell;
        let flags = cell.flags;
        if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let Ok(row) = usize::try_from(indexed.point.line.0 + offset) else {
            continue;
        };
        let col = indexed.point.column.0;
        let width = if flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        };

        let mut fg = colors::resolve(cell.fg, content.colors, theme);
        let mut bg = colors::resolve(cell.bg, content.colors, theme);
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if flags.contains(Flags::DIM) {
            fg = colors::dim(fg);
        }
        // Cells come in reading order, and so do the matches.
        while marks
            .matches
            .get(next_match)
            .is_some_and(|found| *found.end() < indexed.point)
        {
            next_match += 1;
        }
        if marks
            .current
            .as_ref()
            .is_some_and(|current| current.contains(&indexed.point))
        {
            bg = current_color;
            fg = default_bg;
        } else if selection.is_some_and(|range| range.contains(indexed.point)) {
            bg = selection_color;
        } else if marks
            .matches
            .get(next_match)
            .is_some_and(|found| found.contains(&indexed.point))
        {
            bg = match_color;
        }

        if indexed.point == cursor_point {
            if cursor_shape == CursorShape::Block {
                fg = default_bg;
                bg = cursor_color;
            } else if cursor_shape != CursorShape::Hidden {
                frame.cursor = Some(Cursor {
                    row,
                    col,
                    width,
                    shape: cursor_shape,
                    color: cursor_color,
                });
            }
        }

        if bg != default_bg {
            match frame.backgrounds.last_mut() {
                Some(span)
                    if span.row == row && span.col + span.width == col && span.color == bg =>
                {
                    span.width += width;
                }
                _ => frame.backgrounds.push(Span {
                    row,
                    col,
                    width,
                    color: bg,
                }),
            }
        }

        let style = TextStyle {
            color: fg,
            bold: flags.contains(Flags::BOLD),
            italic: flags.contains(Flags::ITALIC),
            underline: flags.intersects(Flags::ALL_UNDERLINES),
            strikeout: flags.contains(Flags::STRIKEOUT),
        };
        let mut text = String::new();
        text.push(if flags.contains(Flags::HIDDEN) {
            ' '
        } else {
            cell.c
        });
        if let Some(zerowidth) = cell.zerowidth() {
            text.extend(zerowidth);
        }

        let wide = width == 2;
        if let Some(run) = current.as_mut()
            && !wide
            && !run.wide
            && run.row == row
            && run.col + run.width == col
            && run.style == style
        {
            run.text.push_str(&text);
            run.width += 1;
            continue;
        }
        if let Some(run) = current.take() {
            push_run(&mut frame.runs, run);
        }
        current = Some(Run {
            row,
            col,
            width,
            text,
            style,
            wide,
        });
    }
    if let Some(run) = current {
        push_run(&mut frame.runs, run);
    }
    frame
}

/// Keeps runs that draw something: blank runs without decorations are
/// skipped, their backgrounds are already spans.
fn push_run(runs: &mut Vec<Run>, run: Run) {
    let blank = run.text.chars().all(|c| c == ' ');
    if !blank || run.style.underline || run.style.strikeout {
        runs.push(run);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::tests::term_with;
    use crate::theme::DARK;
    use alacritty_terminal::grid::Scroll;

    const COLS: u16 = 12;
    const ROWS: u16 = 3;

    /// The frame of a pane that has printed `bytes`.
    fn frame(bytes: &[u8], focused: bool) -> Frame {
        let term = term_with(COLS, ROWS, bytes);
        build(term.renderable_content(), &DARK, focused, &Marks::default())
    }

    /// The run that draws cell (`row`, `col`).
    fn run_at(frame: &Frame, row: usize, col: usize) -> &Run {
        frame
            .runs
            .iter()
            .find(|run| run.row == row && (run.col..run.col + run.width).contains(&col))
            .unwrap_or_else(|| panic!("no run at {row},{col} in {:?}", frame.runs))
    }

    /// Row `row` as drawn: each run's text at its columns, blanks elsewhere.
    fn row_text(frame: &Frame, row: usize) -> String {
        let mut cells = vec![String::from(" "); usize::from(COLS)];
        for run in frame.runs.iter().filter(|run| run.row == row) {
            if run.wide {
                cells[run.col] = run.text.clone();
                cells[run.col + 1].clear();
            } else {
                for (offset, c) in run.text.chars().enumerate() {
                    cells[run.col + offset] = c.to_string();
                }
            }
        }
        cells.concat().trim_end().to_owned()
    }

    /// A background span on row 0.
    fn span(col: usize, width: usize, color: u32) -> Span {
        Span {
            row: 0,
            col,
            width,
            color: rgb(color),
        }
    }

    #[test]
    fn text_is_drawn_at_its_row_and_column() {
        let frame = frame(b"ab\x1b[2;4Hcd", false);
        assert_eq!(row_text(&frame, 0), "ab");
        assert_eq!(row_text(&frame, 1), "   cd");
        assert_eq!(row_text(&frame, 2), "");
    }

    #[test]
    fn a_run_has_one_style_and_a_style_change_starts_a_new_run() {
        // "ab" plain, "c" bold, "d" bold red, then italic, underline, strikeout.
        let frame = frame(
            b"ab\x1b[1mc\x1b[31md\x1b[0;3mi\x1b[0;4mu\x1b[0;9ms\x1b[0m",
            false,
        );
        assert!(std::ptr::eq(run_at(&frame, 0, 0), run_at(&frame, 0, 1)));
        let style = |col| run_at(&frame, 0, col).style;
        let fg = rgb(DARK.foreground);
        assert_eq!((style(0).bold, style(0).color), (false, fg));
        assert_eq!((style(2).bold, style(2).color), (true, fg));
        assert_eq!((style(3).bold, style(3).color), (true, rgb(DARK.ansi[1])));
        assert!(style(4).italic && !style(4).bold);
        assert!(style(5).underline && !style(5).italic);
        assert!(style(6).strikeout && !style(6).underline);
        assert_eq!(row_text(&frame, 0), "abcdius");
    }

    #[test]
    fn adjacent_cells_with_one_background_share_a_span() {
        let frame = frame(b"\x1b[41mabc\x1b[42md\x1b[0m e", false);
        let (red, green) = (DARK.ansi[1], DARK.ansi[2]);
        // The default background needs no span.
        assert_eq!(frame.backgrounds, [span(0, 3, red), span(3, 1, green)]);
    }

    #[test]
    fn a_wide_character_is_its_own_run_two_cells_wide() {
        let frame = frame("a\x1b[41m中\x1b[0m文b".as_bytes(), false);
        let zhong = run_at(&frame, 0, 1);
        assert_eq!((zhong.col, zhong.width, zhong.text.as_str()), (1, 2, "中"));
        assert!(zhong.wide);
        assert!(std::ptr::eq(zhong, run_at(&frame, 0, 2)));
        let wen = run_at(&frame, 0, 3);
        assert_eq!((wen.col, wen.width, wen.text.as_str()), (3, 2, "文"));
        // The next character starts two columns after each wide one.
        assert_eq!(run_at(&frame, 0, 5).col, 5);
        assert_eq!(row_text(&frame, 0), "a中文b");
        // Its background covers both of its cells.
        assert_eq!(frame.backgrounds, [span(1, 2, DARK.ansi[1])]);
    }

    #[test]
    fn inverse_swaps_foreground_and_background() {
        let frame = frame(b"\x1b[7mx\x1b[31;42my\x1b[0m", false);
        let (fg, red, green) = (DARK.foreground, DARK.ansi[1], DARK.ansi[2]);
        assert_eq!(run_at(&frame, 0, 0).style.color, rgb(DARK.background));
        assert_eq!(run_at(&frame, 0, 1).style.color, rgb(green));
        assert_eq!(frame.backgrounds, [span(0, 1, fg), span(1, 1, red)]);
    }

    #[test]
    fn hidden_text_and_blank_cells_draw_no_runs() {
        let frame = frame(b"\x1b[8msecret\x1b[0m", false);
        assert!(frame.runs.is_empty(), "{:?}", frame.runs);
    }

    #[test]
    fn underlined_blanks_still_draw_their_underline() {
        let frame = frame(b"\x1b[4m  \x1b[0m", false);
        assert!(run_at(&frame, 0, 0).style.underline);
    }

    #[test]
    fn a_focused_block_cursor_recolors_its_cell() {
        let frame = frame(b"ab\r", true);
        let under = run_at(&frame, 0, 0);
        assert_eq!(under.text, "a");
        assert_eq!(under.style.color, rgb(DARK.background));
        assert_eq!(run_at(&frame, 0, 1).style.color, rgb(DARK.foreground));
        assert_eq!(frame.backgrounds, [span(0, 1, DARK.cursor)]);
        assert_eq!(frame.cursor, None);
    }

    #[test]
    fn an_unfocused_cursor_is_a_hollow_block_over_an_unchanged_cell() {
        let frame = frame(b"ab\r", false);
        let hollow = Cursor {
            row: 0,
            col: 0,
            width: 1,
            shape: CursorShape::HollowBlock,
            color: rgb(DARK.cursor),
        };
        assert_eq!(frame.cursor, Some(hollow));
        assert_eq!(run_at(&frame, 0, 0).style.color, rgb(DARK.foreground));
        assert!(frame.backgrounds.is_empty());
    }

    #[test]
    fn the_cursor_covers_both_cells_of_a_wide_character() {
        let focused = frame("中\r".as_bytes(), true);
        assert_eq!(focused.backgrounds, [span(0, 2, DARK.cursor)]);
        let unfocused = frame("中\r".as_bytes(), false);
        assert_eq!(unfocused.cursor.map(|cursor| cursor.width), Some(2));
    }

    #[test]
    fn a_hidden_cursor_is_not_drawn() {
        for focused in [true, false] {
            let frame = frame(b"ab\x1b[?25l", focused);
            assert_eq!(frame.cursor, None, "focused: {focused}");
            assert!(frame.backgrounds.is_empty(), "focused: {focused}");
        }
    }

    #[test]
    fn row_0_is_the_top_visible_row_when_scrolled_back() {
        let mut term = term_with(COLS, ROWS, b"1\r\n2\r\n3\r\n4\r\n5");
        term.scroll_display(Scroll::Delta(1));
        let frame = build(term.renderable_content(), &DARK, false, &Marks::default());
        let rows: Vec<_> = (0..usize::from(ROWS))
            .map(|row| row_text(&frame, row))
            .collect();
        assert_eq!(rows, ["2", "3", "4"]);
    }
}
