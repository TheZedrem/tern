//! A sidebar panel that follows the window's active pane (spec 0018,
//! Model): the Files panel and the Port forwards panel are each one of
//! these. It draws what it's handed for that pane and owns no session:
//! browsers and forwards stay the panes'. What to draw changes with the
//! active pane, never by a stale one (TM-sftp-15).

use gpui::{
    AnyView, App, ClickEvent, Context, EventEmitter, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Sizable};

/// What the panel draws for the active pane.
pub enum Body {
    /// Nothing to show for this pane, and why.
    Nothing(SharedString),
    /// An SSH pane whose files weren't asked for yet: its `user@host` and
    /// the "Show files" button. Nothing connects until it's clicked.
    Offer(SharedString),
    /// The pane's own view, and where the keyboard goes in it.
    View { view: AnyView, focus: FocusHandle },
}

impl Body {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Body::Nothing(a), Body::Nothing(b)) | (Body::Offer(a), Body::Offer(b)) => a == b,
            (Body::View { view: a, .. }, Body::View { view: b, .. }) => {
                a.entity_id() == b.entity_id()
            }
            _ => false,
        }
    }
}

pub enum PanePanelEvent {
    /// "Show files": connect to the active pane's host.
    Connect,
}

pub struct PanePanel {
    focus: FocusHandle,
    body: Body,
}

impl EventEmitter<PanePanelEvent> for PanePanel {}

impl PanePanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            body: Body::Nothing(SharedString::default()),
        }
    }

    /// Draws `body` from now on. Redraws only if it differs.
    pub fn set_body(&mut self, body: Body, cx: &mut Context<Self>) {
        if !self.body.same(&body) {
            self.body = body;
            cx.notify();
        }
    }

    /// Puts the keyboard in what's shown.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = match &self.body {
            Body::View { focus, .. } => focus,
            _ => &self.focus,
        };
        window.focus(handle, cx);
    }
}

