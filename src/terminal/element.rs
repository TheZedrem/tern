//! Paints a pane's visible grid: cell backgrounds, then text locked to the
//! cell grid, then the cursor and what an input method composes there. Only
//! visible rows are shaped, and GPUI caches shaped lines, so unchanged rows
//! cost little per frame.

use super::grid;
use super::pty::{GridSize, PADDING, Terminal};
use super::{FontSettings, TerminalPane};
use crate::theme::Theme;
use alacritty_terminal::vte::ansi::{CursorShape, Rgb};
use gpui::{
    App, BorderStyle, Bounds, Element, ElementId, ElementInputHandler, Entity, FocusHandle, Font,
    FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId,
    PaintQuad, Pixels, Point, Rgba, ShapedLine, SharedString, StrikethroughStyle, Style, TextAlign,
    TextRun, UnderlineStyle, Window, fill, font, outline, point, px, relative, size,
};

pub struct TerminalElement {
    pane: Entity<TerminalPane>,
    term: Terminal,
    font: FontSettings,
    theme: Theme,
    focused: bool,
    /// Highlights, the cursor's blink and an input method's preedit.
    marks: grid::Marks,
    /// The grid's focus, which an input method types into.
    focus: FocusHandle,
}

impl TerminalElement {
    pub fn new(
        pane: Entity<TerminalPane>,
        term: Terminal,
        font: FontSettings,
        theme: Theme,
        focused: bool,
        marks: grid::Marks,
        focus: FocusHandle,
    ) -> Self {
        Self {
            pane,
            term,
            font,
            theme,
            focused,
            marks,
            focus,
        }
    }
}

/// Everything `paint` draws, prepared in `prepaint`.
pub struct Layout {
    line_height: Pixels,
    backgrounds: Vec<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<PaintQuad>,
    /// What an input method composes, over the cursor's cells.
    preedit: Option<(PaintQuad, Point<Pixels>, ShapedLine)>,
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Layout;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let cell = self.font.cell_size(window);
        let grid_size = GridSize::fit(
            (f32::from(bounds.size.width), f32::from(bounds.size.height)),
            (f32::from(cell.width), f32::from(cell.height)),
        );
        self.pane
            .update(cx, |pane, _| pane.lay_out(bounds.origin, cell, grid_size));

        let (frame, cursor_cell) = {
            let term = self.term.lock();
            let point = term.grid().cursor.point;
            let row = point.line.0 + term.grid().display_offset() as i32;
            let cursor_cell = usize::try_from(row).ok().map(|row| (row, point.column.0));
            let frame = grid::build(
                term.renderable_content(),
                &self.theme,
                self.focused,
                &self.marks,
            );
            (frame, cursor_cell)
        };

        let origin = bounds.origin + point(px(PADDING), px(PADDING));
        let at = |row: usize, col: usize| {
            origin + point(cell.width * col as f32, cell.height * row as f32)
        };

        let backgrounds = frame
            .backgrounds
            .iter()
            .map(|span| {
                let bounds = Bounds::new(
                    at(span.row, span.col),
                    size(cell.width * span.width as f32, cell.height),
                );
                fill(bounds, hsla(span.color))
            })
            .collect();

        let lines = frame
            .runs
            .into_iter()
            .map(|run| {
                let color = hsla(run.style.color);
                let font = Font {
                    weight: if run.style.bold {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    },
                    style: if run.style.italic {
                        FontStyle::Italic
                    } else {
                        FontStyle::Normal
                    },
                    ..font(self.font.family.clone())
                };
                let text: SharedString = run.text.into();
                let text_run = TextRun {
                    len: text.len(),
                    font,
                    color,
                    background_color: None,
                    underline: run.style.underline.then_some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(color),
                        wavy: false,
                    }),
                    strikethrough: run.style.strikeout.then_some(StrikethroughStyle {
                        thickness: px(1.),
                        color: Some(color),
                    }),
                };
                // Locks every glyph to the cell grid, so a glyph from a
                // fallback font can't shift the rest of the row.
                let force_width = (!run.wide).then_some(cell.width);
                let line =
                    window
                        .text_system()
                        .shape_line(text, self.font.size, &[text_run], force_width);
                (at(run.row, run.col), line)
            })
            .collect();

        let cursor = frame.cursor.map(|cursor| {
            let origin = at(cursor.row, cursor.col);
            let width = cell.width * cursor.width as f32;
            let color = hsla(cursor.color);
            match cursor.shape {
                CursorShape::Beam => fill(Bounds::new(origin, size(px(2.), cell.height)), color),
                CursorShape::Underline => fill(
                    Bounds::new(
                        origin + point(px(0.), cell.height - px(2.)),
                        size(width, px(2.)),
                    ),
                    color,
                ),
                _ => outline(
                    Bounds::new(origin, size(width, cell.height)),
                    color,
                    BorderStyle::Solid,
                ),
            }
        });

        // Underlined over the cursor's cells, on the background, until the
        // input method commits it.
        let preedit = self
            .marks
            .preedit
            .clone()
            .zip(cursor_cell)
            .map(|(text, (row, col))| {
                let origin = at(row, col);
                let color = hsla(super::colors::rgb(self.theme.foreground));
                let text: SharedString = text.into();
                let run = TextRun {
                    len: text.len(),
                    font: font(self.font.family.clone()),
                    color,
                    background_color: None,
                    underline: Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(color),
                        wavy: false,
                    }),
                    strikethrough: None,
                };
                let line = window
                    .text_system()
                    .shape_line(text, self.font.size, &[run], None);
                let width = line.width.max(cell.width);
                let background = hsla(super::colors::rgb(self.theme.background));
                let quad = fill(Bounds::new(origin, size(width, cell.height)), background);
                (quad, origin, line)
            });

        Layout {
            line_height: cell.height,
            backgrounds,
            lines,
            cursor,
            preedit,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // While the grid has the keyboard, input methods type into the pane.
        window.handle_input(
            &self.focus,
            ElementInputHandler::new(bounds, self.pane.clone()),
            cx,
        );
        for quad in layout.backgrounds.drain(..) {
            window.paint_quad(quad);
        }
        for (origin, line) in &layout.lines {
            // A failed glyph raster only loses that run for this frame.
            let _ = line.paint(
                *origin,
                layout.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
        if let Some(cursor) = layout.cursor.take() {
            window.paint_quad(cursor);
        }
        if let Some((background, origin, line)) = layout.preedit.take() {
            window.paint_quad(background);
            let _ = line.paint(
                origin,
                layout.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
    }
}

fn hsla(color: Rgb) -> Hsla {
    Rgba {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: 1.0,
    }
    .into()
}
