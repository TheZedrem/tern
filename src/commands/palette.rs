//! The command palette (spec 0004): gpui-component's command list over the
//! app commands and profiles, over the themes as the theme picker, or over
//! the hosts, grouped, as the host picker (spec 0006), which connects to a
//! host, forgets its saved password, or with the local profiles splits the
//! focused pane with one (spec 0007). Ranked by [`fuzzy`] instead of the
//! list's own substring filter.

use super::{AppCommand, fuzzy};
use crate::files::pane::Pick;
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
    /// A line of the focused files pane's target switcher (spec 0013):
    /// `host` names and groups it; `current` is the target shown.
    Files {
        host: Host,
        pick: Pick,
        current: bool,
    },
    /// The forward a `pf` query asks for, which picking adds to the
    /// focused SSH pane's host; `None` with the reason in the title when it
    /// can't be added (spec 0017, flow 10).
    AddForward {
        title: String,
        forward: Option<crate::forwards::Forward>,
    },
    /// A running port forward, as `<label>: <summary>`, which picking stops
    /// (spec 0017, flow 7).
    Forward {
        id: crate::forwards::Id,
        title: String,
    },
    /// A value for a setting the settings screen picks from a list (spec
    /// 0009); `current` is the one the file has.
    Setting {
        title: String,
        current: bool,
        change: Change,
    },
    /// A plugin's manifest command, `<plugin name>: <title>` (spec 0016,
    /// Commands).
    Plugin {
        title: String,
        command: super::PluginCommand,
    },
}

impl Entry {
    fn title(&self) -> &str {
        match self {
            Entry::Command { title, .. } => title,
            Entry::Profile { title, .. }
            | Entry::Forward { title, .. }
            | Entry::AddForward { title, .. } => title,
            Entry::Setting { title, .. } | Entry::Plugin { title, .. } => title,
            Entry::Theme { name, .. } => name,
            Entry::Host(host)
            | Entry::ForgetHost(host)
            | Entry::SplitWith { host, .. }
            | Entry::Files { host, .. } => &host.label,
        }
    }

