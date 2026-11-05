//! Paints a pane's visible grid: cell backgrounds, then text locked to the
//! cell grid, then the cursor. Only visible rows are shaped, and GPUI caches
//! shaped lines, so unchanged rows cost little per frame.

use super::grid;
use super::pty::{GridSize, PADDING, Terminal};
use super::{FontSettings, TerminalPane};
use crate::theme::Theme;
use alacritty_terminal::vte::ansi::{CursorShape, Rgb};
use gpui::{
    App, BorderStyle, Bounds, Element, ElementId, Entity, Font, FontStyle, FontWeight,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels, Point,
    Rgba, ShapedLine, SharedString, StrikethroughStyle, Style, TextAlign, TextRun, UnderlineStyle,
    Window, fill, font, outline, point, px, relative, size,
};

pub struct TerminalElement {
    pane: Entity<TerminalPane>,
    term: Terminal,
    font: FontSettings,
    theme: Theme,
    focused: bool,
    /// Search matches to highlight.
    marks: grid::Marks,
}

impl TerminalElement {
    pub fn new(
        pane: Entity<TerminalPane>,
        term: Terminal,
        font: FontSettings,
        theme: Theme,
        focused: bool,
        marks: grid::Marks,
    ) -> Self {
        Self {
            pane,
            term,
            font,
            theme,
            focused,
            marks,
        }
    }
}

/// Everything `paint` draws, prepared in `prepaint`.
pub struct Layout {
    line_height: Pixels,
    backgrounds: Vec<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<PaintQuad>,
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

        let frame = {
            let term = self.term.lock();
            grid::build(
                term.renderable_content(),
                &self.theme,
                self.focused,
                &self.marks,
            )
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

        Layout {
            line_height: cell.height,
            backgrounds,
            lines,
            cursor,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
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
