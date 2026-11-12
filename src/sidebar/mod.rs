//! The window's sidebar (spec 0018): one column that hosts panels. It
//! draws the strip of panel buttons, the shown panel's header and its
//! inner edge, and keeps what the session saves: open, width and the panel
//! shown. What a panel is and does is the panel's own business; every one
//! is registered through [`Sidebar::register`], built in or not.
//!
//! The keyboard stays with the shell [load-bearing]: nothing here takes
//! focus except [`Sidebar::focus`] and a click inside a panel.

pub mod hosts;
pub mod pane;
mod panel;
pub mod plugin;

use crate::config::SidebarSide;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, ClickEvent, Context, DragMoveEvent, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, actions, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, IconName, Selectable, Sizable};
pub use panel::{Panel, PanelId};
use std::time::Duration;

/// The width until the edge is dragged, and the least it can be dragged
/// to; the most is half the window (spec 0018, Model).
pub const WIDTH: f32 = 260.0;
pub const MIN_WIDTH: f32 = 200.0;
/// The inner edge it's resized by, as wide as a split's divider.
const EDGE: f32 = 4.0;
/// How long clicks are ignored after something other than the user changed
/// what is under the pointer (TM-plug-23).
pub const CLICK_GUARD: Duration = Duration::from_millis(500);

/// The sidebar's key context: plain keys bound here exist only while the
/// keyboard is inside it.
const CONTEXT: &str = "Sidebar";
const STRIP_CONTEXT: &str = "SidebarStrip";

actions!(sidebar, [Leave, FocusNextControl, FocusPreviousControl]);

/// Binds the sidebar's keys and its panels'.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", Leave, Some(CONTEXT)),
        KeyBinding::new("tab", FocusNextControl, Some(CONTEXT)),
        KeyBinding::new("shift-tab", FocusPreviousControl, Some(CONTEXT)),
        KeyBinding::new("right", FocusNextControl, Some(STRIP_CONTEXT)),
        KeyBinding::new("left", FocusPreviousControl, Some(STRIP_CONTEXT)),
    ]);
    hosts::init(cx);
    plugin::init(cx);
}

/// The width a sidebar `width` wide is drawn with in a window
/// `window_width` wide: at most half of it, never under [`MIN_WIDTH`].
pub fn fitted(width: f32, window_width: f32) -> f32 {
    width.clamp(MIN_WIDTH, (window_width / 2.0).max(MIN_WIDTH))
}

pub enum SidebarEvent {
    /// `panel` is on screen now. `by_user` if the user's own act put it
    /// there (the toggle, a strip button), rather than a panel leaving or
    /// an `open_panel`.
    Shown { panel: PanelId, by_user: bool },
    /// `panel` left the screen: another one took over, or the sidebar
    /// closed.
    Hidden(PanelId),
    /// Open, width or the shown panel changed: the session saves them and
    /// the window lays out again.
    Changed,
    /// The keyboard goes back to the active pane.
    Leave,
}

/// Dragging the inner edge to resize the sidebar.
#[derive(Clone)]
struct DraggedEdge;

/// What a dragged edge draws under the pointer: nothing.
struct Ghost;

impl Render for Ghost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub struct Sidebar {
    focus: FocusHandle,
    panels: Vec<Panel>,
    shown: PanelId,
    open: bool,
    width: f32,
    side: SidebarSide,
    /// Running while clicks are ignored (see [`Self::show_unasked`]).
    guard: Option<Task<()>>,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

impl Sidebar {
    pub fn new(
        open: bool,
        width: f32,
        shown: PanelId,
        side: SidebarSide,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            panels: Vec::new(),
            shown,
            open,
            // A saved width is a file's: never under the least.
            width: width.max(MIN_WIDTH),
            side,
            guard: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn width(&self) -> f32 {
        self.width
    }

    pub fn shown(&self) -> &PanelId {
        &self.shown
    }

    /// Whether panel `id` is registered.
    pub fn has(&self, id: &PanelId) -> bool {
        self.panels.iter().any(|panel| panel.id == *id)
    }

    /// Whether `panel` is on screen.
    pub fn shows(&self, panel: &PanelId) -> bool {
        self.open && self.shown == *panel
    }

    pub fn set_side(&mut self, side: SidebarSide, cx: &mut Context<Self>) {
        self.side = side;
        cx.notify();
    }

    /// Whether the keyboard is in the sidebar.
    pub fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx)
    }

    /// Adds `panel` in its place (spec 0018, Order): built-in panels in
    /// the order they register, then plugins' by plugin name, ignoring
    /// case, each plugin's in the order they register. A plugin can't
    /// choose its place.
    pub fn register(&mut self, panel: Panel, cx: &mut Context<Self>) {
        let key = |panel: &Panel| panel.plugin.as_ref().map(|name| name.to_lowercase());
        let new = key(&panel);
        let at = self
            .panels
            .iter()
            .position(|other| key(other) > new)
            .unwrap_or(self.panels.len());
        self.panels.insert(at, panel);
        cx.notify();
    }

    /// Takes panel `id` out. If it was the one shown, the first panel is.
    pub fn remove(&mut self, id: &PanelId, cx: &mut Context<Self>) {
        self.panels.retain(|panel| panel.id != *id);
        if self.shown == *id
            && let Some(first) = self.panels.first().map(|panel| panel.id.clone())
        {
            self.switch(first, false, cx);
        }
        cx.notify();
    }

    /// Shows panel `id`, opening the sidebar if it's closed, without
    /// moving the keyboard. Not the user's own act: see [`Self::pick`].
    pub fn show(&mut self, id: &PanelId, cx: &mut Context<Self>) {
        if !self.open {
            self.open = true;
            self.shown = id.clone();
            self.announce(false, cx);
        } else {
            self.switch(id.clone(), false, cx);
        }
    }

