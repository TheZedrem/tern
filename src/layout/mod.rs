//! The window's content (spec 0003): the tab bar, which doubles as the
//! title bar when the compositor draws no frame, over the active tab's
//! split tree. The layout itself is plain data in [`tabs`] and [`tree`];
//! this module owns the panes and draws them.

pub mod tabs;
pub mod tree;

use crate::config::Config;
use crate::terminal::pty::{self, Command, GridSize};
use crate::terminal::{FontSettings, PaneEvent, TerminalPane};
use crate::theme::{self, Theme};
use crate::timing::{Mark, Timing};
use gpui::prelude::FluentBuilder;
use gpui::{
    Action, Anchor, AnyElement, App, AppContext, ClickEvent, Context, Decorations, Div,
    DragMoveEvent, ElementId, Entity, InteractiveElement, IntoElement, KeyBinding, MouseButton,
    ParentElement, Pixels, Render, SharedString, Size, StatefulInteractiveElement, Styled,
    Subscription, Window, actions, canvas, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::notification::Notification;
use gpui_component::tab::{Tab, TabBar};
// Component icons only: `gpui_kit_assets::IconName` names the whole Lucide
// catalog, but the registered `Assets` bundle holds just these, and anything
// else renders blank.
use gpui_component::{IconName, Sizable, WindowExt, window_border};
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use tabs::{Closed, TabId, Tabs};
use tree::{Axis, Direction, LeafId, Node};

/// Key context of the workspace: tab and split shortcuts.
const CONTEXT: &str = "Workspace";
/// Key context of the tab-rename field.
const RENAME_CONTEXT: &str = "TabRename";
/// Thickness of a split divider, in logical pixels.
const DIVIDER: f32 = 4.0;
/// The smallest pane a divider may leave (spec 0003).
const MIN_PANE_CELLS: (f32, f32) = (10.0, 3.0);

actions!(
    zed_term,
    [
        NewTab,
        ClosePane,
        NextTab,
        PreviousTab,
        SplitRight,
        SplitDown,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        ZoomPane,
        CancelRename,
    ]
);

/// Activates tab N, counted from 0.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = zed_term, no_json)]
pub struct GoToTab(pub usize);

/// An app shortcut: `Ctrl+Shift` on Linux and Windows, `Cmd` on macOS
/// (spec 0004).
#[cfg(not(target_os = "macos"))]
macro_rules! shortcut {
    ($key:literal) => {
        concat!("ctrl-shift-", $key)
    };
}
#[cfg(target_os = "macos")]
macro_rules! shortcut {
    ($key:literal) => {
        concat!("cmd-", $key)
    };
}

/// Binds spec 0004's default keys for the tab and split actions. T-004
/// adds config overrides on top.
pub fn init(cx: &mut App) {
    let workspace = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new(shortcut!("t"), NewTab, workspace),
        KeyBinding::new(shortcut!("w"), ClosePane, workspace),
        KeyBinding::new("ctrl-tab", NextTab, workspace),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, workspace),
        KeyBinding::new(shortcut!("d"), SplitRight, workspace),
        KeyBinding::new(shortcut!("e"), SplitDown, workspace),
        KeyBinding::new("alt-left", FocusLeft, workspace),
        KeyBinding::new("alt-right", FocusRight, workspace),
        KeyBinding::new("alt-up", FocusUp, workspace),
        KeyBinding::new("alt-down", FocusDown, workspace),
        KeyBinding::new(shortcut!("z"), ZoomPane, workspace),
        KeyBinding::new(shortcut!("1"), GoToTab(0), workspace),
        KeyBinding::new(shortcut!("2"), GoToTab(1), workspace),
        KeyBinding::new(shortcut!("3"), GoToTab(2), workspace),
        KeyBinding::new(shortcut!("4"), GoToTab(3), workspace),
        KeyBinding::new(shortcut!("5"), GoToTab(4), workspace),
        KeyBinding::new(shortcut!("6"), GoToTab(5), workspace),
        KeyBinding::new(shortcut!("7"), GoToTab(6), workspace),
        KeyBinding::new(shortcut!("8"), GoToTab(7), workspace),
        KeyBinding::new(shortcut!("9"), GoToTab(8), workspace),
        KeyBinding::new("escape", CancelRename, Some(RENAME_CONTEXT)),
    ]);
}

/// Dragging a tab to a new place in the bar.
#[derive(Clone)]
struct DraggedTab {
    index: usize,
    title: SharedString,
}

/// Dragging the divider of the split at this path.
#[derive(Clone)]
struct DraggedDivider(Vec<bool>);

/// What follows the mouse while a tab is dragged.
struct TabGhost(SharedString);

