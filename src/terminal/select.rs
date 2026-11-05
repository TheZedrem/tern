//! Mouse selection, copy and paste (spec 0002, flow 4): which cell a click
//! lands on, and what copied and pasted text may hold (threat model 0002).
//! GPUI-free, so it's testable without a window.

use super::pty::{GridSize, PADDING};
use alacritty_terminal::index::{Column, Line, Point, Side};

/// The grid cell under `position`, and which half of it, in a pane whose
/// grid starts at `origin` with cells of `cell`, all in pixels; clamped to
/// the grid. `display_offset` is how far the view is scrolled back.
pub fn cell_at(
    position: (f32, f32),
    origin: (f32, f32),
    cell: (f32, f32),
    size: GridSize,
    display_offset: usize,
) -> (Point, Side) {
    let x = (position.0 - origin.0 - PADDING) / cell.0;
    let y = (position.1 - origin.1 - PADDING) / cell.1;
    let (cols, rows) = (usize::from(size.cols), usize::from(size.rows));
    let col = (x.max(0.0) as usize).min(cols - 1);
    let row = (y.max(0.0) as usize).min(rows - 1);
    let side = if x >= cols as f32 || (x >= 0.0 && x.fract() >= 0.5) {
        Side::Right
    } else {
        Side::Left
    };
    let line = Line(row as i32 - display_offset as i32);
    (Point::new(line, Column(col)), side)
}