    /// [`Self::show`] for a panel that asked for it itself (spec 0018,
    /// flow 6): if that changes what the sidebar shows, the sidebar ignores
    /// clicks for [`CLICK_GUARD`], so a panel can't come up under a click
    /// meant for another.
    pub fn show_unasked(&mut self, id: &PanelId, cx: &mut Context<Self>) {
        if self.shows(id) {
            return;
        }
        self.show(id, cx);
        let wait = cx.background_executor().timer(CLICK_GUARD);
        self.guard = Some(cx.spawn(async move |this, cx| {
            wait.await;
            let _ = this.update(cx, |this, cx| {
                this.guard = None;
                cx.notify();
            });
        }));
    }

    /// Whether clicks are ignored right now.
    pub fn ignores_clicks(&self) -> bool {
        self.guard.is_some()
    }

    /// Opens the sidebar on the panel it showed last, or closes it (spec
    /// 0018, flow 1). The keyboard stays where it was, unless it was
    /// inside: then it goes back to the pane.
    pub fn toggle(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.open {
            let inside = self.has_focus(window, cx);
            self.open = false;
            cx.emit(SidebarEvent::Hidden(self.shown.clone()));
            cx.emit(SidebarEvent::Changed);
            if inside {
                cx.emit(SidebarEvent::Leave);
            }
            cx.notify();
        } else {
            self.open = true;
            self.announce(true, cx);
        }
    }

    /// Opens the sidebar on the panel it showed last, by the user's own
    /// act, if it's closed.
    pub fn open(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            self.open = true;
            self.announce(true, cx);
        }
    }

    /// Puts the keyboard in the shown panel (spec 0018, flow 2).
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = self.panels.iter().find(|panel| panel.id == self.shown) {
            let focus = panel.focus.clone();
            focus(window, cx);
        }
    }

    /// A strip button: shows panel `id`. The keyboard stays in the pane
    /// if it was there, and moves to the new panel if it was in the old
    /// one (spec 0018, flow 3).
    fn pick(&mut self, id: PanelId, window: &mut Window, cx: &mut Context<Self>) {
        if self.ignores_clicks() {
            return;
        }
        let inside = self.has_focus(window, cx);
        self.switch(id, true, cx);
        if inside {
            self.focus(window, cx);
        }
    }

    fn switch(&mut self, id: PanelId, by_user: bool, cx: &mut Context<Self>) {
        if self.shown == id {
            return;
        }
        let old = std::mem::replace(&mut self.shown, id);
        if self.open {
            cx.emit(SidebarEvent::Hidden(old));
        }
        self.announce(by_user, cx);
    }

    /// Says that the shown panel is on screen now, if the sidebar is open.
    fn announce(&mut self, by_user: bool, cx: &mut Context<Self>) {
        if self.open {
            cx.emit(SidebarEvent::Shown {
                panel: self.shown.clone(),
                by_user,
            });
        }
        cx.emit(SidebarEvent::Changed);
        cx.notify();
    }

    /// Sets the width from the mouse: from the outer edge, which stays
    /// put.
    fn drag_edge(
        &mut self,
        event: &DragMoveEvent<DraggedEdge>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (bounds, x) = (event.bounds, event.event.position.x);
        let width = match self.side {
            SidebarSide::Left => x - bounds.left(),
            SidebarSide::Right => bounds.right() - x,
        };
        let width = fitted(f32::from(width), f32::from(window.viewport_size().width));
        if width != self.width {
            self.width = width;
            cx.emit(SidebarEvent::Changed);
            cx.notify();
        }
    }

    /// The row of panel buttons, the shown one's selected (spec 0018,
    /// Strip).
    fn render_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let buttons = self.panels.iter().enumerate().map(|(ix, panel)| {
            let id = panel.id.clone();
            let tooltip: SharedString = match &panel.plugin {
                Some(plugin) => format!("{} (Plugin {plugin})", panel.title).into(),
                None => panel.title.clone(),
            };
            Button::new(("sidebar-panel", ix))
                .icon(panel.icon.clone())
                .ghost()
                .small()
                .selected(panel.id == self.shown)
                .tooltip(tooltip)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.pick(id.clone(), window, cx);
                }))
        });
        div()
            .key_context(STRIP_CONTEXT)
            .flex()
            .flex_wrap()
            .gap(px(2.))
            .px(px(6.))
            .py(px(4.))
            .children(buttons)
    }

    /// The shown panel's title and the close button; a plugin's panel
    /// always carries its `Plugin <name>` line, a built-in one never does
    /// (spec 0018, Header; TM-plug-01).
    fn render_header(&self, panel: &Panel, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let close = Button::new("sidebar-close")
            .icon(IconName::Close)
            .ghost()
            .xsmall()
            .tooltip("Close sidebar")
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                if !this.ignores_clicks() {
                    this.toggle(window, cx);
                }
            }));
        div()
            .flex()
            .items_start()
            .gap(px(4.))
            .px(px(8.))
            .py(px(4.))
            .border_t_1()
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(panel.title.clone()),
                    )
                    .when_some(panel.plugin.clone(), |header, plugin| {
                        header.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(format!("Plugin {plugin}")),
                        )
                    }),
            )
            .child(close)
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (background, border) = (theme.tab_bar, theme.border);
        let edge = div()
            .id("sidebar-edge")
            .w(px(EDGE))
            .h_full()
            .flex_shrink_0()
            .bg(border)
            .cursor_col_resize()
            .on_drag(DraggedEdge, |_, _, _, cx| cx.new(|_| Ghost));
        let shown = self.panels.iter().find(|panel| panel.id == self.shown);
        let body = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(self.render_strip(cx))
            .children(shown.map(|panel| self.render_header(panel, cx)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .children(shown.map(|panel| panel.view.clone())),
            );
        let side = self.side;
        let width = fitted(self.width, f32::from(window.viewport_size().width));
        div()
            .id("sidebar")
            .relative()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            // A press on the sidebar's own surface must not take the
            // keyboard from the shell: without this, GPUI focuses a
            // focus-tracked element on mouse down. What takes keys inside
            // handles its press first.
            .on_any_mouse_down(|_, window, _| window.prevent_default())
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .bg(background)
            .on_action(cx.listener(|_, _: &Leave, _, cx| cx.emit(SidebarEvent::Leave)))
            .on_action(cx.listener(|_, _: &FocusNextControl, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &FocusPreviousControl, window, cx| {
                window.focus_prev(cx);
            }))
            .map(|sidebar| match side {
                SidebarSide::Left => sidebar.child(body).child(edge),
                SidebarSide::Right => sidebar.child(edge).child(body),
            })
            // Over everything while clicks are ignored, so none reaches a
            // panel either.
            .when(self.ignores_clicks(), |sidebar| {
                sidebar.child(div().absolute().inset_0().occlude())
            })
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<DraggedEdge>, window, cx| {
                    this.drag_edge(event, window, cx);
                }),
            )
    }
}

