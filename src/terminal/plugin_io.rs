//! What a plugin may do to a pane once the user allowed it (spec 0016,
//! Actions): type text at its prompt, and read its last lines.

use super::{State, TerminalPane, select};
use alacritty_terminal::event::Notify;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::TermMode;

/// The most lines `read_output` sends (spec 0016, Actions).
const MAX_READ_LINES: u32 = 1000;

impl TerminalPane {
    /// Types `text` at the program's prompt as a paste, and Enter after
    /// it if `enter`. False, with nothing sent, for a program that exited
    /// or one waiting on a prompt dialog (TM-plug-21).
    pub fn type_for_plugin(&self, text: &str, enter: bool) -> bool {
        if self.exited || self.dialog.is_some() {
            return false;
        }
        let State::Running(running) = &self.state else {
            return false;
        };
        let mut bytes = {
            let mut term = running.term.lock();
            let bracketed = term.mode().contains(TermMode::BRACKETED_PASTE);
            term.scroll_display(Scroll::Bottom);
            select::paste_bytes(text, bracketed)
        };
        if enter {
            bytes.push(b'\r');
        }
        running.notifier.notify(bytes);
        true
    }

    /// What `read_output` would send: the selection, else the last
    /// `lines` lines that hold text (at most [`MAX_READ_LINES`]).
    pub fn output(&self, lines: u32) -> String {
        if let Some(selected) = self.selected_text() {
            return selected;
        }
        let State::Running(running) = &self.state else {
            return String::new();
        };
        let lines = lines.clamp(1, MAX_READ_LINES) as i32;
        let term = running.term.lock();
        let bottom = Line(term.screen_lines() as i32 - 1);
        let top =
            Line(-(term.history_size() as i32)).max(bottom - (lines + term.screen_lines() as i32));
        let text = term.bounds_to_string(
            Point::new(top, Column(0)),
            Point::new(bottom, Column(term.columns() - 1)),
        );
        let mut kept: Vec<&str> = text.lines().map(str::trim_end).collect();
        while kept.last().is_some_and(|line| line.is_empty()) {
            kept.pop();
        }
        let skip = kept.len().saturating_sub(lines as usize);
        kept[skip..].join("\n")
    }
}