impl Render for PanePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let body = match &self.body {
            Body::Nothing(why) => div()
                .px(px(8.))
                .py(px(8.))
                .text_sm()
                .text_color(muted)
                .child(why.clone())
                .into_any_element(),
            Body::Offer(host) => div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .px(px(8.))
                .py(px(8.))
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(host.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("Its files aren't shown yet: showing them connects to the host."),
                )
                .child(
                    Button::new("sidebar-show-files")
                        .label("Show files")
                        .primary()
                        .small()
                        .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                            cx.emit(PanePanelEvent::Connect);
                        })),
                )
                .into_any_element(),
            Body::View { view, .. } => view.clone().into_any_element(),
        };
        div()
            .track_focus(&self.focus)
            // As the sidebar's own surface: a press on the panel's text
            // leaves the keyboard with the shell.
            .on_any_mouse_down(|_, window, _| window.prevent_default())
            .size_full()
            .child(body)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use gpui::{AppContext, Entity, EntityId, Modifiers, TestAppContext, point};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// What the panel draws, as a test compares it. Shared with the tests
    /// of the workspace, which hands the panel its body.
    #[derive(Debug, PartialEq)]
    pub(crate) enum Drawn {
        Nothing(String),
        Offer(String),
        View(EntityId),
    }

    impl PanePanel {
        pub(crate) fn drawn(&self) -> Drawn {
            match &self.body {
                Body::Nothing(why) => Drawn::Nothing(why.to_string()),
                Body::Offer(host) => Drawn::Offer(host.to_string()),
                Body::View { view, .. } => Drawn::View(view.entity_id()),
            }
        }
    }

    /// A pane's own view, like its file browser: it takes the keyboard.
    struct PaneView {
        focus: FocusHandle,
    }

    impl Render for PaneView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().track_focus(&self.focus).size_full()
        }
    }

    /// The panel in a test window, and the events it has emitted:
    /// `"connect"`.
    fn panel(
        cx: &mut TestAppContext,
    ) -> (
        Entity<PanePanel>,
        Rc<RefCell<Vec<&'static str>>>,
        &mut gpui::VisualTestContext,
    ) {
        cx.update(gpui_component::init);
        let made = Rc::new(RefCell::new(None));
        let (_, cx) = cx.add_window_view({
            let made = made.clone();
            move |window, cx| {
                let panel = cx.new(PanePanel::new);
                *made.borrow_mut() = Some(panel.clone());
                gpui_component::Root::new(panel, window, cx)
            }
        });
        let panel = made.take().expect("the window built the panel");
        let seen = Rc::new(RefCell::new(Vec::new()));
        cx.update(|_, cx| {
            let seen = seen.clone();
            cx.subscribe(&panel, move |_, event: &PanePanelEvent, _| {
                seen.borrow_mut().push(match event {
                    PanePanelEvent::Connect => "connect",
                });
            })
            .detach();
        });
        (panel, seen, cx)
    }

    /// Spec 0018, flow 2: `sidebar.focus` on Files puts the keyboard in
    /// what the panel shows for the active pane, its listing; with nothing
    /// of the pane's to show, the panel itself takes it, so Esc still
    /// gives it back.
    #[gpui::test]
    fn the_keyboard_goes_to_the_panes_view_or_to_the_panel_without_one(cx: &mut TestAppContext) {
        let (panel, _, cx) = panel(cx);
        let own = cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_body(Body::Offer("me@prod".into()), cx);
                panel.focus(window, cx);
                panel.focus.clone()
            })
        });
        assert!(cx.update(|window, _| own.is_focused(window)));

        let listing = cx.update(|window, cx| {
            let view = cx.new(|cx| PaneView {
                focus: cx.focus_handle(),
            });
            let listing = view.read(cx).focus.clone();
            panel.update(cx, |panel, cx| {
                let body = Body::View {
                    view: view.into(),
                    focus: listing.clone(),
                };
                panel.set_body(body, cx);
                panel.focus(window, cx);
            });
            listing
        });
        assert!(cx.update(|window, _| listing.is_focused(window)));
    }

    /// Spec 0018, Files panel [load-bearing] and AC2: an SSH pane without
    /// a session is shown as its `user@host` with "Show files". Drawing
    /// that asks for no connection; only a click on the button does. The
    /// button carries no test selector, so the panel is clicked from the
    /// top down until the button is hit.
    #[gpui::test]
    fn the_offer_connects_nothing_until_show_files_is_clicked(cx: &mut TestAppContext) {
        let (panel, seen, cx) = panel(cx);
        panel.update(cx, |panel, cx| {
            panel.set_body(Body::Offer("me@prod.example.com".into()), cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(
            panel.read_with(cx, |panel, _| panel.drawn()),
            Drawn::Offer("me@prod.example.com".into())
        );
        assert!(seen.borrow().is_empty(), "{:?}", seen.borrow());

        for step in 0..60 {
            if seen.borrow().contains(&"connect") {
                break;
            }
            let at = point(px(30.0), px(4.0 + 5.0 * step as f32));
            cx.simulate_click(at, Modifiers::none());
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        let connects = seen.borrow().iter().filter(|e| **e == "connect").count();
        assert_eq!(connects, 1, "{:?}", seen.borrow());
    }

    /// A pane's view, as the workspace hands it to the panel.
    fn pane_view(cx: &mut gpui::VisualTestContext) -> (Body, EntityId) {
        cx.update(|_, cx| {
            let view = cx.new(|cx| PaneView {
                focus: cx.focus_handle(),
            });
            let (id, focus) = (view.entity_id(), view.read(cx).focus.clone());
            let body = Body::View {
                view: view.into(),
                focus,
            };
            (body, id)
        })
    }

    /// Spec 0018, Files panel and Port forwards panel [load-bearing]
    /// (TM-sftp-15, TM-fwd-07): the panel draws what it was handed for the
    /// active pane, never what it drew for the pane before. Another
    /// pane's view, an offer or a text replaces a view at once, and a
    /// view replaces them. Being handed a view asks for no connection.
    #[gpui::test]
    fn the_panel_draws_only_what_it_was_handed_last(cx: &mut TestAppContext) {
        let (panel, seen, cx) = panel(cx);
        let (first, first_id) = pane_view(cx);
        let (second, second_id) = pane_view(cx);
        let (third, third_id) = pane_view(cx);
        assert_ne!(first_id, second_id);
        let steps = [
            (first, Drawn::View(first_id)),
            (second, Drawn::View(second_id)),
            (
                Body::Offer("me@prod".into()),
                Drawn::Offer("me@prod".into()),
            ),
            (
                Body::Offer("deploy@stage".into()),
                Drawn::Offer("deploy@stage".into()),
            ),
            (third, Drawn::View(third_id)),
            (
                Body::Nothing("no browser here".into()),
                Drawn::Nothing("no browser here".into()),
            ),
        ];
        for (body, expected) in steps {
            panel.update(cx, |panel, cx| panel.set_body(body, cx));
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(panel.read_with(cx, |panel, _| panel.drawn()), expected);
        }
        assert!(seen.borrow().is_empty(), "{:?}", seen.borrow());
    }
}