/// The column the first frame draws where a sidebar saved open will be,
/// before the sidebar exists, so no pane is resized after it (spec 0018,
/// Startup impact).
pub fn placeholder(width: f32, cx: &App) -> impl IntoElement {
    div()
        .w(px(width))
        .h_full()
        .flex_shrink_0()
        .bg(cx.theme().tab_bar)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A sidebar event, as a test compares it.
    #[derive(Clone, Debug, PartialEq)]
    enum Seen {
        Shown(PanelId, bool),
        Hidden(PanelId),
        Changed,
        Leave,
    }

    impl Sidebar {
        /// The strip's panels, in its order. Shared with the tests of the
        /// workspace, which registers the built-in panels.
        pub(crate) fn order(&self) -> Vec<PanelId> {
            self.panels.iter().map(|panel| panel.id.clone()).collect()
        }
    }

    /// A panel's body: something that takes the keyboard.
    struct Body {
        focus: FocusHandle,
    }

    impl Render for Body {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().track_focus(&self.focus).size_full()
        }
    }

    /// Stands in for the workspace: a pane that holds the keyboard, with
    /// the sidebar on its right while it's open.
    struct Host {
        pane: FocusHandle,
        sidebar: Entity<Sidebar>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let open = self.sidebar.read(cx).is_open();
            div()
                .size_full()
                .flex()
                .child(div().track_focus(&self.pane).flex_1().h_full())
                .when(open, |host| host.child(self.sidebar.clone()))
        }
    }

    /// A closed sidebar in a test window beside a pane that has the
    /// keyboard, and every event the sidebar emits.
    struct Rig<'a> {
        sidebar: Entity<Sidebar>,
        pane: FocusHandle,
        seen: Rc<RefCell<Vec<Seen>>>,
        cx: &'a mut VisualTestContext,
    }

    /// A plugin panel's ID.
    fn plugin(plugin: &str, panel: &str) -> PanelId {
        PanelId::Plugin {
            plugin: plugin.into(),
            panel: panel.into(),
        }
    }

    /// A sidebar without panels, closed, 260 px wide, on the right.
    fn bare(cx: &mut TestAppContext) -> Rig<'_> {
        cx.update(|cx| {
            gpui_component::init(cx);
            init(cx);
        });
        let made = Rc::new(RefCell::new(None));
        let (_, cx) = cx.add_window_view({
            let made = made.clone();
            move |window, cx| {
                let host = cx.new(|cx| Host {
                    pane: cx.focus_handle(),
                    sidebar: cx.new(|cx| {
                        Sidebar::new(false, WIDTH, PanelId::Files, SidebarSide::Right, cx)
                    }),
                });
                *made.borrow_mut() = Some(host.clone());
                gpui_component::Root::new(host, window, cx)
            }
        });
        let host = made.take().expect("the window built the host");
        let seen = Rc::new(RefCell::new(Vec::new()));
        let (sidebar, pane) = cx.update(|window, cx| {
            let (sidebar, pane) = {
                let host = host.read(cx);
                (host.sidebar.clone(), host.pane.clone())
            };
            window.focus(&pane, cx);
            let seen = seen.clone();
            cx.subscribe(&sidebar, move |_, event: &SidebarEvent, _| {
                seen.borrow_mut().push(match event {
                    SidebarEvent::Shown { panel, by_user } => Seen::Shown(panel.clone(), *by_user),
                    SidebarEvent::Hidden(panel) => Seen::Hidden(panel.clone()),
                    SidebarEvent::Changed => Seen::Changed,
                    SidebarEvent::Leave => Seen::Leave,
                });
            })
            .detach();
            (sidebar, pane)
        });
        let mut rig = Rig {
            sidebar,
            pane,
            seen,
            cx,
        };
        rig.draw();
        rig
    }

    /// A sidebar with the two built-in panels registered the way the
    /// workspace registers them, and where the keyboard goes in each.
    fn rig(cx: &mut TestAppContext) -> (Rig<'_>, FocusHandle, FocusHandle) {
        let mut rig = bare(cx);
        let files = rig.register(PanelId::Files, "Files", None);
        let hosts = rig.register(PanelId::Hosts, "SSH hosts", None);
        rig.seen();
        (rig, files, hosts)
    }

    impl Rig<'_> {
        fn draw(&mut self) {
            self.cx.update(|window, cx| window.draw(cx).clear(cx));
        }

        /// Registers a panel through the one call every panel uses, and
        /// returns the handle `sidebar.focus` puts the keyboard on.
        fn register(
            &mut self,
            id: PanelId,
            title: &'static str,
            plugin: Option<&'static str>,
        ) -> FocusHandle {
            let sidebar = self.sidebar.clone();
            let handle = self.cx.update(|_, cx| {
                let body = cx.new(|cx| Body {
                    focus: cx.focus_handle(),
                });
                let handle = body.read(cx).focus.clone();
                let focus = handle.clone();
                let panel = Panel {
                    id,
                    title: title.into(),
                    icon: IconName::Folder.into(),
                    plugin: plugin.map(Into::into),
                    view: body.into(),
                    focus: Rc::new(move |window, cx| {
                        window.focus(&focus, cx);
                    }),
                };
                sidebar.update(cx, |sidebar, cx| sidebar.register(panel, cx));
                handle
            });
            self.draw();
            handle
        }

        /// Runs `act` on the sidebar, then draws what it left.
        fn act<R>(
            &mut self,
            act: impl FnOnce(&mut Sidebar, &mut Window, &mut Context<Sidebar>) -> R,
        ) -> R {
            let sidebar = self.sidebar.clone();
            let result = self
                .cx
                .update(|window, cx| sidebar.update(cx, |sidebar, cx| act(sidebar, window, cx)));
            self.cx.run_until_parked();
            self.draw();
            result
        }

        /// The events since the last call.
        fn seen(&mut self) -> Vec<Seen> {
            std::mem::take(&mut *self.seen.borrow_mut())
        }

        /// The panels coming and going since the last call, without the
        /// `Changed` and `Leave` between them.
        fn shown_and_hidden(&mut self) -> Vec<Seen> {
            let mut seen = self.seen();
            seen.retain(|event| matches!(event, Seen::Shown(..) | Seen::Hidden(_)));
            seen
        }

        /// The strip's panels, in its order.
        fn order(&mut self) -> Vec<PanelId> {
            self.act(|sidebar, _, _| sidebar.order())
        }

        fn shown(&mut self) -> PanelId {
            self.act(|sidebar, _, _| sidebar.shown().clone())
        }

        fn is_open(&mut self) -> bool {
            self.act(|sidebar, _, _| sidebar.is_open())
        }

        /// Whether the keyboard is in the pane, where the shell is.
        fn pane_has_the_keyboard(&mut self) -> bool {
            let pane = self.pane.clone();
            self.cx.update(|window, _| pane.is_focused(window))
        }

        /// Whether the keyboard is anywhere in the sidebar.
        fn sidebar_has_the_keyboard(&mut self) -> bool {
            self.act(|sidebar, window, cx| sidebar.has_focus(window, cx))
        }

        fn has_the_keyboard(&mut self, handle: &FocusHandle) -> bool {
            self.cx.update(|window, _| handle.is_focused(window))
        }

        /// The window's width, in pixels.
        fn window_width(&mut self) -> f32 {
            self.cx
                .update(|window, _| f32::from(window.viewport_size().width))
        }
    }

    /// Spec 0018, Width: 260 px until dragged, never under 200 px nor over
    /// half the window, and clamped again when the window shrinks. In a
    /// window too narrow for both, 200 px stands.
    #[test]
    fn the_width_is_fitted_between_200_px_and_half_the_window() {
        assert_eq!(WIDTH, 260.0);
        assert_eq!(fitted(WIDTH, 1200.0), 260.0);
        assert_eq!(fitted(320.0, 1200.0), 320.0);
        assert_eq!(fitted(120.0, 1200.0), 200.0);
        assert_eq!(fitted(900.0, 1200.0), 600.0);
        // The same sidebar as its window shrinks.
        assert_eq!(fitted(480.0, 1000.0), 480.0);
        assert_eq!(fitted(480.0, 800.0), 400.0);
        assert_eq!(fitted(480.0, 300.0), 200.0);
    }

    /// Spec 0018, Order [load-bearing], AC8 and AC9: Files, SSH hosts,
    /// then plugin panels by plugin name ignoring case, each plugin's in
    /// the order it registers them, whatever order the registrations come
    /// in. A plugin can't get ahead of a built-in panel or another plugin
    /// by registering first.
    #[gpui::test]
    fn panels_are_listed_built_ins_first_then_plugins_by_name_ignoring_case(
        cx: &mut TestAppContext,
    ) {
        let mut rig = bare(cx);
        rig.register(plugin("Zeta", "one"), "One", Some("Zeta"));
        rig.register(PanelId::Files, "Files", None);
        rig.register(plugin("alpha", "second"), "B", Some("alpha"));
        rig.register(PanelId::Hosts, "SSH hosts", None);
        rig.register(plugin("Zeta", "two"), "Two", Some("Zeta"));
        rig.register(plugin("alpha", "first"), "A", Some("alpha"));
        rig.register(plugin("Beta", "only"), "Only", Some("Beta"));
        assert_eq!(
            rig.order(),
            [
                PanelId::Files,
                PanelId::Hosts,
                plugin("alpha", "second"),
                plugin("alpha", "first"),
                plugin("Beta", "only"),
                plugin("Zeta", "one"),
                plugin("Zeta", "two"),
            ]
        );
    }

    /// Spec 0018, Order [load-bearing], flow 5 and AC4 (T-084): Port
    /// forwards comes and goes with the active pane, so it registers
    /// after plugins' panels have. It is still listed after SSH hosts and
    /// before every plugin's panel, each time it comes back.
    #[gpui::test]
    fn a_built_in_panel_that_registers_late_is_listed_before_the_plugins(cx: &mut TestAppContext) {
        let (mut rig, _, _) = rig(cx);
        let docker = plugin("docker", "containers");
        rig.register(docker.clone(), "Containers", Some("docker"));
        assert!(!rig.act(|sidebar, _, _| sidebar.has(&PanelId::Forwards)));

        for round in 0..2 {
            rig.register(PanelId::Forwards, "Port forwards", None);
            assert!(rig.act(|sidebar, _, _| sidebar.has(&PanelId::Forwards)));
            assert_eq!(
                rig.order(),
                [
                    PanelId::Files,
                    PanelId::Hosts,
                    PanelId::Forwards,
                    docker.clone()
                ],
                "round {round}"
            );
            rig.act(|sidebar, _, cx| sidebar.remove(&PanelId::Forwards, cx));
            assert!(!rig.act(|sidebar, _, _| sidebar.has(&PanelId::Forwards)));
            assert_eq!(
                rig.order(),
                [PanelId::Files, PanelId::Hosts, docker.clone()]
            );
        }
    }

    /// Spec 0018, Port forwards panel and flow 5: when Port forwards
    /// leaves while it is the shown panel, Files is shown, the sidebar
    /// stays open, and that isn't the user's act, so nothing connects by
    /// it. Coming back, the panel is listed again but not shown by itself.
    #[gpui::test]
    fn port_forwards_leaving_while_shown_shows_files_and_does_not_come_back_by_itself(
        cx: &mut TestAppContext,
    ) {
        let (mut rig, _, _) = rig(cx);
        rig.register(PanelId::Forwards, "Port forwards", None);
        rig.act(|sidebar, _, cx| sidebar.show(&PanelId::Forwards, cx));
        assert_eq!(rig.shown(), PanelId::Forwards);
        rig.seen();

        rig.act(|sidebar, _, cx| sidebar.remove(&PanelId::Forwards, cx));
        assert!(rig.is_open(), "the panel leaving closed the sidebar");
        assert_eq!(rig.shown(), PanelId::Files);
        assert_eq!(
            rig.shown_and_hidden(),
            [
                Seen::Hidden(PanelId::Forwards),
                Seen::Shown(PanelId::Files, false)
            ]
        );

        rig.register(PanelId::Forwards, "Port forwards", None);
        assert_eq!(rig.shown(), PanelId::Files, "it came back by itself");
        assert_eq!(rig.shown_and_hidden(), []);
        assert!(rig.pane_has_the_keyboard());
    }

    /// Spec 0018, AC8: the sidebar has no code path for a panel of its
    /// own. Its code names no panel ID and no panel type; the built-in
    /// panels reach it only through `register`.
    #[test]
    fn the_sidebars_code_names_no_panel_of_its_own() {
        let source = include_str!("mod.rs");
        let code = source.split("#[cfg(test)]").next().unwrap_or(source);
        for name in [
            "PanelId::Files",
            "PanelId::Hosts",
            "PanelId::Forwards",
            "PanelId::Plugin",
            "FilesPanel",
            "PanePanel",
            "HostsPanel",
            "ForwardsView",
        ] {
            assert!(!code.contains(name), "src/sidebar/mod.rs names {name}");
        }
    }

    /// Spec 0018, flow 1 and AC3: the toggle shows the sidebar on Files
    /// the first time, hides it whatever panel it shows, and brings it
    /// back on the panel it showed last. Each time a panel comes on screen
    /// or leaves it, `Shown` or `Hidden` says so (flow 3), and `Changed`
    /// tells the session to save.
    #[gpui::test]
    fn the_toggle_opens_on_the_panel_shown_last_and_closes_whatever_is_shown(
        cx: &mut TestAppContext,
    ) {
        let (mut rig, _, _) = rig(cx);
        assert!(!rig.is_open());

        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        assert!(rig.is_open());
        assert_eq!(rig.shown(), PanelId::Files);
        let seen = rig.seen();
        assert!(seen.contains(&Seen::Changed), "{seen:?}");
        assert!(
            seen.contains(&Seen::Shown(PanelId::Files, true)),
            "{seen:?}"
        );
        assert!(
            !seen.iter().any(|e| matches!(e, Seen::Hidden(_))),
            "{seen:?}"
        );

        rig.act(|sidebar, window, cx| sidebar.pick(PanelId::Hosts, window, cx));
        assert_eq!(
            rig.shown_and_hidden(),
            [
                Seen::Hidden(PanelId::Files),
                Seen::Shown(PanelId::Hosts, true)
            ]
        );

        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        assert!(!rig.is_open());
        let seen = rig.seen();
        assert!(seen.contains(&Seen::Changed), "{seen:?}");
        assert!(seen.contains(&Seen::Hidden(PanelId::Hosts)), "{seen:?}");
        assert!(
            !seen.iter().any(|e| matches!(e, Seen::Shown(..))),
            "{seen:?}"
        );

        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        assert!(rig.is_open());
        assert_eq!(rig.shown(), PanelId::Hosts);
        assert_eq!(rig.shown_and_hidden(), [Seen::Shown(PanelId::Hosts, true)]);
    }

    /// Spec 0018, flow 6 and Module surface: `show` opens a closed sidebar
    /// on the panel it names, or switches an open one to it, and says it
    /// wasn't the user's own act, so nothing connects because of it.
    /// Showing the panel that is on screen already says nothing.
    #[gpui::test]
    fn showing_a_panel_opens_or_switches_the_sidebar_and_is_not_the_users_act(
        cx: &mut TestAppContext,
    ) {
        let (mut rig, _, _) = rig(cx);
        rig.act(|sidebar, _, cx| sidebar.show(&PanelId::Hosts, cx));
        assert!(rig.is_open());
        assert_eq!(rig.shown(), PanelId::Hosts);
        assert_eq!(rig.shown_and_hidden(), [Seen::Shown(PanelId::Hosts, false)]);

        rig.act(|sidebar, _, cx| sidebar.show(&PanelId::Files, cx));
        assert_eq!(
            rig.shown_and_hidden(),
            [
                Seen::Hidden(PanelId::Hosts),
                Seen::Shown(PanelId::Files, false)
            ]
        );

        rig.act(|sidebar, _, cx| sidebar.show(&PanelId::Files, cx));
        assert_eq!(rig.shown_and_hidden(), []);
        assert!(rig.is_open());
    }

    /// Spec 0018, flow 5 and AC8: a panel registered through the same call
    /// the built-in panels use is listed after them and can be shown;
    /// removing it while it is shown shows Files, without that being the
    /// user's act. Removing a panel that isn't shown changes nothing on
    /// screen.
    #[gpui::test]
    fn removing_the_shown_panel_shows_files(cx: &mut TestAppContext) {
        let (mut rig, _, _) = rig(cx);
        let docker = plugin("docker", "containers");
        let logs = plugin("docker", "logs");
        rig.register(docker.clone(), "Containers", Some("docker"));
        rig.register(logs.clone(), "Logs", Some("docker"));
        assert_eq!(
            rig.order(),
            [PanelId::Files, PanelId::Hosts, docker.clone(), logs.clone()]
        );
        rig.act(|sidebar, _, cx| sidebar.show(&docker, cx));
        assert_eq!(rig.shown(), docker);
        rig.seen();

        rig.act(|sidebar, _, cx| sidebar.remove(&logs, cx));
        assert_eq!(rig.shown(), docker);
        assert_eq!(rig.shown_and_hidden(), []);

        rig.act(|sidebar, _, cx| sidebar.remove(&docker, cx));
        assert_eq!(rig.order(), [PanelId::Files, PanelId::Hosts]);
        assert_eq!(rig.shown(), PanelId::Files);
        assert!(rig.is_open(), "a panel leaving closed the sidebar");
        assert_eq!(
            rig.shown_and_hidden(),
            [Seen::Hidden(docker), Seen::Shown(PanelId::Files, false)]
        );
    }

    /// Spec 0018, flow 5: when the panel a closed sidebar showed last
    /// leaves, the sidebar stays closed and opens on Files next time.
    #[gpui::test]
    fn a_closed_sidebar_whose_last_panel_left_opens_on_files(cx: &mut TestAppContext) {
        let (mut rig, _, _) = rig(cx);
        let docker = plugin("docker", "containers");
        rig.register(docker.clone(), "Containers", Some("docker"));
        rig.act(|sidebar, _, cx| sidebar.show(&docker, cx));
        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        rig.seen();

        rig.act(|sidebar, _, cx| sidebar.remove(&docker, cx));
        assert!(!rig.is_open(), "a panel leaving opened the sidebar");
        assert_eq!(rig.shown_and_hidden(), []);

        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        assert_eq!(rig.shown(), PanelId::Files);
    }

    /// Spec 0018, "The keyboard stays with the shell" [load-bearing], AC1
    /// and AC10: opening the sidebar, switching panels, a panel shown from
    /// outside, a panel leaving and closing the sidebar never take the
    /// keyboard from the pane.
    #[gpui::test]
    fn opening_switching_and_closing_leave_the_keyboard_in_the_pane(cx: &mut TestAppContext) {
        let (mut rig, _, _) = rig(cx);
        let docker = plugin("docker", "containers");
        rig.register(docker.clone(), "Containers", Some("docker"));
        assert!(rig.pane_has_the_keyboard());

        type Act = Box<dyn FnOnce(&mut Sidebar, &mut Window, &mut Context<Sidebar>)>;
        let steps: [(&str, Act); 6] = [
            (
                "toggle open",
                Box::new(|s, window, cx| s.toggle(window, cx)),
            ),
            (
                "strip button",
                Box::new(|s, window, cx| s.pick(PanelId::Hosts, window, cx)),
            ),
            ("show", {
                let docker = docker.clone();
                Box::new(move |s, _, cx| s.show(&docker, cx))
            }),
            ("remove", Box::new(move |s, _, cx| s.remove(&docker, cx))),
            (
                "toggle closed",
                Box::new(|s, window, cx| s.toggle(window, cx)),
            ),
            ("open", Box::new(|s, _, cx| s.open(cx))),
        ];
        for (step, act) in steps {
            rig.act(act);
            assert!(rig.pane_has_the_keyboard(), "{step} took the keyboard");
            assert!(!rig.sidebar_has_the_keyboard(), "{step}");
            let seen = rig.seen();
            assert!(!seen.contains(&Seen::Leave), "{step}: {seen:?}");
        }
    }

    /// Spec 0018, flow 2 and AC6: `focus` puts the keyboard in the shown
    /// panel. Esc there asks for it to go back to the pane and leaves the
    /// sidebar open on the same panel. Esc with the keyboard in the pane
    /// isn't the sidebar's key at all.
    #[gpui::test]
    fn focus_puts_the_keyboard_in_the_shown_panel_and_esc_gives_it_back(cx: &mut TestAppContext) {
        let (mut rig, _, hosts) = rig(cx);
        rig.act(|sidebar, _, cx| sidebar.show(&PanelId::Hosts, cx));
        rig.seen();

        rig.cx.simulate_keystrokes("escape");
        assert_eq!(rig.seen(), [], "Esc in the pane reached the sidebar");

        rig.act(|sidebar, window, cx| sidebar.focus(window, cx));
        assert!(rig.has_the_keyboard(&hosts), "not in the shown panel");
        assert!(rig.sidebar_has_the_keyboard());
        assert!(!rig.pane_has_the_keyboard());

        rig.cx.simulate_keystrokes("escape");
        assert_eq!(rig.seen(), [Seen::Leave]);
        assert!(rig.is_open(), "Esc closed the sidebar");
        assert_eq!(rig.shown(), PanelId::Hosts);
    }

    /// Spec 0018, flow 3: a strip button moves the keyboard to the new
    /// panel if it was in the old one.
    #[gpui::test]
    fn switching_with_the_keyboard_in_the_old_panel_moves_it_to_the_new_one(
        cx: &mut TestAppContext,
    ) {
        let (mut rig, files, hosts) = rig(cx);
        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        rig.act(|sidebar, window, cx| sidebar.focus(window, cx));
        assert!(rig.has_the_keyboard(&files));

        rig.act(|sidebar, window, cx| sidebar.pick(PanelId::Hosts, window, cx));
        assert!(rig.has_the_keyboard(&hosts), "the keyboard didn't follow");
        assert!(!rig.pane_has_the_keyboard());
    }

    /// Spec 0018, flow 1 and "The keyboard stays with the shell": hiding
    /// the sidebar with the keyboard inside gives it back to the pane;
    /// hiding it with the keyboard in the pane asks for nothing.
    #[gpui::test]
    fn hiding_the_sidebar_with_the_keyboard_inside_gives_it_back(cx: &mut TestAppContext) {
        let (mut rig, _, _) = rig(cx);
        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        let seen = rig.seen();
        assert!(!seen.contains(&Seen::Leave), "{seen:?}");

        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        rig.act(|sidebar, window, cx| sidebar.focus(window, cx));
        assert!(rig.sidebar_has_the_keyboard());
        rig.seen();
        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        assert!(!rig.is_open());
        let seen = rig.seen();
        assert!(seen.contains(&Seen::Leave), "{seen:?}");
    }

    impl Rig<'_> {
        /// Gives the keyboard back to the pane, as before a click.
        fn focus_pane(&mut self) {
            let pane = self.pane.clone();
            self.cx.update(|window, cx| window.focus(&pane, cx));
            self.draw();
        }

        /// Clicks along the strip of an open sidebar on Files, the pane
        /// holding the keyboard before each click, until a click shows SSH
        /// hosts: the buttons carry no test selector. Stops right after
        /// that click, and returns where it was.
        fn click_the_hosts_button(&mut self) -> gpui::Point<gpui::Pixels> {
            let left = self.window_width() - WIDTH;
            for y in [10.0, 16.0, 22.0] {
                for step in 0..40 {
                    self.focus_pane();
                    self.seen();
                    let at = point(px(left + EDGE + 2.0 + 4.0 * step as f32), px(y));
                    self.cx.simulate_click(at, Modifiers::none());
                    self.draw();
                    if self.shown() == PanelId::Hosts {
                        return at;
                    }
                }
            }
            panic!("no click on the strip showed SSH hosts");
        }

        /// Drags the inner edge of an open, 260 px wide sidebar on the
        /// right to each of `to` in turn, and returns the width after
        /// each.
        fn drag_the_edge(&mut self, to: &[f32]) -> Vec<f32> {
            let (y, none) = (px(300.0), Modifiers::none());
            let edge = self.window_width() - WIDTH + EDGE / 2.0;
            self.cx
                .simulate_mouse_down(point(px(edge), y), MouseButton::Left, none);
            let mut widths = Vec::new();
            // The first move only starts the drag.
            for (ix, x) in [edge - 10.0].iter().chain(to).enumerate() {
                self.cx
                    .simulate_mouse_move(point(px(*x), y), MouseButton::Left, none);
                self.draw();
                if ix > 0 {
                    widths.push(self.act(|sidebar, _, _| sidebar.width()));
                }
            }
            let last = to.last().copied().unwrap_or(edge);
            self.cx
                .simulate_mouse_up(point(px(last), y), MouseButton::Left, none);
            self.draw();
            widths
        }
    }

    /// Spec 0018, flow 3: a click on a strip button shows that panel, as
    /// the user's own act, and says which panel left the screen.
    #[gpui::test]
    fn a_click_on_a_strip_button_shows_its_panel(cx: &mut TestAppContext) {
        let (mut rig, _, _) = rig(cx);
        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        rig.click_the_hosts_button();
        assert_eq!(
            rig.shown_and_hidden(),
            [
                Seen::Hidden(PanelId::Files),
                Seen::Shown(PanelId::Hosts, true)
            ]
        );
        assert!(rig.is_open());
    }

    /// Spec 0018, flow 3 and "The keyboard stays with the shell"
    /// [load-bearing]: with the keyboard in the pane, a click on a strip
    /// button switches the panel and leaves the keyboard in the pane.
    #[gpui::test]
    fn a_click_on_a_strip_button_leaves_the_keyboard_in_the_pane(cx: &mut TestAppContext) {
        let (mut rig, _, _) = rig(cx);
        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        rig.click_the_hosts_button();
        assert!(rig.pane_has_the_keyboard(), "the click took the keyboard");
        assert!(!rig.sidebar_has_the_keyboard());
    }

    /// Spec 0018, "The keyboard stays with the shell" [load-bearing]: the
    /// keyboard enters the sidebar only by a click on something in a panel
    /// that takes keys, or by `sidebar.focus`. A click on the strip beside
    /// its buttons is neither, so the keyboard stays in the pane.
    #[gpui::test]
    fn a_click_on_the_strip_beside_its_buttons_leaves_the_keyboard_in_the_pane(
        cx: &mut TestAppContext,
    ) {
        let (mut rig, _, _) = rig(cx);
        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        // Two buttons at the strip's start leave its end empty.
        let end = point(px(rig.window_width() - 20.0), px(14.0));
        rig.cx.simulate_click(end, Modifiers::none());
        rig.draw();
        assert_eq!(rig.shown(), PanelId::Files, "the click hit a button");
        assert!(rig.pane_has_the_keyboard(), "the click took the keyboard");
        assert!(!rig.sidebar_has_the_keyboard());
    }

    /// Spec 0018, Width: dragging the inner edge sets the width, from 200
    /// px up to half the window, and the session hears of it.
    #[gpui::test]
    fn dragging_the_inner_edge_sets_the_width_within_its_limits(cx: &mut TestAppContext) {
        let (mut rig, _, _) = rig(cx);
        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        rig.seen();
        let window = rig.window_width();
        assert!(window >= 800.0, "the test window is {window} px wide");

        let widths = rig.drag_the_edge(&[window - 320.0, 10.0, window - 20.0]);
        assert!(
            (widths[0] - 320.0).abs() < 1.0,
            "dragged to 320 px: {widths:?}"
        );
        assert_eq!(widths[1], window / 2.0, "dragged past half the window");
        assert_eq!(widths[2], MIN_WIDTH, "dragged under 200 px");
        let seen = rig.seen();
        assert!(seen.contains(&Seen::Changed), "{seen:?}");
        assert!(rig.is_open());
    }

    /// Spec 0018, "The keyboard stays with the shell" [load-bearing]: the
    /// keyboard enters the sidebar only by a click on something in a panel
    /// that takes keys, or by `sidebar.focus`. Dragging the edge to resize
    /// the sidebar is neither, so the keyboard stays in the pane.
    #[gpui::test]
    fn dragging_the_inner_edge_leaves_the_keyboard_in_the_pane(cx: &mut TestAppContext) {
        let (mut rig, _, _) = rig(cx);
        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        let window = rig.window_width();
        rig.drag_the_edge(&[window - 320.0]);
        assert!(rig.pane_has_the_keyboard(), "the drag took the keyboard");
        assert!(!rig.sidebar_has_the_keyboard());
    }

    impl Rig<'_> {
        /// Lets `time` pass on the test clock.
        fn wait(&mut self, time: Duration) {
            self.cx.executor().advance_clock(time);
            self.cx.run_until_parked();
            self.draw();
        }

        fn ignores_clicks(&mut self) -> bool {
            self.act(|sidebar, _, _| sidebar.ignores_clicks())
        }
    }

    /// Spec 0018, flow 6 [load-bearing] and TM-plug-23: "After an
    /// `open_panel` changed what the sidebar shows, the sidebar ignores
    /// clicks for 0.5 s", and no longer. It "opens the sidebar if needed
    /// and shows the panel, without focus", and isn't the user's own act.
    /// One for the panel that is on screen already changes nothing, so it
    /// starts no 0.5 s.
    #[gpui::test]
    fn a_panel_shown_unasked_makes_the_sidebar_ignore_clicks_for_half_a_second(
        cx: &mut TestAppContext,
    ) {
        let (mut rig, _, _) = rig(cx);
        let docker = plugin("docker", "containers");
        rig.register(docker.clone(), "Containers", Some("docker"));
        rig.seen();
        assert_eq!(CLICK_GUARD, Duration::from_millis(500));
        assert!(!rig.ignores_clicks());

        // Onto a closed sidebar.
        rig.act(|sidebar, _, cx| sidebar.show_unasked(&docker, cx));
        assert!(rig.is_open());
        assert_eq!(rig.shown(), docker);
        assert_eq!(rig.shown_and_hidden(), [Seen::Shown(docker.clone(), false)]);
        assert!(rig.ignores_clicks());
        assert!(rig.pane_has_the_keyboard(), "it took the keyboard");
        rig.wait(Duration::from_millis(499));
        assert!(rig.ignores_clicks(), "clicks came back before 0.5 s");
        rig.wait(Duration::from_millis(1));
        assert!(
            !rig.ignores_clicks(),
            "clicks are still ignored after 0.5 s"
        );

        // The panel is on screen already: nothing changes under a click.
        rig.act(|sidebar, _, cx| sidebar.show_unasked(&docker, cx));
        assert!(
            !rig.ignores_clicks(),
            "nothing changed, yet clicks are ignored"
        );
        assert_eq!(rig.seen(), []);

        // From another panel of an open sidebar.
        rig.act(|sidebar, window, cx| sidebar.pick(PanelId::Files, window, cx));
        rig.seen();
        rig.act(|sidebar, _, cx| sidebar.show_unasked(&docker, cx));
        assert_eq!(
            rig.shown_and_hidden(),
            [
                Seen::Hidden(PanelId::Files),
                Seen::Shown(docker.clone(), false)
            ]
        );
        assert!(rig.ignores_clicks());
        assert!(rig.pane_has_the_keyboard(), "it took the keyboard");
        assert!(!rig.sidebar_has_the_keyboard());
        rig.wait(CLICK_GUARD);
        assert!(!rig.ignores_clicks());

        // Onto a sidebar the user closed on that very panel: it comes up
        // where there was none.
        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        rig.act(|sidebar, _, cx| sidebar.show_unasked(&docker, cx));
        assert!(rig.is_open());
        assert!(rig.ignores_clicks(), "a sidebar opening changed the screen");
    }

    /// Spec 0018, flow 6 and AC10: within the 0.5 s after an `open_panel`
    /// a click on the sidebar does nothing, so a panel that came up under
    /// the pointer can't catch a click meant for what was there; the same
    /// click after it acts again.
    #[gpui::test]
    fn a_click_right_after_a_panel_was_shown_unasked_does_nothing(cx: &mut TestAppContext) {
        let (mut rig, _, _) = rig(cx);
        let docker = plugin("docker", "containers");
        rig.register(docker.clone(), "Containers", Some("docker"));
        rig.act(|sidebar, window, cx| sidebar.toggle(window, cx));
        let hosts_button = rig.click_the_hosts_button();
        rig.act(|sidebar, window, cx| sidebar.pick(PanelId::Files, window, cx));

        rig.act(|sidebar, _, cx| sidebar.show_unasked(&docker, cx));
        rig.seen();
        rig.cx.simulate_click(hosts_button, Modifiers::none());
        rig.draw();
        assert_eq!(rig.shown(), docker, "a click within 0.5 s switched panels");
        assert_eq!(rig.seen(), []);
        assert!(rig.is_open());
        assert!(
            rig.pane_has_the_keyboard(),
            "the ignored click took the keyboard"
        );

        rig.wait(CLICK_GUARD);
        rig.cx.simulate_click(hosts_button, Modifiers::none());
        rig.draw();
        assert_eq!(rig.shown(), PanelId::Hosts, "clicks stayed ignored");
        assert_eq!(
            rig.shown_and_hidden(),
            [Seen::Hidden(docker), Seen::Shown(PanelId::Hosts, true)]
        );
    }
}
