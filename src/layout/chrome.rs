//! The tab bar, top or side, which doubles as the title bar when the
//! compositor draws no frame (spec 0003), and renaming tabs.

use super::splits::DraggedPane;
use super::tabs::TabId;
use super::tree::LeafId;
use super::{CancelRename, CloseWindow, Workspace, tabs};
use crate::commands::{NewTab, OpenSettings};
use crate::config::{Profile, TabBarSide};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, Decorations, Entity, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::tab::{Tab, TabBar};
// Component icons only: `gpui_kit_assets::IconName` names the whole Lucide
// catalog, but the registered `Assets` bundle holds just these, and anything
// else renders blank.
use gpui_component::{ActiveTheme, IconName, Sizable};
use std::cmp::Ordering;

/// Key context of the tab-rename field.
pub(super) const RENAME_CONTEXT: &str = "TabRename";

/// Dragging a tab to a new place in the bar, or onto a side of a pane
/// (spec 0007).
#[derive(Clone)]
pub(super) struct DraggedTab {
    pub(super) index: usize,
    pub(super) id: TabId,
    pub(super) title: SharedString,
}

/// What follows the mouse while a tab is dragged.
pub(super) struct TabGhost(pub(super) SharedString);

impl Render for TabGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px(px(10.))
            .py(px(4.))
            .rounded(px(4.))
            .border_1()
            .border_color(theme.drag_border)
            .bg(theme.tab_active)
            .text_color(theme.tab_active_foreground)
            .text_size(px(12.))
            .shadow_md()
            .child(self.0.clone())
    }
}

/// A tab being renamed: its ID and the text field.
pub(super) struct Rename {
    pub(super) tab: TabId,
    pub(super) input: Entity<InputState>,
}

impl Workspace {
    /// The tab's title: the user's name, else its focused pane's (spec
    /// 0003).
    fn tab_title(&self, tab: &tabs::Tab, cx: &App) -> SharedString {
        match &tab.name {
            Some(name) => name.clone().into(),
            None => self.pane_title(tab.focused, cx),
        }
    }

    /// A pane's title: its program's, else the name of its profile. A
    /// restored pane that hasn't started shows what it was saved with.
    pub(super) fn pane_title(&self, leaf: LeafId, cx: &App) -> SharedString {
        if let Some(files) = self.files.get(&leaf) {
            return files.pane.read(cx).title(cx).into();
        }
        let Some(pane) = self.panes.get(&leaf) else {
            let Some(saved) = self.pending.get(&leaf) else {
                return SharedString::default();
            };
            if saved.files.is_some() {
                return "Files".into();
            }
            let profile = self.config.profile(&saved.profile);
            let name = profile
                .as_ref()
                .map_or(&*saved.profile, Profile::display_name);
            return saved.title.as_deref().unwrap_or(name).to_owned().into();
        };
        let pane = pane.read(cx);
        pane.title()
            .unwrap_or(&pane.profile().name)
            .to_owned()
            .into()
    }

