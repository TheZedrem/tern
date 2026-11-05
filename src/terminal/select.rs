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