/// The bytes a paste of `text` sends (TM-escape-07): every control
/// character but tab and line breaks dropped, so the text can neither end a
/// bracketed paste early nor type an escape sequence; then bracketed when
/// the program asked for it, else with each line break sent as Enter.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let clean: String = text
        .chars()
        .filter(|&c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
        .collect();
    if bracketed {
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        clean.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// What a copy of `selected` puts on the clipboard: the text without the
/// zero-width and direction characters the screen didn't show
/// (TM-escape-12), by the title rules.
pub fn copy_text(selected: &str) -> String {
    selected
        .chars()
        .filter(|&c| !super::is_invisible_format(c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 10×5 grid of 8×16 pixel cells, in a pane at (100, 50).
    const SIZE: GridSize = GridSize {
        cols: 10,
        rows: 5,
        cell_width: 8,
        cell_height: 16,
    };
    const ORIGIN: (f32, f32) = (100.0, 50.0);
    const CELL: (f32, f32) = (8.0, 16.0);

    /// The point `cols` cells right of the grid's left edge and `rows`
    /// cells below its top, which the pane draws inside its padding.
    fn at(cols: f32, rows: f32) -> (f32, f32) {
        (
            ORIGIN.0 + PADDING + cols * CELL.0,
            ORIGIN.1 + PADDING + rows * CELL.1,
        )
    }

    /// Cell `col` of `line`: 0 is the top of the screen, negative lines
    /// are in the scrollback.
    fn cell(line: i32, col: usize) -> Point {
        Point::new(Line(line), Column(col))
    }

    /// Spec 0002, flow 4: a click lands on the cell drawn under it, on the
    /// half it hit, so a drag selects from there. Scrolled back, the rows
    /// on screen are lines of the scrollback.
    #[test]
    fn a_point_maps_to_the_half_of_the_cell_drawn_under_it_in_the_scrolled_view() {
        let left = cell_at(at(2.25, 1.5), ORIGIN, CELL, SIZE, 0);
        assert_eq!(left, (cell(1, 2), Side::Left));
        let right = cell_at(at(2.75, 1.5), ORIGIN, CELL, SIZE, 0);
        assert_eq!(right, (cell(1, 2), Side::Right));
        // Scrolled back three lines, the top row is three lines up.
        let top = cell_at(at(0.25, 0.5), ORIGIN, CELL, SIZE, 3);
        assert_eq!(top, (cell(-3, 0), Side::Left));
        let bottom = cell_at(at(9.75, 4.5), ORIGIN, CELL, SIZE, 3);
        assert_eq!(bottom, (cell(1, 9), Side::Right));
    }

    /// Spec 0002, flow 4: dragging past the grid's edge selects up to the
    /// edge. A point outside the grid, in the padding or beyond the pane,
    /// lands on the nearest cell, on the side that faces out.
    #[test]
    fn a_point_outside_the_grid_maps_to_the_nearest_edge_cell() {
        // In the padding at the top left, and beyond the pane up there.
        let padding = cell_at(ORIGIN, ORIGIN, CELL, SIZE, 0);
        assert_eq!(padding, (cell(0, 0), Side::Left));
        let beyond = cell_at((0.0, 0.0), ORIGIN, CELL, SIZE, 0);
        assert_eq!(beyond, (cell(0, 0), Side::Left));
        // Right of the last column, and below the last row.
        let right = cell_at(at(12.0, 2.5), ORIGIN, CELL, SIZE, 0);
        assert_eq!(right, (cell(2, 9), Side::Right));
        let below = cell_at(at(3.25, 9.0), ORIGIN, CELL, SIZE, 0);
        assert_eq!(below, (cell(4, 3), Side::Left));
        // Past the bottom right corner, scrolled back two lines.
        let corner = cell_at(at(40.0, 40.0), ORIGIN, CELL, SIZE, 2);
        assert_eq!(corner, (cell(2, 9), Side::Right));
    }

    /// What a paste of `text` sends, as text.
    fn pasted(text: &str, bracketed: bool) -> String {
        String::from_utf8(paste_bytes(text, bracketed)).expect("a paste is UTF-8")
    }

    /// TM-escape-07: a paste can't type an escape sequence or a control
    /// key. ESC, DEL, the C1 controls and every other control character
    /// but tab, `\n` and `\r` are dropped, bracketed or not.
    #[test]
    fn a_paste_drops_every_control_character_but_tab_and_line_breaks() {
        let controls = ('\0'..='\x1f').chain('\x7f'..='\u{9f}');
        for c in controls.filter(|c| !matches!(c, '\t' | '\n' | '\r')) {
            let text = format!("a{c}b");
            assert_eq!(pasted(&text, false), "ab", "{c:?}");
            assert_eq!(pasted(&text, true), "\x1b[200~ab\x1b[201~", "{c:?}");
        }
        assert_eq!(pasted("a\tb", false), "a\tb");
        assert_eq!(pasted("a\tb", true), "\x1b[200~a\tb\x1b[201~");
    }

    /// TM-escape-07, AC4: with bracketed paste on, the text goes to the
    /// program between one `ESC [200~` and one `ESC [201~`, line breaks
    /// and all. A pasted `ESC [201~` loses its ESC, so it can't end the
    /// paste early and have the rest run as typed (pastejacking).
    #[test]
    fn a_bracketed_paste_is_one_paste_that_pasted_text_cant_end_early() {
        assert_eq!(
            pasted("echo hi\x1b[201~\nrm -rf ~\n", true),
            "\x1b[200~echo hi[201~\nrm -rf ~\n\x1b[201~"
        );
    }

    /// TM-escape-08: without bracketed paste zed-term pastes like xterm,
    /// each line break sent as Enter (`\r`), whether the text ends its
    /// lines in `\n`, `\r\n` or `\r`.
    #[test]
    fn without_bracketed_paste_each_line_break_is_sent_as_enter() {
        assert_eq!(
            pasted("one\ntwo\r\nthree\rfour\n", false),
            "one\rtwo\rthree\rfour\r"
        );
    }

    /// TM-escape-12: a copy holds what the screen showed. Zero-width and
    /// direction characters are dropped, so neither a zero-width space in
    /// a command nor `invoice\u{202E}fdp.exe` pastes as other than it
    /// looked. The joiners emoji and scripts like Persian need stay, and
    /// so does the rest of the text, tabs and line breaks included.
    #[test]
    fn copy_drops_zero_width_and_direction_characters_but_keeps_the_joiners() {
        // Zero-width space, word joiner, zero-width no-break space, the
        // direction marks, embeddings, overrides and isolates.
        let hidden = ['\u{200B}', '\u{2060}', '\u{FEFF}']
            .into_iter()
            .chain(['\u{200E}', '\u{200F}', '\u{061C}'])
            .chain('\u{202A}'..='\u{202E}')
            .chain('\u{2066}'..='\u{2069}');
        for c in hidden {
            let selected = format!("r{c}m -rf /tmp/build");
            assert_eq!(copy_text(&selected), "rm -rf /tmp/build", "{c:?}");
        }
        // 👨‍💻 (man, zero-width joiner, laptop), Persian "mikhaham" (I
        // want) with a zero-width non-joiner after "mi", and two lines.
        let shown = [
            "\u{1F468}\u{200D}\u{1F4BB}",
            "\u{0645}\u{06CC}\u{200C}\u{062E}\u{0648}\u{0627}\u{0647}\u{0645}",
            "ls -l\tsrc\ncargo test",
        ];
        for text in shown {
            assert_eq!(copy_text(text), text);
        }
    }
}
