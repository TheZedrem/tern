//! The command palette (spec 0004): gpui-component's command list over the
//! app commands and profiles, over the themes as the theme picker, or over
//! the hosts, grouped, as the host picker (spec 0006), which connects to a
//! host, forgets its saved password, or with the local profiles splits the
//! focused pane with one (spec 0007). Ranked by [`fuzzy`] instead of the
//! list's own substring filter.

use super::{AppCommand, fuzzy};
use crate::layout::tree::Axis;
use crate::settings::change::Change;
use crate::ssh::Host;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, div, px,
};
use gpui_component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_component::{ActiveTheme, Disableable, IndexPath};
use std::borrow::Cow;

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
    /// Connects to the host in a new tab (spec 0006).
    Host(Host),
    /// Forgets what the keychain holds for the host (spec 0006, flow 7).
    ForgetHost(Host),
    /// Splits the focused pane along `axis` with the host or local profile
    /// (spec 0007, flow 6).
    SplitWith { host: Host, axis: Axis },
    /// A value for a setting the settings screen picks from a list (spec
    /// 0009); `current` is the one the file has.
    Setting {
        title: String,
        current: bool,
        change: Change,
    },
}

impl Entry {
    fn title(&self) -> &str {
        match self {
            Entry::Command { title, .. } => title,
            Entry::Profile { title, .. } => title,
            Entry::Setting { title, .. } => title,
            Entry::Theme { name, .. } => name,
            Entry::Host(host) | Entry::ForgetHost(host) | Entry::SplitWith { host, .. } => {
                &host.label
            }
        }
    }

    fn host(&self) -> Option<&Host> {
        match self {
            Entry::Host(host) | Entry::ForgetHost(host) | Entry::SplitWith { host, .. } => {
                Some(host)
            }
            _ => None,
        }
    }

    /// What a query is matched against: a host by its label, where it
    /// connects and its group (spec 0006, flow 1).
    fn haystack(&self) -> Cow<'_, str> {
        match self.host() {
            Some(host) => format!("{} {} {}", host.label, host.destination, host.group).into(),
            None => self.title().into(),
        }
    }

    fn group(&self) -> Option<&str> {
        self.host().map(|host| host.group.as_str())
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
            Entry::Setting { title, current, .. } => {
                CommandItem::new().label(title.clone()).checked(*current)
            }
            Entry::Host(host) | Entry::ForgetHost(host) | Entry::SplitWith { host, .. } => {
                CommandItem::new().label(host.label.clone())
            }
        }
    }
}

/// What the palette asks the workspace to do.
pub enum PaletteEvent {
    OpenProfile(String),
    /// Forget what's saved for the host with this ID.
    ForgetHost(String),
    /// Split the focused pane along `axis` with the profile with this ID.
    SplitWith {
        id: String,
        axis: Axis,
    },
    /// Show theme `name` without saving it.
    PreviewTheme(String),
    /// Save theme `name` as `theme.default`.
    ChooseTheme(String),
    /// Write a setting picked from a list (spec 0009).
    Change(Change),
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
    /// What an empty list says.
    empty: SharedString,
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
            empty: "Nothing matches".into(),
        }
    }

    /// What the list says when it has nothing to show.
    pub fn with_empty(mut self, empty: impl Into<SharedString>) -> Self {
        self.empty = empty.into();
        self
    }

    fn rank(&mut self, query: &str) {
        let haystacks: Vec<Cow<str>> = self.entries.iter().map(Entry::haystack).collect();
        self.shown = fuzzy::rank(query, haystacks.iter().map(|haystack| &**haystack));
    }

    /// The shown entries by group, a group where its best match ranks.
    /// Entries without a group come first, as the list draws them.
    fn sections(&self) -> Vec<(Option<&str>, Vec<usize>)> {
        let mut sections: Vec<(Option<&str>, Vec<usize>)> = Vec::new();
        for &ix in &self.shown {
            let group = self.entries.get(ix).and_then(Entry::group);
            match sections.iter_mut().find(|(known, _)| *known == group) {
                Some((_, entries)) => entries.push(ix),
                None => sections.push((group, vec![ix])),
            }
        }
        sections.sort_by_key(|(group, _)| group.is_some());
        sections
    }

    /// The `entries` index of the list row `index`.
    fn entry_ix(&self, index: IndexPath) -> Option<usize> {
        let sections = self.sections();
        let (_, entries) = sections.get(index.section)?;
        entries.get(index.row).copied()
    }

    /// The list row of entry `ix`.
    fn index_of(&self, ix: usize) -> Option<IndexPath> {
        self.sections()
            .iter()
            .enumerate()
            .find_map(|(section, (_, entries))| {
                let row = entries.iter().position(|&entry| entry == ix)?;
                Some(IndexPath {
                    section,
                    row,
                    column: 0,
                })
            })
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
            Some(Entry::Host(host)) => cx.emit(PaletteEvent::OpenProfile(host.id.clone())),
            Some(Entry::ForgetHost(host)) => cx.emit(PaletteEvent::ForgetHost(host.id.clone())),
            Some(Entry::SplitWith { host, axis }) => cx.emit(PaletteEvent::SplitWith {
                id: host.id.clone(),
                axis: *axis,
            }),
            Some(Entry::Theme { name, .. }) => cx.emit(PaletteEvent::ChooseTheme(name.clone())),
            Some(Entry::Setting { change, .. }) => cx.emit(PaletteEvent::Change(change.clone())),
            // A command has run through its action already.
            Some(Entry::Command { .. }) | None => {}
        }
        cx.emit(PaletteEvent::Closed { cancelled: false });
    }
}

