//! The sidebar of one SSH pane (spec 0008): its host in the header, the
//! path field, the flat listing, and the connection's state. It draws what
//! its [`Client`] reports and never waits on the network itself.

use super::client::{Client, Event};
use super::{Entry, Listing, MAX_ENTRIES};
use crate::terminal::{TerminalPane, clean_title};
use futures::StreamExt;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, ScrollStrategy,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, UniformListScrollHandle,
    WeakEntity, Window, actions, div, px, uniform_list,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Icon, IconName, Sizable};

/// The sidebar's key context, where its keys are bound.
const CONTEXT: &str = "SftpSidebar";
/// The height of a listing row, in logical pixels.
const ROW_HEIGHT: f32 = 24.0;

actions!(sftp, [SelectPrevious, SelectNext, OpenSelected, GoUp]);

/// Binds the listing's keys: arrows move, Enter opens, Backspace goes up
/// (spec 0008, flow 2). The path field's own bindings win while it has
/// the keyboard.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, Some(CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("enter", OpenSelected, Some(CONTEXT)),
        KeyBinding::new("backspace", GoUp, Some(CONTEXT)),
    ]);
}

/// What the sidebar asks of the workspace.
pub enum SftpEvent {
    /// Its close button: hide the sidebar, keep the session.
    Close,
}

/// Where the connection is.
enum Connection {
    Connecting,
    Connected {
        home: String,
    },
    /// Over, and why.
    Ended(SharedString),
}

pub struct SftpView {
    focus: FocusHandle,
    /// The pane whose host this is, which builds each connection's command.
    pane: WeakEntity<TerminalPane>,
    /// `user@host`, always in the header (TM-sftp-01).
    target: SharedString,
    connection: Connection,
    client: Option<Client>,
    _events: Option<Task<()>>,
    /// What's listed; kept when a later listing fails.
    listing: Option<Listing>,
    /// The folder being listed now.
    loading: Option<String>,
    /// Why the last listing failed, shown above the one kept.
    error: Option<SharedString>,
    /// The row Enter acts on.
    selected: Option<usize>,
    path: Entity<InputState>,
    scroll: UniformListScrollHandle,
    _path_events: Subscription,
}

impl EventEmitter<SftpEvent> for SftpView {}

