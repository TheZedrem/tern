//! The Port forwards view of an SSH pane's sidebar (spec 0017, flows 1–6):
//! the add row (kind, listen port, `to`, "Let other machines connect") and the
//! host's forwards with their state, Retry and Delete. It draws what the
//! [`Registry`] holds; the pane it belongs to builds each forward's ssh,
//! so its prompts open over that pane.
//!
//! Keys (keyboard first): Tab and Shift+Tab walk the controls, Enter in a
//! field adds, Esc closes the sidebar. In the list, Up and Down select,
//! Enter retries a failed forward, Delete removes one.

use super::registry::{Entry, Host, Registry, State};
use super::{Forward, Id, Kind};
use crate::terminal::TerminalPane;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, actions, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Disableable, IconName, Sizable};

/// The view's key context.
const CONTEXT: &str = "PortForwards";
/// The key context of the list of forwards.
const LIST_CONTEXT: &str = "PortForwardsList";

actions!(
    forwards,
    [
        SelectPrevious,
        SelectNext,
        RetrySelected,
        DeleteSelected,
        FocusNextControl,
        FocusPreviousControl,
        Dismiss
    ]
);

/// Binds the view's keys. The widget library binds Tab nowhere, so it's
/// bound here; Enter isn't bound on the view itself, so a focused button
/// still gets it.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", FocusNextControl, Some(CONTEXT)),
        KeyBinding::new("shift-tab", FocusPreviousControl, Some(CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(CONTEXT)),
        KeyBinding::new("up", SelectPrevious, Some(LIST_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(LIST_CONTEXT)),
        KeyBinding::new("enter", RetrySelected, Some(LIST_CONTEXT)),
        KeyBinding::new("delete", DeleteSelected, Some(LIST_CONTEXT)),
        KeyBinding::new("backspace", DeleteSelected, Some(LIST_CONTEXT)),
    ]);
}

/// What the add row says for a kind, so it's clear which side listens and
/// where the forward leads (spec 0017, flow 2).
struct Words {
    what: SharedString,
    listen: SharedString,
    to: Option<SharedString>,
    /// Where it listens without "Let other machines connect".
    local: SharedString,
    /// The warning with it (flow 3).
    network: SharedString,
}

impl Words {
    fn of(kind: Kind, host: &str) -> Self {
        match kind {
            Kind::Local => Self {
                what: format!("A port on this machine that leads to a host:port {host} can reach.")
                    .into(),
                listen: "Port on this machine".into(),
                to: Some(format!("Leads to (as {host} sees it)").into()),
                local: "Only this machine can connect (127.0.0.1)".into(),
                network: "Reachable from other machines on the network".into(),
            },
            Kind::Remote => Self {
                what: format!("A port on {host} that leads to a host:port this machine can reach.")
                    .into(),
                listen: format!("Port on {host}").into(),
                to: Some("Leads to (as this machine sees it)".into()),
                local: format!("Only {host} itself can connect (127.0.0.1)").into(),
                network: format!(
                    "Reachable from other machines on {host}'s network, if the server allows it"
                )
                .into(),
            },
            Kind::Socks => Self {
                what: format!(
                    "A SOCKS proxy on this machine: apps set to use it reach what {host} can reach."
                )
                .into(),
                listen: "Port on this machine".into(),
                to: None,
                local: "Only this machine can connect (127.0.0.1)".into(),
                network: format!("An open proxy into {host}'s network").into(),
            },
        }
    }
}

/// What a line of the list shows, read out of the registry.
struct Row {
    id: Id,
    summary: String,
    state: State,
}

impl Row {
    fn of(entry: &Entry) -> Self {
        Self {
            id: entry.id,
            summary: entry.forward.summary(),
            state: entry.state.clone(),
        }
    }
}

pub enum ForwardsEvent {
    /// The user closed the sidebar from here.
    Close,
}

