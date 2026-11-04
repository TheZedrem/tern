//! The window's content. One pane for now; tabs, splits and the title bar
//! arrive with T-003 (spec 0003).

use crate::terminal::{PaneEvent, TerminalPane};
use crate::theme::{self, Theme};
use crate::timing::{Mark, Timing};
use gpui::{
    Anchor, Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription, Window, div,
    px,
};
use gpui_component::WindowExt;
use gpui_component::notification::Notification;
use std::sync::Arc;

pub struct Workspace {
    pane: Entity<TerminalPane>,
    theme: Theme,
    timing: Arc<Timing>,
    bench: bool,
    /// Boot notices, shown once the first frame is on screen.
    notices: Vec<String>,
    /// TTFP for the dev badge, once known.
    ttfp: Option<f64>,
    _first_frame: Subscription,
}

impl Workspace {
    pub fn new(
        pane: Entity<TerminalPane>,
        theme: Theme,
        notices: Vec<String>,
        timing: Arc<Timing>,
        bench: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let first_frame =
            cx.subscribe_in(&pane, window, |this, _, event, window, cx| match event {
                PaneEvent::FirstFrame => this.first_frame(window, cx),
            });
        let focus = pane.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
        Self {
            pane,
            theme,
            timing,
            bench,
            notices,
            ttfp: None,
            _first_frame: first_frame,
        }
    }

    /// Records and prints TTFP (spec 0001), then runs what waits for the
    /// first frame. In bench mode, quits instead.
    fn first_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.timing.mark(Mark::Rendered);
        let report = self.timing.report();
        if let Ok(json) = serde_json::to_string(&report) {
            println!("{json}");
        }
        if self.bench {
            cx.quit();
            return;
        }
        self.ttfp = report.rendered;
        for notice in self.notices.drain(..) {
            let notice = Notification::new()
                .message(notice)
                .placement(Anchor::BottomRight);
            window.push_notification(notice, cx);
        }
        cx.notify();
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // The dev badge (spec 0001): every debug run shows its own TTFP.
        let badge = self.ttfp.filter(|_| cfg!(debug_assertions)).map(|ms| {
            div()
                .absolute()
                .top(px(8.))
                .right(px(12.))
                .px(px(8.))
                .py(px(2.))
                .rounded(px(4.))
                .bg(gpui::black().opacity(0.6))
                .text_size(px(12.))
                .text_color(gpui::white())
                .child(format!("TTFP {ms:.0} ms"))
        });
        div()
            .size_full()
            .relative()
            .bg(theme::hsla(self.theme.background))
            .text_color(theme::hsla(self.theme.foreground))
            .child(self.pane.clone())
            .children(badge)
    }
}