impl SftpView {
    /// The sidebar of `pane`, which starts connecting right away (spec
    /// 0008, flow 1).
    pub fn new(pane: &Entity<TerminalPane>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let path = cx.new(|cx| InputState::new(window, cx).placeholder("Path"));
        let path_events = cx.subscribe_in(&path, window, |this, input, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let typed = input.read(cx).value().to_string();
                this.go(&typed, window, cx);
            }
        });
        let target = pane.read(cx).ssh_target().unwrap_or_default();
        let mut view = Self {
            focus: cx.focus_handle(),
            pane: pane.downgrade(),
            target: target.into(),
            connection: Connection::Connecting,
            client: None,
            _events: None,
            listing: None,
            loading: None,
            error: None,
            selected: None,
            path,
            scroll: UniformListScrollHandle::new(),
            _path_events: path_events,
        };
        view.connect(window, cx);
        view
    }

    /// Starts a connection with a new askpass token from the pane; the
    /// last one, if any, ends.
    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.client = None;
        self.loading = None;
        self.error = None;
        let command = self
            .pane
            .update(cx, |pane, cx| pane.sftp_command(window, cx))
            .ok()
            .flatten();
        let Some(command) = command else {
            self.connection = Connection::Ended("This pane has no SSH connection".into());
            cx.notify();
            return;
        };
        self.connection = Connection::Connecting;
        let (client, mut events) = Client::connect(command);
        self.client = Some(client);
        self._events = Some(cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = events.next().await {
                let handled = this.update_in(cx, |view, window, cx| view.event(event, window, cx));
                if handled.is_err() {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            // TODO(T-022): start in the folder the pane's shell reported.
            Event::Connected { home } => {
                self.connection = Connection::Connected { home: home.clone() };
                self.list(home);
            }
            Event::Listed(listing) => {
                self.loading = None;
                self.error = None;
                self.path.update(cx, |input, cx| {
                    input.set_value(listing.path.clone(), window, cx)
                });
                self.selected = (!listing.entries.is_empty()).then_some(0);
                self.scroll.scroll_to_item(0, ScrollStrategy::Top);
                self.listing = Some(listing);
            }
            Event::ListFailed { path, error } => {
                self.loading = None;
                self.error = Some(format!("Couldn't open {}: {error}", clean_title(&path)).into());
            }
            Event::Ended(reason) => {
                self.client = None;
                self.loading = None;
                self.connection = Connection::Ended(reason.into());
            }
        }
        cx.notify();
    }

    /// Asks for the folder at absolute `path`.
    fn list(&mut self, path: String) {
        if let Some(client) = &self.client {
            self.loading = Some(path.clone());
            client.list(path);
        }
    }

    /// Lists what was typed into the path field (spec 0008, Model).
    fn go(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Connection::Connected { home } = &self.connection {
            let path = super::resolve(home, typed);
            self.list(path);
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Enters the folder at `ix`. Files wait for downloads and edits
    /// (T-035, T-036).
    fn open(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(listing) = &self.listing else {
            return;
        };
        if let Some(entry) = listing.entries.get(ix).filter(|entry| entry.is_folder()) {
            let path = super::join(&listing.path, &entry.name);
            self.list(path);
            cx.notify();
        }
    }

    fn go_up(&mut self, cx: &mut Context<Self>) {
        if let Some(listing) = &self.listing {
            let parent = super::parent(&listing.path);
            if parent != listing.path {
                self.list(parent);
                cx.notify();
            }
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.listing.as_ref().map(|listing| listing.path.clone()) {
            self.list(path);
            cx.notify();
        }
    }

    fn select(&mut self, step: isize, cx: &mut Context<Self>) {
        let count = self
            .listing
            .as_ref()
            .map_or(0, |listing| listing.entries.len());
        if count == 0 {
            return;
        }
        let next = match self.selected {
            Some(ix) => ix.saturating_add_signed(step).min(count - 1),
            None => 0,
        };
        self.selected = Some(next);
        self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn render_row(&self, ix: usize, entry: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, selected_bg, hover) =
            (theme.muted_foreground, theme.list_active, theme.list_hover);
        let icon = if entry.is_folder() {
            IconName::Folder
        } else {
            IconName::File
        };
        let size = entry
            .size
            .filter(|_| !entry.is_folder())
            .map(super::human_size)
            .unwrap_or_default();
        let modified = entry.modified.map(super::date).unwrap_or_default();
        div()
            .id(("sftp-entry", ix))
            .h(px(ROW_HEIGHT))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .when(self.selected == Some(ix), |row| row.bg(selected_bg))
            .hover(|row| row.bg(hover))
            .child(Icon::new(icon).xsmall().text_color(muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(entry.link, |name| name.italic())
                    .child(clean_title(&entry.name)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(muted)
                    .child(size),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(muted)
                    .child(modified),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.selected = Some(ix);
                if event.click_count() >= 2 {
                    this.open(ix, cx);
                }
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_status(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let line = |text: SharedString| div().px(px(8.)).py(px(4.)).text_xs().child(text);
        match &self.connection {
            Connection::Connecting => Some(
                line("Connecting…".into())
                    .text_color(muted)
                    .into_any_element(),
            ),
            Connection::Ended(reason) => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(line("ssh said:".into()).text_color(muted))
                    .child(line(reason.clone()).text_color(danger).whitespace_normal())
                    .child(
                        div().px(px(8.)).child(
                            Button::new("sftp-reconnect")
                                .label("Connect again")
                                .small()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.connect(window, cx)),
                                ),
                        ),
                    )
                    .into_any_element(),
            ),
            Connection::Connected { .. } => match (&self.error, &self.loading) {
                (Some(error), _) => Some(line(error.clone()).text_color(danger).into_any_element()),
                (None, Some(_)) => {
                    Some(line("Loading…".into()).text_color(muted).into_any_element())
                }
                (None, None) => None,
            },
        }
    }
}

impl Focusable for SftpView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SftpView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let header = div()
            .flex()
            .items_center()
            .gap(px(4.))
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
                    .child(clean_title(&self.target)),
            )
            .child(
                Button::new("sftp-refresh")
                    .icon(IconName::RefreshCw)
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(
                Button::new("sftp-close")
                    .icon(IconName::Close)
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SftpEvent::Close))),
            );
        let path_row = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(6.))
            .py(px(4.))
            .child(
                Button::new("sftp-up")
                    .icon(IconName::ArrowUp)
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|this, _, _, cx| this.go_up(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.path).xsmall()),
            );
        let entries = self
            .listing
            .as_ref()
            .map_or(0, |listing| listing.entries.len());
        let list = match &self.listing {
            Some(listing) if listing.entries.is_empty() => div()
                .px(px(8.))
                .py(px(4.))
                .text_xs()
                .text_color(muted)
                .child("Empty folder")
                .into_any_element(),
            Some(_) => uniform_list(
                "sftp-entries",
                entries,
                cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                    let Some(listing) = this.listing.take() else {
                        return Vec::new();
                    };
                    let rows: Vec<AnyElement> = range
                        .filter_map(|ix| Some(this.render_row(ix, listing.entries.get(ix)?, cx)))
                        .collect();
                    this.listing = Some(listing);
                    rows
                }),
            )
            .track_scroll(&self.scroll)
            .flex_1()
            .into_any_element(),
            None => div().into_any_element(),
        };
        let truncated = self
            .listing
            .as_ref()
            .filter(|listing| listing.truncated)
            .map(|_| {
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_xs()
                    .text_color(muted)
                    .child(format!("Showing the first {MAX_ENTRIES} entries"))
            });
        div()
            .id("sftp-sidebar")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.select(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.select(1, cx)))
            .on_action(cx.listener(|this, _: &OpenSelected, _, cx| {
                if let Some(ix) = this.selected {
                    this.open(ix, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &GoUp, _, cx| this.go_up(cx)))
            .size_full()
            .flex()
            .flex_col()
            .text_sm()
            .child(header)
            .child(path_row)
            .children(self.render_status(cx))
            .child(list)
            .children(truncated)
    }
}
