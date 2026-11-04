//! The command palette (spec 0004): gpui-component's command list over the
//! app commands and profiles, or over the themes as the theme picker,
//! ranked by [`fuzzy`] instead of the list's own substring filter.

use super::{AppCommand, fuzzy};
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, div, px,
};
use gpui_component::command::{Command, CommandItem, CommandState};
use gpui_component::{ActiveTheme, Disableable, IndexPath};

/// One line of the palette.
pub enum Entry {
    /// Runs through its action. One that can't run now is dimmed (flow 3).
    Command {
        command: &'static AppCommand,
        title: &'static str,
        enabled: bool,
    },
    /// Opens the profile in a new tab (spec 0005).
    Profile { id: String, title: String },
    /// A theme of the theme picker; `current` is the one saved.
    Theme { name: String, current: bool },
}

impl Entry {
    fn title(&self) -> &str {
        match self {
            Entry::Command { title, .. } => title,
            Entry::Profile { title, .. } => title,
            Entry::Theme { name, .. } => name,
        }
    }

    fn item(&self) -> CommandItem {
        match self {
            Entry::Command {
                command,
                title,
                enabled,
            } => CommandItem::new()
                .label(*title)
                .action((command.action)())
                .disabled(!enabled),
            Entry::Profile { id, title } => CommandItem::new()
                .label(title.clone())
                .keywords([id.clone()]),
            Entry::Theme { name, current } => {
                CommandItem::new().label(name.clone()).checked(*current)
            }
        }
    }
}

/// What the palette asks the workspace to do.
pub enum PaletteEvent {
    OpenProfile(String),
    /// Show theme `name` without saving it.
    PreviewTheme(String),
    /// Save theme `name` as `theme.default`.
    ChooseTheme(String),
    /// Done: after running an entry, or `cancelled` by Esc.
    Closed {
        cancelled: bool,
    },
}

pub struct Palette {
    state: Entity<CommandState>,
    entries: Vec<Entry>,
    /// Indices into `entries`, in the order the query ranks them.
    shown: Vec<usize>,
    /// The highlighted entry, an index into `entries`.
    highlighted: Option<usize>,
    /// The entry to highlight once the list has been drawn.
    pending: Option<usize>,
    placeholder: SharedString,
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Palette {
    /// A focused palette over `entries`, with entry `highlight` highlighted
    /// (else the first).
    pub fn new(
        entries: Vec<Entry>,
        placeholder: impl Into<SharedString>,
        highlight: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |state, cx| state.focus(window, cx));
        Self {
            state,
            shown: (0..entries.len()).collect(),
            entries,
            highlighted: None,
            pending: highlight,
            placeholder: placeholder.into(),
        }
    }

    fn rank(&mut self, query: &str) {
        self.shown = fuzzy::rank(query, self.entries.iter().map(Entry::title));
    }

    /// The `entries` index of the list row `index`.
    fn entry_ix(&self, index: IndexPath) -> Option<usize> {
        self.shown.get(index.row).copied()
    }

    fn highlight(&mut self, index: IndexPath, cx: &mut Context<Self>) {
        self.highlighted = self.entry_ix(index);
        if let Some(Entry::Theme { name, .. }) =
            self.highlighted.and_then(|ix| self.entries.get(ix))
        {
            cx.emit(PaletteEvent::PreviewTheme(name.clone()));
        }
        cx.notify();
    }

    fn confirm(&mut self, index: IndexPath, cx: &mut Context<Self>) {
        match self.entry_ix(index).and_then(|ix| self.entries.get(ix)) {
            Some(Entry::Profile { id, .. }) => cx.emit(PaletteEvent::OpenProfile(id.clone())),
            Some(Entry::Theme { name, .. }) => cx.emit(PaletteEvent::ChooseTheme(name.clone())),
            // A command has run through its action already.
            Some(Entry::Command { .. }) | None => {}
        }
        cx.emit(PaletteEvent::Closed { cancelled: false });
    }
}

impl Render for Palette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(entry) = self.pending.take()
            && let Some(row) = self.shown.iter().position(|&ix| ix == entry)
        {
            // The list knows its rows only after it has been drawn once.
            let state = self.state.clone();
            window.on_next_frame(move |window, cx| {
                state.update(cx, |state, cx| {
                    let index = IndexPath {
                        section: 0,
                        row,
                        column: 0,
                    };
                    state.set_selected_index(Some(index), window, cx);
                });
            });
        }
        let palette = cx.entity().downgrade();
        let items = self
            .shown
            .iter()
            .filter_map(|&ix| self.entries.get(ix))
            .map(Entry::item);
        // The ID `keybindings:` in config.yaml binds keys to.
        let id = self.highlighted.and_then(|ix| match self.entries.get(ix) {
            Some(Entry::Command { command, .. }) => Some(command.id),
            _ => None,
        });
        let muted = cx.theme().muted_foreground;
        Command::new(&self.state)
            .items(items)
            .filterable(false)
            .placeholder(self.placeholder.clone())
            .max_h(px(360.))
            .on_query({
                let palette = palette.clone();
                move |query: &str, _: &mut Window, cx: &mut App| {
                    let _ = palette.update(cx, |palette, cx| {
                        palette.rank(query);
                        cx.notify();
                    });
                }
            })
            .on_select({
                let palette = palette.clone();
                move |index, _: &mut Window, cx: &mut App| {
                    let _ = palette.update(cx, |palette, cx| palette.highlight(index, cx));
                }
            })
            .on_confirm({
                let palette = palette.clone();
                move |index, _: &mut Window, cx: &mut App| {
                    let _ = palette.update(cx, |palette, cx| palette.confirm(index, cx));
                }
            })
            .on_cancel(move |_: &mut Window, cx: &mut App| {
                let _ = palette.update(cx, |_, cx| {
                    cx.emit(PaletteEvent::Closed { cancelled: true });
                });
            })
            .footer(move |_: &CommandState, _: &mut Window, _: &mut App| {
                div()
                    .px(px(12.))
                    .py(px(6.))
                    .text_xs()
                    .text_color(muted)
                    .child(id.unwrap_or_default())
            })
    }
}