pub struct ForwardsView {
    focus: FocusHandle,
    list_focus: FocusHandle,
    /// The SSH pane whose sidebar this is: it starts the forwards' ssh.
    pane: WeakEntity<TerminalPane>,
    host: Host,
    registry: Entity<Registry>,
    kind: Kind,
    listen: Entity<InputState>,
    to: Entity<InputState>,
    /// "Let other machines connect", off each time the row is used (flow 3).
    network: bool,
    selected: Option<Id>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ForwardsEvent> for ForwardsView {}

impl Focusable for ForwardsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ForwardsView {
    pub fn new(
        pane: WeakEntity<TerminalPane>,
        host: Host,
        registry: Entity<Registry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let listen = cx.new(|cx| InputState::new(window, cx).placeholder("5432"));
        let to = cx.new(|cx| InputState::new(window, cx).placeholder("host:port"));
        let mut subscriptions = vec![cx.observe(&registry, |_, _, cx| cx.notify())];
        for input in [&listen, &to] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.add(window, cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ));
        }
        Self {
            focus: cx.focus_handle(),
            list_focus: cx.focus_handle(),
            pane,
            host,
            registry,
            kind: Kind::default(),
            listen,
            to,
            network: false,
            selected: None,
            _subscriptions: subscriptions,
        }
    }

    /// Puts the keyboard in the listen field, where adding starts.
    pub fn focus_add(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.listen.update(cx, |input, cx| input.focus(window, cx));
    }

    /// What the add row describes, or why Add is disabled (flow 2).
    fn draft(&self, cx: &App) -> Result<Forward, &'static str> {
        let listen = self.listen.read(cx).value();
        let to = self.to.read(cx).value();
        let forward = Forward::parse(self.kind, &listen, &to, self.network)?;
        if self.registry.read(cx).clashes(&self.host.args, &forward) {
            return Err("This host already has a forward on that port");
        }
        Ok(forward)
    }

    /// Starts the forward the add row describes, if it's complete.
    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(forward) = self.draft(cx) else {
            return;
        };
        let Some(pane) = self.pane.upgrade() else {
            return;
        };
        let id = self.registry.update(cx, |registry, _| registry.next_id());
        let command = pane.update(cx, |pane, cx| {
            pane.forward_command(id, &forward, window, cx)
        });
        let Some(command) = command else {
            return;
        };
        let host = self.host.clone();
        self.registry.update(cx, |registry, cx| {
            registry.add(id, host, forward, &command, cx);
        });
        for input in [&self.listen, &self.to] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.network = false;
        self.selected = Some(id);
        cx.notify();
    }

    fn retry(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(forward) = self.registry.read(cx).get(id).map(|e| e.forward.clone()) else {
            return;
        };
        let Some(pane) = self.pane.upgrade() else {
            return;
        };
        let command = pane.update(cx, |pane, cx| {
            pane.forward_command(id, &forward, window, cx)
        });
        if let Some(command) = command {
            self.registry
                .update(cx, |registry, cx| registry.retry(id, &command, cx));
        }
    }

    fn delete(&mut self, id: Id, cx: &mut Context<Self>) {
        let ids = self.ids(cx);
        let next = ids
            .iter()
            .position(|&shown| shown == id)
            .and_then(|at| {
                ids.get(at + 1)
                    .or_else(|| at.checked_sub(1).and_then(|p| ids.get(p)))
            })
            .copied();
        self.registry
            .update(cx, |registry, cx| registry.remove(id, cx));
        self.selected = next;
        cx.notify();
    }

    /// The host's forwards' IDs, as listed.
    fn ids(&self, cx: &App) -> Vec<Id> {
        self.registry
            .read(cx)
            .of(&self.host.args)
            .map(|entry| entry.id)
            .collect()
    }

    /// The selected forward, if it's still listed, else the first.
    fn current(&self, cx: &App) -> Option<Id> {
        let ids = self.ids(cx);
        self.selected
            .filter(|id| ids.contains(id))
            .or_else(|| ids.first().copied())
    }

    fn select(&mut self, step: isize, cx: &mut Context<Self>) {
        let ids = self.ids(cx);
        let at = self
            .current(cx)
            .and_then(|id| ids.iter().position(|&shown| shown == id));
        let next = match at {
            Some(at) => at
                .saturating_add_signed(step)
                .min(ids.len().saturating_sub(1)),
            None => 0,
        };
        self.selected = ids.get(next).copied();
        cx.notify();
    }

    fn render_add(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, danger, warning, border) = (
            theme.muted_foreground,
            theme.danger,
            theme.warning,
            theme.border,
        );
        let draft = self.draft(cx);
        let kinds = Kind::ALL.into_iter().map(|kind| {
            let button = Button::new(SharedString::from(format!("forward-kind-{}", kind.label())))
                .label(kind.label())
                .small()
                .flex_1()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.kind = kind;
                    cx.notify();
                }));
            if self.kind == kind {
                button.primary()
            } else {
                button.outline()
            }
        });
        let words = Words::of(self.kind, &self.host.label);
        let label = |text: SharedString| div().text_xs().text_color(muted).child(text);
        // Only say what's wrong once something's typed.
        let typed =
            !self.listen.read(cx).value().is_empty() || !self.to.read(cx).value().is_empty();
        let problem = draft.as_ref().err().copied().filter(|_| typed);
        let (note, note_color) = match self.network {
            true => (words.network, warning),
            false => (words.local, muted),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(8.))
            .py(px(8.))
            .border_b_1()
            .border_color(border)
            .child(div().flex().gap(px(4.)).children(kinds))
            .child(div().text_xs().text_color(muted).child(words.what))
            .child(label(words.listen))
            .child(Input::new(&self.listen).small())
            .when_some(words.to, |row, to| {
                row.child(label(to)).child(Input::new(&self.to).small())
            })
            .child(
                Checkbox::new("forward-network")
                    .label("Let other machines connect")
                    .checked(self.network)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.network = *checked;
                        cx.notify();
                    })),
            )
            .child(div().text_xs().text_color(note_color).child(note))
            .children(problem.map(|text| div().text_xs().text_color(danger).child(text)))
            .child(
                Button::new("forward-add")
                    .label("Add forward")
                    .primary()
                    .small()
                    .w_full()
                    .disabled(draft.is_err())
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.add(window, cx);
                    })),
            )
    }

    fn render_entry(&self, entry: Row, selected: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, danger, success, active) = (
            theme.muted_foreground,
            theme.danger,
            theme.success,
            theme.list_active,
        );
        let id = entry.id;
        let failed = matches!(entry.state, State::Failed(_));
        let (state, color) = match entry.state {
            State::Starting => (SharedString::from("Starting…"), muted),
            State::Up => (SharedString::from("Up"), success),
            State::Failed(reason) => (reason, danger),
        };
        div()
            .id(("forward", id))
            .px(px(8.))
            .py(px(4.))
            .flex()
            .items_start()
            .gap(px(4.))
            .when(selected, |row| row.bg(active))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.selected = Some(id);
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_sm().truncate().child(entry.summary))
                    .child(
                        div()
                            .text_xs()
                            .text_color(color)
                            .whitespace_normal()
                            .child(state),
                    ),
            )
            // The list's own keys reach these: no tab stops of their own.
            .when(failed, |row| {
                row.child(
                    Button::new(("forward-retry", id))
                        .icon(IconName::RefreshCw)
                        .ghost()
                        .xsmall()
                        .tab_stop(false)
                        .tooltip("Retry (Enter)")
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.retry(id, window, cx);
                        })),
                )
            })
            .child(
                Button::new(("forward-delete", id))
                    .icon(IconName::Delete)
                    .ghost()
                    .xsmall()
                    .tab_stop(false)
                    .tooltip("Delete (Del)")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.delete(id, cx))),
            )
    }
}

