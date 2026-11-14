//! The mouse over a pane (spec 0002, flows 4, 5 and 7): selecting and
//! copying, links, pasting, and the wheel.

use super::pty::GridSize;
use super::{PaneEvent, State, TerminalPane, link, select};
use alacritty_terminal::event::Notify;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{Term, TermMode};
use gpui::{
    App, ClipboardItem, Context, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, ScrollEvent, Styled, Window, div, px,
};
use gpui_component::ActiveTheme;

impl TerminalPane {
    /// The cell under `position` in `term`, and which half of it.
    fn cell_at<T>(
        &self,
        position: Point<Pixels>,
        term: &Term<T>,
        size: GridSize,
    ) -> Option<(
        alacritty_terminal::index::Point,
        alacritty_terminal::index::Side,
    )> {
        let (origin, cell) = self.geometry?;
        Some(select::cell_at(
            (f32::from(position.x), f32::from(position.y)),
            (f32::from(origin.x), f32::from(origin.y)),
            (f32::from(cell.width), f32::from(cell.height)),
            size,
            term.grid().display_offset(),
        ))
    }

    /// Starts a selection under the left button: dragging selects from
    /// there, a double click a word, a triple click a line (spec 0002,
    /// flow 4). Over a link, a click opens it as `open_links` says (flow
    /// 7).
    pub(super) fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ctrl = if cfg!(target_os = "macos") {
            event.modifiers.platform
        } else {
            event.modifiers.control
        };
        let others = event.modifiers.number_of_modifiers() > u8::from(ctrl);
        let focused = self.focus.is_focused(window);
        let press = link::press(
            self.settings.open_links,
            ctrl,
            others,
            event.click_count,
            focused,
        );
        let mut armed = None;
        if press != link::Press::Select
            && let Some(link) = self.link_under(event.position)
        {
            if press == link::Press::Open {
                // GPUI's URL opener, never a shell (TM-escape-03).
                cx.open_url(&link.target);
                return;
            }
            armed = Some(link);
        }
        let State::Running(running) = &self.state else {
            return;
        };
        let mut term = running.term.lock();
        let Some((point, side)) = self.cell_at(event.position, &term, running.size) else {
            return;
        };
        let kind = match event.click_count {
            1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        term.selection = Some(Selection::new(kind, point, side));
        drop(term);
        self.pressed_link = armed.map(|link| (link, point));
        self.selecting = true;
        cx.notify();
    }