impl Render for TabGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(10.))
            .py(px(4.))
            .rounded(px(4.))
            .bg(gpui::black().opacity(0.6))
            .text_color(gpui::white())
            .text_size(px(12.))
            .child(self.0.clone())
    }
}

/// A divider drag shows nothing but the moving divider itself.
struct DividerGhost;

impl Render for DividerGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A tab being renamed: its ID and the text field.
struct Rename {
    tab: TabId,
    input: Entity<InputState>,
}

pub struct Workspace {
    tabs: Tabs,
    panes: HashMap<LeafId, Entity<TerminalPane>>,
    pane_subscriptions: HashMap<LeafId, Subscription>,
    config: Config,
    theme: Theme,
    timing: Arc<Timing>,
    bench: bool,
    /// Boot notices, shown once the first frame is on screen.
    notices: Vec<String>,
    /// TTFP for the dev badge, once the first frame was drawn.
    ttfp: Option<f64>,
    rename: Option<Rename>,
    rename_subscription: Option<Subscription>,
    /// A mouse press on the title bar's empty area that may become a move.
    title_bar_pressed: bool,
    /// The tab content's size at the last layout, for the split-room check.
    content_size: Rc<Cell<Option<Size<Pixels>>>>,
}

impl Workspace {
    /// A workspace with one tab running `boot`, the session started before
    /// the window existed (spec 0001).
    #[allow(clippy::too_many_arguments, reason = "one call, from the boot path")]
    pub fn new(
        boot: Result<pty::Session, String>,
        command: &Command,
        config: Config,
        theme: Theme,
        notices: Vec<String>,
        timing: Arc<Timing>,
        bench: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tabs, leaf) = Tabs::new();
        let mut workspace = Self {
            tabs,
            panes: HashMap::new(),
            pane_subscriptions: HashMap::new(),
            config,
            theme,
            timing,
            bench,
            notices,
            ttfp: None,
            rename: None,
            rename_subscription: None,
            title_bar_pressed: false,
            content_size: Rc::default(),
        };
        workspace.add_pane(leaf, boot, command, window, cx);
        workspace.focus_active(window, cx);
        workspace
    }

    fn add_pane(
        &mut self,
        leaf: LeafId,
        session: Result<pty::Session, String>,
        command: &Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (config, theme, bench) = (&self.config, self.theme, self.bench);
        let pane = cx.new(|cx| TerminalPane::new(session, command, config, theme, bench, cx));
        let subscription = cx.subscribe_in(&pane, window, move |this, _, event, window, cx| {
            this.pane_event(leaf, event, window, cx);
        });
        self.panes.insert(leaf, pane);
        self.pane_subscriptions.insert(leaf, subscription);
    }

    /// Starts the default shell for `leaf`, in `cwd` or the home directory.
    /// Profiles arrive with T-005.
    fn spawn_pane(
        &mut self,
        leaf: LeafId,
        cwd: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = Command::default_shell();
        let font = FontSettings::from_config(&self.config.font);
        let viewport = window.viewport_size();
        let grid = GridSize::fit(
            (f32::from(viewport.width), f32::from(viewport.height)),
            font.estimated_cell(),
        );
        let session = pty::spawn(
            &command,
            grid,
            cwd.as_deref(),
            self.config.scrollback as usize,
            Arc::clone(&self.timing),
        );
        self.add_pane(leaf, session, &command, window, cx);
    }

    fn pane_event(
        &mut self,
        leaf: LeafId,
        event: &PaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PaneEvent::FirstFrame => self.first_frame(window, cx),
            PaneEvent::Focused => {
                if self.tabs.focus(leaf) {
                    cx.notify();
                }
            }
            PaneEvent::TitleChanged => cx.notify(),
            PaneEvent::Exited => self.close_pane(leaf, window, cx),
        }
    }

    /// Records and prints TTFP (spec 0001), then runs what waits for the
    /// first frame. In bench mode, quits instead. Later panes' first frames
    /// change nothing: the marks are set once.
    fn first_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.timing.mark(Mark::Rendered) {
            return;
        }
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

    /// Moves keyboard focus to the active tab's focused pane.
    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.panes.get(&self.tabs.active().focused) {
            let handle = pane.read(cx).focus_handle().clone();
            window.focus(&handle, cx);
        }
    }

    /// Stops a pane's program and forgets the pane.
    fn stop_pane(&mut self, leaf: LeafId, cx: &mut Context<Self>) {
        self.pane_subscriptions.remove(&leaf);
        if let Some(pane) = self.panes.remove(&leaf) {
            pane.update(cx, |pane, _| pane.shutdown());
        }
    }

    fn after_close(&mut self, closed: Closed, window: &mut Window, cx: &mut Context<Self>) {
        if closed == Closed::Last {
            cx.quit();
            return;
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn close_pane(&mut self, leaf: LeafId, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_pane(leaf, cx);
        let closed = self.tabs.close_pane(leaf);
        self.after_close(closed, window, cx);
    }

    fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (leaves, closed) = self.tabs.close_tab(ix);
        for leaf in leaves {
            self.stop_pane(leaf, cx);
        }
        self.after_close(closed, window, cx);
    }

    /// The tab's title: the user's name, else the focused pane's title, else
    /// the profile's name (`shell` until profiles land with T-005).
    fn tab_title(&self, tab: &tabs::Tab, cx: &App) -> SharedString {
        tab.name
            .clone()
            .or_else(|| {
                self.panes
                    .get(&tab.focused)
                    .and_then(|pane| pane.read(cx).title().map(str::to_owned))
            })
            .unwrap_or_else(|| "shell".to_owned())
            .into()
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let leaf = self.tabs.new_tab();
        self.spawn_pane(leaf, None, window, cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    fn close_focused_pane(&mut self, _: &ClosePane, window: &mut Window, cx: &mut Context<Self>) {
        let leaf = self.tabs.active().focused;
        self.close_pane(leaf, window, cx);
    }

    /// Splits the focused pane, unless that would leave either half below
    /// the minimum pane size (spec 0003); then the shortcut does nothing.
    fn split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(size) = self.content_size.get() {
            let area = (f32::from(size.width), f32::from(size.height));
            if !self
                .tabs
                .can_split(axis, area, self.pane_min(window), DIVIDER)
            {
                return;
            }
        }
        let source = self.tabs.active().focused;
        let cwd = self.panes.get(&source).and_then(|pane| pane.read(cx).cwd());
        let leaf = self.tabs.split(axis);
        self.spawn_pane(leaf, cwd, window, cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    fn focus_toward(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.focus_direction(direction).is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn switch_tab(
        &mut self,
        change: impl FnOnce(&mut Tabs),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        change(&mut self.tabs);
        self.focus_active(window, cx);
        cx.notify();
    }

    fn start_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.tabs().get(ix) else {
            return;
        };
        let (id, title) = (tab.id, self.tab_title(tab, cx));
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        self.rename_subscription = Some(cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let name = input.read(cx).value().to_string();
                    this.finish_rename(Some(name), window, cx);
                }
                InputEvent::Blur => this.finish_rename(None, window, cx),
                _ => {}
            },
        ));
        input.update(cx, |input, cx| input.focus(window, cx));
        self.rename = Some(Rename { tab: id, input });
        cx.notify();
    }

    /// Ends a rename; `Some` saves the name (blank resets it), `None` keeps
    /// the old one.
    fn finish_rename(&mut self, name: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        if let (Some(name), Some(ix)) = (name, self.tabs.index_of(rename.tab)) {
            self.tabs.rename(ix, Some(name));
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Sets a divider from the mouse, keeping both sides at least
    /// `MIN_PANE_CELLS` (spec 0003).
    fn drag_divider(
        &mut self,
        path: &[bool],
        axis: Axis,
        event: &DragMoveEvent<DraggedDivider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.drag(cx).0 != path {
            return;
        }
        let bounds = event.bounds;
        let position = event.event.position;
        let (offset, length) = match axis {
            Axis::Row => (position.x - bounds.left(), bounds.size.width),
            Axis::Column => (position.y - bounds.top(), bounds.size.height),
        };
        let (offset, length) = (f32::from(offset), f32::from(length));
        let pane_min = self.pane_min(window);
        let Some(Node::Split {
            ratio,
            first,
            second,
            ..
        }) = self.tabs.active_mut().root.split_at_mut(path)
        else {
            return;
        };
        let (first_min, second_min) = {
            let (a, b) = (
                first.min_size(pane_min, DIVIDER),
                second.min_size(pane_min, DIVIDER),
            );
            match axis {
                Axis::Row => (a.0, b.0),
                Axis::Column => (a.1, b.1),
            }
        };
        let wanted = (offset - DIVIDER / 2.0) / (length - DIVIDER);
        *ratio = tree::clamp_ratio(wanted, length, first_min, second_min, DIVIDER);
        cx.notify();
    }

    /// The smallest pane in pixels: `MIN_PANE_CELLS` of the current font,
    /// plus the grid's padding.
    fn pane_min(&self, window: &Window) -> (f32, f32) {
        let cell = FontSettings::from_config(&self.config.font).cell_size(window);
        (
            MIN_PANE_CELLS.0 * f32::from(cell.width) + 2.0 * pty::PADDING,
            MIN_PANE_CELLS.1 * f32::from(cell.height) + 2.0 * pty::PADDING,
        )
    }

    fn render_leaf(&self, leaf: LeafId) -> AnyElement {
        match self.panes.get(&leaf) {
            Some(pane) => div().size_full().child(pane.clone()).into_any_element(),
            None => div().size_full().into_any_element(),
        }
    }

    fn render_node(
        &self,
        node: &Node,
        path: Vec<bool>,
        pane_min: (f32, f32),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (axis, ratio, first, second) = match node {
            Node::Leaf(leaf) => return self.render_leaf(*leaf),
            Node::Split {
                axis,
                ratio,
                first,
                second,
            } => (*axis, ratio, first, second),
        };
        let key: String = path.iter().map(|&b| if b { '1' } else { '0' }).collect();
        // Both halves share the space left after the divider by weight, so
        // the first gets exactly `ratio` of it. Their minimum sizes keep a
        // shrinking window from squeezing a pane below the minimum.
        let half = |share: f32, node: &Node, child: AnyElement| {
            let (min_width, min_height) = node.min_size(pane_min, DIVIDER);
            let mut half = div()
                .flex_basis(px(0.))
                .overflow_hidden()
                .map(|half| match axis {
                    Axis::Row => half.min_w(px(min_width)).min_h_0(),
                    Axis::Column => half.min_h(px(min_height)).min_w_0(),
                })
                .child(child);
            half.style().flex_grow = Some(share);
            half.style().flex_shrink = Some(1.);
            half
        };
        let child_path = |second_child: bool| {
            let mut child = path.clone();
            child.push(second_child);
            child
        };
        let first = half(
            *ratio,
            first,
            self.render_node(first, child_path(false), pane_min, cx),
        );
        let second = half(
            1.0 - *ratio,
            second,
            self.render_node(second, child_path(true), pane_min, cx),
        );
        let divider = div()
            .id(ElementId::Name(format!("divider-{key}").into()))
            .flex_shrink_0()
            .bg(theme::hsla(self.theme.foreground).opacity(0.12))
            .map(|divider| match axis {
                Axis::Row => divider.w(px(DIVIDER)).h_full().cursor_col_resize(),
                Axis::Column => divider.h(px(DIVIDER)).w_full().cursor_row_resize(),
            })
            .on_drag(DraggedDivider(path.clone()), |_, _, _, cx| {
                cx.new(|_| DividerGhost)
            });
        let container = div()
            .id(ElementId::Name(format!("split-{key}").into()))
            .size_full()
            .flex()
            .map(|container: gpui::Stateful<Div>| match axis {
                Axis::Row => container.flex_row(),
                Axis::Column => container.flex_col(),
            })
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<DraggedDivider>, window, cx| {
                    this.drag_divider(&path, axis, event, window, cx);
                },
            ));
        container
            .child(first)
            .child(divider)
            .child(second)
            .into_any_element()
    }

    fn render_tabs(&self, window: &mut Window, cx: &mut Context<Self>) -> TabBar {
        let renaming = self.rename.as_ref().map(|rename| rename.tab);
        let tabs = self.tabs.tabs().iter().enumerate().map(|(ix, tab)| {
            let title = self.tab_title(tab, cx);
            let label = match &self.rename {
                Some(rename) if renaming == Some(tab.id) => div()
                    .key_context(RENAME_CONTEXT)
                    .on_action(cx.listener(|this, _: &CancelRename, window, cx| {
                        this.finish_rename(None, window, cx);
                    }))
                    .w(px(160.))
                    .child(Input::new(&rename.input).xsmall())
                    .into_any_element(),
                _ => div()
                    .id(("tab-label", tab.id))
                    .max_w(px(220.))
                    .truncate()
                    .child(title.clone())
                    .on_drag(
                        DraggedTab {
                            index: ix,
                            title: title.clone(),
                        },
                        |tab, _, _, cx| cx.new(|_| TabGhost(tab.title.clone())),
                    )
                    .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                        this.tabs.move_tab(dragged.index, ix);
                        cx.notify();
                    }))
                    .into_any_element(),
            };
            let close = Button::new(("close-tab", tab.id))
                .icon(IconName::Close)
                .ghost()
                .xsmall()
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.close_tab(ix, window, cx);
                }));
            Tab::new().child(label).suffix(close).on_click(cx.listener(
                move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_rename(ix, window, cx);
                    } else {
                        this.switch_tab(|tabs| tabs.activate(ix), window, cx);
                    }
                },
            ))
        });

        let client_decorated = matches!(window.window_decorations(), Decorations::Client { .. });
        let new_tab = Button::new("new-tab")
            .icon(IconName::Plus)
            .ghost()
            .small()
            .on_click(cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx)));
        let suffix = div()
            .flex()
            .items_center()
            .child(new_tab)
            .when(client_decorated, |suffix| {
                suffix.child(Self::window_controls(window))
            });
        TabBar::new("tabs")
            .selected_index(self.tabs.active_index())
            .children(tabs)
            .suffix(suffix)
            .last_empty_space(self.title_bar_space(client_decorated, cx))
    }

    /// The tab bar's free space. When the compositor draws no frame it acts
    /// as the title bar: drag to move, double-click to maximize (spec 0003,
    /// flow 8). A press only becomes a move once the mouse moves, so the
    /// double click still registers.
    fn title_bar_space(&self, client_decorated: bool, cx: &mut Context<Self>) -> AnyElement {
        let space = div().id("title-bar-space").flex_1().h_full().min_w(px(24.));
        if !client_decorated {
            return space.into_any_element();
        }
        space
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_bar_pressed = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_bar_pressed = false),
            )
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if std::mem::take(&mut this.title_bar_pressed) {
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.zoom_window();
                }
            })
            .into_any_element()
    }

    /// Minimize, maximize and close, for compositors that draw no frame.
    fn window_controls(window: &Window) -> impl IntoElement {
        let supported = window.window_controls();
        let maximize_icon = if window.is_maximized() {
            IconName::WindowRestore
        } else {
            IconName::WindowMaximize
        };
        div()
            .flex()
            .items_center()
            .when(supported.minimize, |controls| {
                controls.child(
                    Button::new("minimize-window")
                        .icon(IconName::WindowMinimize)
                        .ghost()
                        .small()
                        .on_click(|_, window, _| window.minimize_window()),
                )
            })
            .when(supported.maximize, |controls| {
                controls.child(
                    Button::new("maximize-window")
                        .icon(maximize_icon)
                        .ghost()
                        .small()
                        .on_click(|_, window, _| window.zoom_window()),
                )
            })
            .child(
                Button::new("close-window")
                    .icon(IconName::WindowClose)
                    .ghost()
                    .small()
                    .on_click(|_, window, _| window.remove_window()),
            )
    }
}