    fn host(&self) -> Option<&Host> {
        match self {
            Entry::Host(host)
            | Entry::ForgetHost(host)
            | Entry::SplitWith { host, .. }
            | Entry::Files { host, .. } => Some(host),
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
            Entry::Plugin { title, command } => CommandItem::new()
                .label(title.clone())
                .action(Box::new(command.clone())),
            Entry::Theme { name, current } => {
                CommandItem::new().label(name.clone()).checked(*current)
            }
            Entry::Setting { title, current, .. } => {
                CommandItem::new().label(title.clone()).checked(*current)
            }
            Entry::Host(host) | Entry::ForgetHost(host) | Entry::SplitWith { host, .. } => {
                CommandItem::new().label(host.label.clone())
            }
            Entry::Files { host, current, .. } => CommandItem::new()
                .label(host.label.clone())
                .checked(*current),
            Entry::Forward { title, .. } => CommandItem::new().label(title.clone()),
            Entry::AddForward { title, forward } => CommandItem::new()
                .label(title.clone())
                .disabled(forward.is_none()),
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
    /// Act on the focused files pane's targets (spec 0013).
    Files(Pick),
    /// Stop the port forward with this ID (spec 0017, flow 7).
    StopForward(crate::forwards::Id),
    /// Add this port forward to the focused SSH pane's host (flow 10).
    AddForward(crate::forwards::Forward),
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
    /// Turns a query into an entry of its own, shown alone (the `pf`
    /// shortcut); `entries` holds it last while it's shown.
    quick: Option<Quick>,
    /// How many of `entries` aren't the quick entry.
    fixed: usize,
}

/// What [`Palette::with_quick`] takes: an entry for a query, if it's one.
pub type Quick = Box<dyn Fn(&str, &App) -> Option<Entry>>;

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
            fixed: entries.len(),
            quick: None,
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

    /// Lets a query become an entry of its own: when `quick` makes one,
    /// it's the only entry shown.
    pub fn with_quick(mut self, quick: Quick) -> Self {
        self.quick = Some(quick);
        self
    }

    fn rank(&mut self, query: &str, cx: &App) {
        self.entries.truncate(self.fixed);
        if let Some(entry) = self.quick.as_ref().and_then(|quick| quick(query, cx)) {
            self.entries.push(entry);
            self.shown = vec![self.fixed];
            return;
        }
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
            Some(Entry::Files { pick, .. }) => cx.emit(PaletteEvent::Files(pick.clone())),
            Some(Entry::Forward { id, .. }) => cx.emit(PaletteEvent::StopForward(*id)),
            Some(Entry::AddForward { forward, .. }) => {
                let Some(forward) = forward else {
                    return;
                };
                cx.emit(PaletteEvent::AddForward(forward.clone()));
            }
            // A command has run through its action already.
            Some(Entry::Command { .. } | Entry::Plugin { .. }) | None => {}
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
            Some(Entry::Plugin { command, .. }) => command.id().into(),
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
                        palette.rank(query, cx);
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
    use crate::forwards::{self, Forward};
    use gpui::{TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::rc::Rc;

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

    /// What a palette has emitted: forwards to add, and how often it closed.
    #[derive(Default)]
    struct Seen {
        added: Vec<Forward>,
        closed: usize,
    }

    /// A palette over two profiles with the `pf` shortcut as its quick
    /// entry, and what it emits.
    fn pf_palette(
        cx: &mut TestAppContext,
    ) -> (Entity<Palette>, &mut VisualTestContext, Rc<RefCell<Seen>>) {
        cx.update(gpui_component::init);
        let entries = || {
            ["Split right", "Port forwards"]
                .map(|title| Entry::Profile {
                    id: title.into(),
                    title: title.into(),
                })
                .into()
        };
        let quick: Quick = Box::new(|query, _| {
            forwards::shortcut(query).map(|parsed| match parsed {
                Ok(forward) => Entry::AddForward {
                    title: format!("Add port forward {}", forward.summary()),
                    forward: Some(forward),
                },
                Err(reason) => Entry::AddForward {
                    title: reason.into(),
                    forward: None,
                },
            })
        });
        let (palette, cx) = cx.add_window_view(|window, cx| {
            Palette::new(entries(), "Search", None, window, cx).with_quick(quick)
        });
        let seen = Rc::new(RefCell::new(Seen::default()));
        cx.update(|_, cx| {
            let seen = seen.clone();
            cx.subscribe(&palette, move |_, event, _| match event {
                PaletteEvent::AddForward(forward) => seen.borrow_mut().added.push(forward.clone()),
                PaletteEvent::Closed { .. } => seen.borrow_mut().closed += 1,
                _ => {}
            })
            .detach();
        });
        (palette, cx, seen)
    }

    /// The titles of the entries a query shows, in order.
    fn shown(palette: &Entity<Palette>, query: &str, cx: &mut VisualTestContext) -> Vec<String> {
        palette.update(cx, |palette, cx| {
            palette.rank(query, cx);
            palette
                .shown
                .iter()
                .map(|&ix| palette.entries[ix].title().to_owned())
                .collect()
        })
    }

    /// Spec 0017, flow 10: a `pf` query is the palette's only entry, and
    /// going back to an ordinary query ranks the commands again without a
    /// leftover forward entry.
    #[gpui::test]
    fn a_pf_query_is_the_only_entry_until_the_query_changes(cx: &mut TestAppContext) {
        let (palette, cx, _) = pf_palette(cx);
        assert_eq!(
            shown(&palette, "pf 13305:13305", cx),
            ["Add port forward 13305 → localhost:13305"]
        );
        assert_eq!(
            shown(&palette, "pf 1:2", cx),
            ["Add port forward 1 → localhost:2"]
        );
        assert_eq!(shown(&palette, "port", cx), ["Port forwards"]);
        assert_eq!(shown(&palette, "", cx), ["Split right", "Port forwards"]);
    }

    /// Spec 0017, flow 10 and AC11: Enter on the shortcut's entry asks to
    /// add that forward and closes the palette.
    #[gpui::test]
    fn confirming_a_pf_entry_adds_its_forward(cx: &mut TestAppContext) {
        let (palette, cx, seen) = pf_palette(cx);
        palette.update(cx, |palette, cx| {
            palette.rank("pf 13305:13305", cx);
            palette.confirm(IndexPath::default(), cx);
        });
        let seen = seen.borrow();
        assert_eq!(seen.added.len(), 1, "one forward asked for");
        assert_eq!(seen.added[0].arg(), "127.0.0.1:13305:localhost:13305");
        assert!(!seen.added[0].network);
        assert_eq!(seen.closed, 1, "the palette closes");
    }

    /// Spec 0017, flow 10 and AC11: a refused forward stays dimmed with its
    /// reason, and Enter on it adds nothing and leaves the palette open.
    #[gpui::test]
    fn confirming_a_refused_pf_entry_adds_nothing(cx: &mut TestAppContext) {
        let (palette, cx, seen) = pf_palette(cx);
        for query in ["pf 0.0.0.0:80:web:80", "pf 70000:1"] {
            palette.update(cx, |palette, cx| {
                palette.rank(query, cx);
                assert!(
                    matches!(
                        palette.entries[palette.shown[0]],
                        Entry::AddForward { forward: None, .. }
                    ),
                    "`{query}` isn't dimmed"
                );
                palette.confirm(IndexPath::default(), cx);
            });
        }
        let seen = seen.borrow();
        assert!(seen.added.is_empty(), "added {:?}", seen.added);
        assert_eq!(seen.closed, 0, "the palette closed");
    }
}