    /// Drags a selection out, or with no button down, tracks the link under
    /// the pointer.
    pub(super) fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.pressed_button.is_none() {
            let link = self.link_under(event.position);
            if self.hovered_link != link {
                self.hovered_link = link;
                cx.notify();
            }
            return;
        }
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let State::Running(running) = &self.state else {
            return;
        };
        let mut term = running.term.lock();
        let Some((point, side)) = self.cell_at(event.position, &term, running.size) else {
            return;
        };
        if let Some(selection) = &mut term.selection {
            selection.update(point, side);
        }
        drop(term);
        // Leaving the pressed cell makes the click a drag, which selects.
        if self
            .pressed_link
            .as_ref()
            .is_some_and(|(_, pressed)| *pressed != point)
        {
            self.pressed_link = None;
        }
        cx.notify();
    }

    /// The link Tern would open under `position` (spec 0002, flow 7).
    fn link_under(&mut self, position: Point<Pixels>) -> Option<link::Link> {
        let State::Running(running) = &self.state else {
            return None;
        };
        let term = running.term.lock();
        let (point, _) = self.cell_at(position, &term, running.size)?;
        let urls = self.urls.get_or_insert_with(link::detector);
        link::at(&term, point, urls)
    }

    /// The pointer left the pane: no link is hovered.
    pub(super) fn hover_ended(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if !hovered && self.hovered_link.take().is_some() {
            cx.notify();
        }
    }

    /// Ends a selection: a click that selected nothing clears it; anything
    /// selected becomes the primary selection on Linux, and the clipboard
    /// with `copy_on_select`.
    pub(super) fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.selecting) {
            return;
        }
        // A click on a link, with `open_links: click` (spec 0002, flow 7).
        if let Some((link, _)) = self.pressed_link.take() {
            if let State::Running(running) = &self.state {
                running.term.lock().selection = None;
            }
            cx.open_url(&link.target);
            cx.notify();
            return;
        }
        let Some(text) = self.selected_text() else {
            if let State::Running(running) = &self.state {
                running.term.lock().selection = None;
            }
            cx.notify();
            return;
        };
        #[cfg(target_os = "linux")]
        cx.write_to_primary(ClipboardItem::new_string(text.clone()));
        if self.settings.copy_on_select {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// The button came up outside the pane: that was a drag, never a click
    /// on a link.
    pub(super) fn mouse_up_out(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pressed_link = None;
        self.mouse_up(event, window, cx);
    }

    /// A right click focuses the pane and pastes the clipboard into it, as
    /// Windows Terminal and PuTTY do (spec 0002, flow 4). The pane has no
    /// context menu, so the button is free.
    pub(super) fn right_click(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle(cx), cx);
        cx.emit(PaneEvent::Focused);
        TerminalPane::paste(self, cx);
    }

    /// Middle-click pastes the primary selection (Linux).
    pub(super) fn middle_click(
        &mut self,
        _: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        #[cfg(target_os = "linux")]
        if let Some(text) = cx.read_from_primary().and_then(|item| item.text()) {
            self.paste_text(&text);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = cx;
    }

    /// The selection as copy puts it on the clipboard, if anything is
    /// selected.
    pub(super) fn selected_text(&self) -> Option<String> {
        let State::Running(running) = &self.state else {
            return None;
        };
        let text = running.term.lock().selection_to_string()?;
        Some(select::copy_text(&text)).filter(|text| !text.is_empty())
    }

    /// Whether anything is selected, so "Copy" has something to copy.
    pub fn has_selection(&self) -> bool {
        self.selected_text().is_some()
    }

    /// Copies the selection to the clipboard (spec 0002, flow 4).
    pub fn copy(&self, cx: &mut App) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Pastes the clipboard into the program (spec 0002, flow 4).
    pub fn paste(&self, cx: &mut App) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.paste_text(&text);
        }
    }

    /// Sends `text` to the program as a paste (TM-escape-07). Nothing
    /// reaches a program that has exited, or one waiting on a prompt
    /// dialog: a pasted password would go to the remote shell.
    fn paste_text(&self, text: &str) {
        if self.exited || self.dialog.is_some() {
            return;
        }
        let State::Running(running) = &self.state else {
            return;
        };
        let bytes = {
            let mut term = running.term.lock();
            let bracketed = term.mode().contains(TermMode::BRACKETED_PASTE);
            term.scroll_display(Scroll::Bottom);
            select::paste_bytes(text, bracketed)
        };
        running.notifier.notify(bytes);
    }

    /// Scrolls through the scrollback, a line per wheel notch (spec 0002,
    /// flow 5). Where a full-screen program (less, vim, man) asked for it,
    /// the wheel sends arrow keys instead, so it scrolls itself.
    pub(super) fn scroll_wheel(
        &mut self,
        event: &ScrollEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, cell)) = self.geometry else {
            return;
        };
        let State::Running(running) = &self.state else {
            return;
        };
        let lines = f32::from(event.delta.pixel_delta(cell.height).y) / f32::from(cell.height)
            + self.scroll_remainder;
        self.scroll_remainder = lines.fract();
        let lines = lines.trunc() as i32;
        if lines == 0 {
            return;
        }
        let mut term = running.term.lock();
        let mode = *term.mode();
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let app_cursor = mode.contains(TermMode::APP_CURSOR);
            let key = match (lines > 0, app_cursor) {
                (true, true) => "\x1bOA",
                (true, false) => "\x1b[A",
                (false, true) => "\x1bOB",
                (false, false) => "\x1b[B",
            };
            drop(term);
            let keys = key.repeat(lines.unsigned_abs() as usize);
            running.notifier.notify(keys.into_bytes());
        } else {
            term.scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }

    /// Where the hovered link goes, over the pane's bottom left corner
    /// (TM-escape-02).
    pub(super) fn render_link_hint(&self, cx: &App) -> Option<impl IntoElement> {
        let link = self.hovered_link.as_ref()?;
        let theme = cx.theme();
        Some(
            div()
                .absolute()
                .bottom(px(4.))
                .left(px(4.))
                .max_w(gpui::relative(0.9))
                .px(px(6.))
                .py(px(2.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .text_color(theme.popover_foreground)
                .text_size(px(11.))
                .truncate()
                .child(link::hint(&link.target)),
        )
    }
}