/// An invisible element covering its parent that records the parent's size
/// at every layout.
fn measure_size(size: Rc<Cell<Option<Size<Pixels>>>>) -> impl IntoElement {
    canvas(
        move |bounds, _, _| size.set(Some(bounds.size)),
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tabs.active();
        let content = match tab.zoomed {
            Some(leaf) => self.render_leaf(leaf),
            None => self.render_node(&tab.root, Vec::new(), self.pane_min(window), cx),
        };
        // The dev badge (spec 0001): every debug run shows its own TTFP.
        // Bottom left: window controls sit top right, notices bottom right.
        let badge = self.ttfp.filter(|_| cfg!(debug_assertions)).map(|ms| {
            div()
                .absolute()
                .bottom(px(8.))
                .left(px(12.))
                .px(px(8.))
                .py(px(2.))
                .rounded(px(4.))
                .bg(gpui::black().opacity(0.6))
                .text_size(px(12.))
                .text_color(gpui::white())
                .child(format!("TTFP {ms:.0} ms"))
        });
        let workspace = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::hsla(self.theme.background))
            .text_color(theme::hsla(self.theme.foreground))
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_focused_pane))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| {
                this.switch_tab(Tabs::next_tab, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| {
                this.switch_tab(Tabs::previous_tab, window, cx);
            }))
            .on_action(cx.listener(|this, action: &GoToTab, window, cx| {
                let ix = action.0;
                this.switch_tab(|tabs| tabs.activate(ix), window, cx);
            }))
            .on_action(cx.listener(|this, _: &SplitRight, window, cx| {
                this.split(Axis::Row, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SplitDown, window, cx| {
                this.split(Axis::Column, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusLeft, window, cx| {
                this.focus_toward(Direction::Left, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusRight, window, cx| {
                this.focus_toward(Direction::Right, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusUp, window, cx| {
                this.focus_toward(Direction::Up, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusDown, window, cx| {
                this.focus_toward(Direction::Down, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ZoomPane, _, cx| {
                this.tabs.toggle_zoom();
                cx.notify();
            }))
            .child(self.render_tabs(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(content)
                    .child(measure_size(Rc::clone(&self.content_size))),
            )
            .children(badge);
        // Shadow and resize edges when the compositor draws no frame;
        // nothing otherwise.
        window_border().child(workspace)
    }
}