impl Render for Palette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(index) = self.pending.take().and_then(|entry| self.index_of(entry)) {
            // The list knows its rows only after it has been drawn once.
            let state = self.state.clone();
            window.on_next_frame(move |window, cx| {
                state.update(cx, |state, cx| {
                    state.set_selected_index(Some(index), window, cx);
                });
            });
        }
        let palette = cx.entity().downgrade();
        // Under the list: a command's ID, which `keybindings:` in
        // config.yaml binds keys to, or where a host connects.
        let footer: SharedString = match self.highlighted.and_then(|ix| self.entries.get(ix)) {
            Some(Entry::Command { command, .. }) => command.id.into(),
            Some(Entry::Host(host) | Entry::ForgetHost(host) | Entry::SplitWith { host, .. }) => {
                host.destination.clone().into()
            }
            _ => SharedString::default(),
        };
        let muted = cx.theme().muted_foreground;
        let mut list = Command::new(&self.state);
        for (group, entries) in self.sections() {
            let items = entries
                .iter()
                .filter_map(|&ix| self.entries.get(ix))
                .map(Entry::item);
            list = match group {
                Some(group) => list.group(CommandGroup::new().label(group.to_owned()).items(items)),
                None => list.items(items),
            };
        }
        let empty = self.empty.clone();
        list.filterable(false)
            .placeholder(self.placeholder.clone())
            .max_h(px(360.))
            .empty(move |_: &CommandState, _: &mut Window, _: &mut App| {
                div()
                    .px(px(12.))
                    .py(px(8.))
                    .text_sm()
                    .text_color(muted)
                    .child(empty.clone())
            })
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
                    .child(footer.clone())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec 0006, flows 1 and 7: forgetting picks the host in the same
    /// picker as connecting: listed by its label, found by its label, where
    /// it connects or its group, and shown in its group.
    #[test]
    fn the_forget_picker_lists_finds_and_groups_hosts_like_the_connect_picker() {
        let host = Host {
            id: "db".into(),
            label: "Postgres".into(),
            group: "Databases".into(),
            destination: "admin@db.example.com:2222".into(),
        };
        for entry in [Entry::Host(host.clone()), Entry::ForgetHost(host.clone())] {
            assert_eq!(entry.title(), "Postgres");
            assert_eq!(entry.group(), Some("Databases"));
            for query in ["postg", "db.example", "datab"] {
                let haystack = entry.haystack();
                assert_eq!(
                    fuzzy::rank(query, [&*haystack]),
                    [0],
                    "`{query}` in {haystack}"
                );
            }
        }
    }

    /// Spec 0007, flow 6: the split picker lists, finds and groups a host as
    /// the host picker does, and a local profile, a line of its own group
    /// with no destination, by its name and group.
    #[test]
    fn the_split_picker_lists_finds_and_groups_profiles_like_the_host_picker() {
        let host = Host {
            id: "db".into(),
            label: "Postgres".into(),
            group: "Databases".into(),
            destination: "admin@db.example.com:2222".into(),
        };
        let local = Host {
            id: "fish".into(),
            label: "Fish shell".into(),
            group: "Local".into(),
            destination: String::new(),
        };
        let cases: [(Host, &[&str]); 2] = [
            (host, &["postg", "db.example", "datab"]),
            (local, &["fish", "local"]),
        ];
        for (line, queries) in cases {
            for axis in [Axis::Row, Axis::Column] {
                let entry = Entry::SplitWith {
                    host: line.clone(),
                    axis,
                };
                assert_eq!(entry.title(), line.label);
                assert_eq!(entry.group(), Some(line.group.as_str()));
                let haystack = entry.haystack();
                for query in queries {
                    assert_eq!(
                        fuzzy::rank(query, [&*haystack]),
                        [0],
                        "`{query}` in {haystack}"
                    );
                }
            }
        }
    }
}