impl Render for ForwardsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let current = self.current(cx);
        let rows: Vec<Row> = self
            .registry
            .read(cx)
            .of(&self.host.args)
            .map(Row::of)
            .collect();
        let entries: Vec<_> = rows
            .into_iter()
            .map(|row| {
                let selected = Some(row.id) == current;
                self.render_entry(row, selected, cx).into_any_element()
            })
            .collect();
        let empty = entries.is_empty();
        let header = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(8.))
            .py(px(4.))
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .child(self.host.label.clone()),
            )
            .child(
                Button::new("forwards-close")
                    .icon(IconName::Close)
                    .ghost()
                    .xsmall()
                    .tooltip("Close sidebar (Esc)")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ForwardsEvent::Close))),
            );
        let list = div()
            .id("forwards-list")
            .key_context(LIST_CONTEXT)
            .track_focus(&self.list_focus)
            .tab_stop(true)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.select(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.select(1, cx)))
            .on_action(cx.listener(|this, _: &RetrySelected, window, cx| {
                if let Some(id) = this.current(cx) {
                    this.retry(id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteSelected, _, cx| {
                if let Some(id) = this.current(cx) {
                    this.delete(id, cx);
                }
            }))
            .when(empty, |list| {
                list.child(
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .text_xs()
                        .text_color(muted)
                        .child("No port forwards to this host"),
                )
            })
            .children(entries);
        div()
            .id("port-forwards")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .tab_group()
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|_, _: &FocusNextControl, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &FocusPreviousControl, window, cx| {
                window.focus_prev(cx);
            }))
            .on_action(cx.listener(|_, _: &Dismiss, _, cx| cx.emit(ForwardsEvent::Close)))
            .child(header)
            .child(self.render_add(cx))
            .child(list)
    }
}