    /// Opens the rename field over tab `ix`, holding its current title, all
    /// selected so typing replaces it.
    pub(super) fn start_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
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
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
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
        self.show_active(window, cx);
    }

    /// A middle click on tab `ix` closes it as its close button does
    /// (spec 0003, flow 2). GPUI fires it on release over the tab the press
    /// began on, and only left clicks activate a tab, so it never does.
    fn middle_click_tab(
        &mut self,
        event: &ClickEvent,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.is_middle_click() {
            cx.stop_propagation();
            self.ask_close_tab(ix, window, cx);
        }
    }

    pub(super) fn render_tabs(&self, window: &mut Window, cx: &mut Context<Self>) -> TabBar {
        let renaming = self.rename.as_ref().map(|rename| rename.tab);
        let settings = Self::settings_button("settings-button", cx);
        let sidebar = Self::sidebar_button("sidebar-button", cx);
        let tabs = self.tabs.tabs().iter().enumerate().map(|(ix, tab)| {
            let title = self.tab_title(tab, cx);
            let is_renaming = renaming == Some(tab.id);
            let label = match &self.rename {
                Some(rename) if is_renaming => div()
                    .key_context(RENAME_CONTEXT)
                    .on_action(cx.listener(|this, _: &CancelRename, window, cx| {
                        this.finish_rename(None, window, cx);
                    }))
                    .w(px(160.))
                    .child(Input::new(&rename.input).xsmall())
                    .into_any_element(),
                _ => div()
                    .max_w(px(220.))
                    .truncate()
                    .child(title.clone())
                    .into_any_element(),
            };
            let close = Button::new(("close-tab", tab.id))
                .icon(IconName::Close)
                .ghost()
                .xsmall()
                // Where a test clicks; a no-op outside test builds.
                .debug_selector(move || format!("tab-close-{ix}"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.ask_close_tab(ix, window, cx);
                }));
            Tab::new()
                .debug_selector(move || format!("tab-{ix}"))
                .child(label)
                .suffix(close)
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_rename(ix, window, cx);
                    } else {
                        this.switch_tab(|tabs| tabs.activate(ix), window, cx);
                    }
                }))
                .on_aux_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.middle_click_tab(event, ix, window, cx);
                }))
                // The whole tab is the drag handle, except while its rename
                // field needs the mouse for selecting text.
                .when(!is_renaming, |this| {
                    let dragged = DraggedTab {
                        index: ix,
                        id: tab.id,
                        title,
                    };
                    this.on_drag(dragged, |tab, _, _, cx| {
                        cx.new(|_| TabGhost(tab.title.clone()))
                    })
                })
                // Mark the side the dragged tab lands on (`Tabs::move_tab`
                // puts it at this tab's index). Tabs have a clear 1px border
                // on each side, so the marker takes both and the width holds.
                .drag_over::<DraggedTab>(move |style, dragged, _, cx| {
                    let marker = cx.theme().drag_border;
                    match dragged.index.cmp(&ix) {
                        Ordering::Less => style.border_l_0().border_r_2().border_color(marker),
                        Ordering::Greater => style.border_l_2().border_r_0().border_color(marker),
                        Ordering::Equal => style,
                    }
                })
                .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                    this.tabs.move_tab(dragged.index, ix);
                    this.save_soon(window, cx);
                    cx.notify();
                }))
                // A pane dropped on a tab becomes the tab after it (spec
                // 0007, flow 5).
                .drag_over::<DraggedPane>(|style, _, _, cx| {
                    let marker = cx.theme().drag_border;
                    style.border_l_0().border_r_2().border_color(marker)
                })
                .on_drop(cx.listener(move |this, dragged: &DraggedPane, window, cx| {
                    this.pane_to_tab(dragged.leaf, ix + 1, window, cx);
                }))
        });

        // Only a bar on top doubles as the title bar; one at the bottom
        // leaves that to `render_title_bar`.
        let client_decorated = matches!(window.window_decorations(), Decorations::Client { .. })
            && self.config.tab_bar == TabBarSide::Top;
        let new_tab = Button::new("new-tab")
            .debug_selector(|| "new-tab".into())
            .icon(IconName::Plus)
            .ghost()
            .small()
            .on_click(cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx)));
        let suffix = div()
            .flex()
            .items_center()
            .child(new_tab)
            .child(sidebar)
            .child(settings)
            .when(client_decorated, |suffix| {
                suffix.child(Self::window_controls(window))
            });
        TabBar::new("tabs")
            .selected_index(self.tabs.active_index())
            .children(tabs)
            .suffix(suffix)
            .last_empty_space(self.title_bar_space(client_decorated, cx))
    }

    /// The tabs as a list down the left or right side, `tab_bar: left` or
    /// `right` (spec 0003): the same clicks, renames, drags and drops as the
    /// bar, the drop markers above and below instead of beside.
    pub(super) fn render_side_tabs(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (bar, active, active_text, text, border) = (
            theme.tab_bar,
            theme.tab_active,
            theme.tab_active_foreground,
            theme.tab_foreground,
            theme.border,
        );
        let renaming = self.rename.as_ref().map(|rename| rename.tab);
        let shown = self.tabs.active_index();
        let tabs = self.tabs.tabs().iter().enumerate().map(|(ix, tab)| {
            let title = self.tab_title(tab, cx);
            let is_renaming = renaming == Some(tab.id);
            let label = match &self.rename {
                Some(rename) if is_renaming => div()
                    .key_context(RENAME_CONTEXT)
                    .on_action(cx.listener(|this, _: &CancelRename, window, cx| {
                        this.finish_rename(None, window, cx);
                    }))
                    .flex_1()
                    .child(Input::new(&rename.input).xsmall())
                    .into_any_element(),
                _ => div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(title.clone())
                    .into_any_element(),
            };
            let close = Button::new(("close-side-tab", tab.id))
                .icon(IconName::Close)
                .ghost()
                .xsmall()
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.ask_close_tab(ix, window, cx);
                }));
            div()
                .id(("side-tab", tab.id))
                .debug_selector(move || format!("side-tab-{ix}"))
                .flex()
                .items_center()
                .gap(px(4.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(4.))
                .border_y_2()
                .border_color(gpui::transparent_black())
                .text_sm()
                .map(|row| match ix == shown {
                    true => row.bg(active).text_color(active_text),
                    false => row.text_color(text),
                })
                .child(label)
                .child(close)
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_rename(ix, window, cx);
                    } else {
                        this.switch_tab(|tabs| tabs.activate(ix), window, cx);
                    }
                }))
                .on_aux_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.middle_click_tab(event, ix, window, cx);
                }))
                .when(!is_renaming, |row| {
                    let dragged = DraggedTab {
                        index: ix,
                        id: tab.id,
                        title,
                    };
                    row.on_drag(dragged, |tab, _, _, cx| {
                        cx.new(|_| TabGhost(tab.title.clone()))
                    })
                })
                .drag_over::<DraggedTab>(move |style, dragged, _, cx| {
                    let marker = cx.theme().drag_border;
                    match dragged.index.cmp(&ix) {
                        Ordering::Less => style.border_color(marker).border_t_0(),
                        Ordering::Greater => style.border_color(marker).border_b_0(),
                        Ordering::Equal => style,
                    }
                })
                .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                    this.tabs.move_tab(dragged.index, ix);
                    this.save_soon(window, cx);
                    cx.notify();
                }))
                .drag_over::<DraggedPane>(|style, _, _, cx| {
                    style.border_color(cx.theme().drag_border).border_t_0()
                })
                .on_drop(cx.listener(move |this, dragged: &DraggedPane, window, cx| {
                    this.pane_to_tab(dragged.leaf, ix + 1, window, cx);
                }))
        });
        let new_tab = Button::new("new-side-tab")
            .icon(IconName::Plus)
            .label("New tab")
            .ghost()
            .small()
            .on_click(cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx)));
        // Below the tabs, a dropped pane becomes the last tab (spec 0007).
        let rest = div()
            .id("side-tabs-rest")
            .flex_1()
            .min_h(px(24.))
            .drag_over::<DraggedPane>(|style, _, _, cx| {
                style.border_t_2().border_color(cx.theme().drag_border)
            })
            .on_drop(cx.listener(|this, dragged: &DraggedPane, window, cx| {
                this.pane_to_tab(dragged.leaf, usize::MAX, window, cx);
            }));
        let _ = window;
        div()
            .id("side-tabs")
            .w(px(200.))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(6.))
            .bg(bar)
            .map(|list| match self.config.tab_bar {
                TabBarSide::Right => list.border_l_1(),
                _ => list.border_r_1(),
            })
            .border_color(border)
            .overflow_y_scroll()
            .children(tabs)
            .child(new_tab)
            .child(rest)
            .child(Self::sidebar_button("side-sidebar-button", cx))
            .child(Self::settings_button("side-settings-button", cx))
    }

    /// The gear in the tab list's corner that opens the settings screen
    /// (spec 0009, flow 1).
    fn settings_button(id: &'static str, cx: &mut Context<Self>) -> Button {
        Button::new(id)
            .icon(IconName::Settings)
            .ghost()
            .small()
            .tooltip("Settings")
            .on_click(
                cx.listener(|this, _, window, cx| this.open_settings(&OpenSettings, window, cx)),
            )
    }

    /// With the tabs down the side, what's left on top where the compositor
    /// draws no frame: the title bar's free space and the window controls
    /// (spec 0003, flow 8).
    pub(super) fn render_title_bar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .h(px(34.))
            .bg(cx.theme().tab_bar)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(self.title_bar_space(true, cx))
            .child(Self::window_controls(window))
    }

    /// The tab bar's free space, where a dropped pane becomes the last tab
    /// (spec 0007, flow 5). When the compositor draws no frame it acts as
    /// the title bar: drag to move, double-click to maximize (spec 0003,
    /// flow 8). A press only becomes a move once the mouse moves, so the
    /// double click still registers.
    fn title_bar_space(&self, client_decorated: bool, cx: &mut Context<Self>) -> AnyElement {
        let space = div()
            .id("title-bar-space")
            .flex_1()
            .h_full()
            .min_w(px(24.))
            .drag_over::<DraggedPane>(|style, _, _, cx| {
                style.border_l_2().border_color(cx.theme().drag_border)
            })
            .on_drop(cx.listener(|this, dragged: &DraggedPane, window, cx| {
                this.pane_to_tab(dragged.leaf, usize::MAX, window, cx);
            }));
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
                    // Through the workspace, which asks first if SFTP edits
                    // would go (spec 0008, flow 3).
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(CloseWindow), cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use crate::config::TabBarSide;
    use crate::layout::sidebar::tests::{Rig, boot, local_and_two_hosts, rig, two_hosts};
    use crate::layout::tests::open_workspace;
    use crate::layout::tree::LeafId;
    use gpui::{Modifiers, MouseButton, Pixels, Point, TestAppContext, point, px};

    impl Rig<'_> {
        /// Each tab by its pane (1 shell, 2 prod, 3 stage), and the shown
        /// one's.
        fn tabs(&mut self) -> (Vec<LeafId>, LeafId) {
            self.workspace.read_with(self.cx, |workspace, _| {
                let tabs = workspace.tabs.tabs().iter().map(|tab| tab.focused);
                (tabs.collect(), workspace.tabs.active().focused)
            })
        }

        /// Where `selector` was last drawn, in its middle.
        fn center_of(&mut self, selector: &'static str) -> Point<Pixels> {
            self.draw();
            let bounds = self.cx.debug_bounds(selector);
            bounds
                .unwrap_or_else(|| panic!("{selector} isn't drawn"))
                .center()
        }

        /// Presses the middle button at `down` and lets go at `up`.
        fn middle_drag(&mut self, down: Point<Pixels>, up: Point<Pixels>) {
            let none = Modifiers::none();
            self.cx.simulate_mouse_down(down, MouseButton::Middle, none);
            self.cx.simulate_mouse_move(up, MouseButton::Middle, none);
            self.cx.simulate_mouse_up(up, MouseButton::Middle, none);
            self.draw();
        }

        fn middle_click(&mut self, at: Point<Pixels>) {
            self.middle_drag(at, at);
        }
    }

    /// AC14 (spec 0003), flow 2: a middle click on an inactive tab in the
    /// bar closes it without showing it first: the tab shown before stays
    /// shown and keeps the keyboard.
    #[gpui::test]
    fn a_middle_click_on_an_inactive_tab_closes_it_and_keeps_the_shown_tab(
        cx: &mut TestAppContext,
    ) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        let prod = rig.center_of("tab-1");
        rig.middle_click(prod);
        assert_eq!(rig.tabs(), (vec![1, 3], 1));
        assert!(rig.pane_has_the_keyboard(1));
    }

    /// AC14 (spec 0003), flow 2: the side list's tabs close on a middle
    /// click the same way.
    #[gpui::test]
    fn a_middle_click_on_a_side_tab_closes_it_and_keeps_the_shown_tab(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let mut config = two_hosts();
        config.tab_bar = TabBarSide::Left;
        let (workspace, cx) = open_workspace(boot(&local_and_two_hosts()), config, cx);
        let mut rig = Rig { workspace, cx };
        let prod = rig.center_of("side-tab-1");
        rig.middle_click(prod);
        assert_eq!(rig.tabs(), (vec![1, 3], 1));
        assert!(rig.pane_has_the_keyboard(1));
    }

    /// AC14 (spec 0003), flow 2: a middle press on one tab let go over
    /// another closes neither.
    #[gpui::test]
    fn a_middle_press_released_over_another_tab_closes_nothing(cx: &mut TestAppContext) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        let (prod, stage) = (rig.center_of("tab-1"), rig.center_of("tab-2"));
        rig.middle_drag(prod, stage);
        assert_eq!(rig.tabs(), (vec![1, 2, 3], 1));
    }

    /// AC14 (spec 0003), flow 2: a middle click on the bar's empty area,
    /// the new-tab button or a pane closes no tab (and opens none).
    #[gpui::test]
    fn a_middle_click_off_the_tabs_closes_nothing(cx: &mut TestAppContext) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        let last_tab = rig.center_of("tab-2");
        let bounds = rig.cx.debug_bounds("tab-2").expect("drawn");
        let empty = point(bounds.right() + px(30.), last_tab.y);
        let new_tab = rig.center_of("new-tab");
        let size = rig.cx.update(|window, _| window.viewport_size());
        let pane = point(size.width / 2., size.height / 2.);
        for at in [empty, new_tab, pane] {
            rig.middle_click(at);
            assert_eq!(rig.tabs(), (vec![1, 2, 3], 1), "at {at:?}");
        }
    }
}
