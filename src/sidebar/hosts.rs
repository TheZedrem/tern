//! The sidebar's SSH hosts panel (spec 0018, Model): the host picker's
//! list (spec 0006) under its groups, with a filter. A single click only
//! selects; Enter or a double click asks the workspace to open the host
//! in a new tab. Listing reads `config.yaml`'s profiles and
//! `~/.ssh/config` off the UI thread, connects nothing and never runs
//! `ssh -G` (TM-askpass-20).

use crate::commands::fuzzy;
use crate::config::Profile;
use crate::ssh;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, ClickEvent, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    KeyBinding, ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Task,
    Window, actions, div, px,
};
use gpui_component::ActiveTheme;
use gpui_component::Sizable;
use gpui_component::input::{Input, InputEvent, InputState};

const CONTEXT: &str = "SidebarHosts";

actions!(sidebar_hosts, [SelectPrevious, SelectNext, Reload]);

/// Binds the panel's keys; they exist only while the keyboard is in it.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, Some(CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("f5", Reload, Some(CONTEXT)),
    ]);
}

/// The hosts `query` leaves, as indices into `hosts`, under their groups:
/// a group sits where its best match ranks, so without a query the groups
/// keep the list's order. Matched by label, destination and group with
/// the picker's matcher.
pub fn sections<'a>(hosts: &'a [ssh::Host], query: &str) -> Vec<(&'a str, Vec<usize>)> {
    let haystacks: Vec<String> = hosts
        .iter()
        .map(|host| format!("{} {} {}", host.label, host.destination, host.group))
        .collect();
    let mut sections: Vec<(&str, Vec<usize>)> = Vec::new();
    for ix in fuzzy::rank(query, haystacks.iter().map(String::as_str)) {
        let group = hosts[ix].group.as_str();
        match sections.iter_mut().find(|(known, _)| *known == group) {
            Some((_, found)) => found.push(ix),
            None => sections.push((group, vec![ix])),
        }
    }
    sections
}

pub enum HostsEvent {
    /// Open the host with this profile ID in a new tab.
    Open(String),
    /// An ssh config file couldn't be read: said once per read.
    Notice(String),
}

pub struct HostsPanel {
    filter: Entity<InputState>,
    profiles: Vec<Profile>,
    hosts: Vec<ssh::Host>,
    /// The selected host's ID; the first listed one counts if it's gone.
    selected: Option<String>,
    /// Whether the list was read at least once.
    read: bool,
    reading: Option<Task<()>>,
    _filter_events: Subscription,
}

impl EventEmitter<HostsEvent> for HostsPanel {}

impl HostsPanel {
    pub fn new(profiles: Vec<Profile>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter hosts"));
        let filter_events = cx.subscribe_in(
            &filter,
            window,
            |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::PressEnter { .. } => this.open_selected(cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        );
        Self {
            filter,
            profiles,
            hosts: Vec::new(),
            selected: None,
            read: false,
            reading: None,
            _filter_events: filter_events,
        }
    }

    /// Takes a reloaded config's profiles; a list that was read is read
    /// again.
    pub fn set_profiles(&mut self, profiles: Vec<Profile>, cx: &mut Context<Self>) {
        self.profiles = profiles;
        if self.read {
            self.reload(cx);
        }
    }

    /// Reads the hosts again, off the UI thread: when the panel is shown,
    /// on F5 and on a config reload. Nothing watches `~/.ssh/config`.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.read = true;
        let profiles = self.profiles.clone();
        let found = cx.background_spawn(async move {
            let path = ssh::config_path().unwrap_or_default();
            ssh::hosts(&profiles, &path)
        });
        self.reading = Some(cx.spawn(async move |this, cx| {
            let (hosts, notices) = found.await;
            let _ = this.update(cx, |this, cx| {
                this.hosts = hosts;
                for notice in notices {
                    cx.emit(HostsEvent::Notice(notice));
                }
                cx.notify();
            });
        }));
    }

    /// Puts the keyboard in the filter field.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.filter.update(cx, |input, cx| input.focus(window, cx));
    }

    /// The listed hosts' indices, top to bottom.
    fn listed(&self, cx: &App) -> Vec<usize> {
        let query = self.filter.read(cx).value();
        sections(&self.hosts, &query)
            .into_iter()
            .flat_map(|(_, hosts)| hosts)
            .collect()
    }

    /// The selected host if it's listed, else the first listed.
    fn current(&self, cx: &App) -> Option<usize> {
        let listed = self.listed(cx);
        listed
            .iter()
            .copied()
            .find(|&ix| Some(&self.hosts[ix].id) == self.selected.as_ref())
            .or_else(|| listed.first().copied())
    }

    fn select(&mut self, step: isize, cx: &mut Context<Self>) {
        let listed = self.listed(cx);
        let at = self
            .current(cx)
            .and_then(|current| listed.iter().position(|&ix| ix == current));
        let next = match at {
            Some(at) => at
                .saturating_add_signed(step)
                .min(listed.len().saturating_sub(1)),
            None => 0,
        };
        self.selected = listed.get(next).map(|&ix| self.hosts[ix].id.clone());
        cx.notify();
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.current(cx) {
            cx.emit(HostsEvent::Open(self.hosts[ix].id.clone()));
        }
    }

    fn render_host(&self, ix: usize, selected: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, active, hover) = (theme.muted_foreground, theme.list_active, theme.list_hover);
        let host = &self.hosts[ix];
        let id = host.id.clone();
        div()
            .id(("sidebar-host", ix))
            .px(px(8.))
            .py(px(3.))
            .when(selected, |row| row.bg(active))
            .hover(|row| row.bg(hover))
            .child(div().text_sm().child(host.label.clone()))
            .when(!host.destination.is_empty(), |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(host.destination.clone()),
                )
            })
            // A single click only selects: nothing connects by a stray
            // click (spec 0018, flow 4).
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                this.selected = Some(id.clone());
                if event.click_count() >= 2 {
                    cx.emit(HostsEvent::Open(id.clone()));
                }
                cx.notify();
            }))
    }
}

impl Render for HostsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let query = self.filter.read(cx).value();
        let current = self.current(cx);
        let note = |text: &'static str| {
            div()
                .px(px(8.))
                .py(px(8.))
                .text_sm()
                .text_color(muted)
                .child(text)
        };
        let mut list = div()
            .id("sidebar-hosts")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        let sections = sections(&self.hosts, &query);
        if self.hosts.is_empty() {
            if self.read && self.reading.as_ref().is_none_or(Task::is_ready) {
                list = list.child(note(
                    "No hosts yet: add `type: ssh` profiles to config.yaml, \
                     or hosts to ~/.ssh/config",
                ));
            }
        } else if sections.is_empty() {
            list = list.child(note("No host matches"));
        }
        for (group, hosts) in sections {
            list = list.child(
                div()
                    .px(px(8.))
                    .pt(px(8.))
                    .pb(px(2.))
                    .text_xs()
                    .text_color(muted)
                    .child(group.to_owned()),
            );
            for ix in hosts {
                list = list.child(self.render_host(ix, current == Some(ix), cx));
            }
        }
        div()
            .key_context(CONTEXT)
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.select(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.select(1, cx)))
            .on_action(cx.listener(|this, _: &Reload, _, cx| this.reload(cx)))
            .child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .child(Input::new(&self.filter).small()),
            )
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext, VisualTestContext,
        point,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    fn host(label: &str, group: &str, destination: &str) -> ssh::Host {
        ssh::Host {
            id: format!("ssh:{label}"),
            label: label.into(),
            group: group.into(),
            destination: destination.into(),
        }
    }

    /// Spec 0018, AC5: the hosts sit under their groups in the list's
    /// order, and part of a group's name narrows the list to that group.
    #[test]
    fn hosts_are_listed_under_their_groups_and_a_group_name_narrows_to_it() {
        let hosts = three_hosts();
        assert_eq!(
            sections(&hosts, ""),
            vec![("Production", vec![0, 2]), ("Staging", vec![1])]
        );
        assert_eq!(sections(&hosts, "stag"), vec![("Staging", vec![1])]);
        assert_eq!(sections(&hosts, "2222"), vec![("Production", vec![2])]);
        assert!(sections(&hosts, "zzz").is_empty());
    }

    /// Spec 0018, SSH hosts panel: the filter narrows by label, by host
    /// and by group, with the picker's matcher, so letters in order are
    /// enough and case doesn't matter.
    #[test]
    fn the_filter_matches_label_destination_and_group() {
        let hosts = three_hosts();
        assert_eq!(sections(&hosts, "cache"), vec![("Staging", vec![1])]);
        assert_eq!(sections(&hosts, "WEB.EX"), vec![("Production", vec![0])]);
        assert_eq!(sections(&hosts, "prod"), vec![("Production", vec![0, 2])]);
    }

    fn three_hosts() -> Vec<ssh::Host> {
        vec![
            host("web", "Production", "me@web.example"),
            host("cache", "Staging", "cache.example"),
            host("db", "Production", "db.example:2222"),
        ]
    }

    /// The panel in a test window, listing `hosts` as if read, with the
    /// keyboard in its filter field as `sidebar.focus` puts it, and the
    /// IDs of the hosts it asked to open.
    struct Rig<'a> {
        panel: Entity<HostsPanel>,
        opened: Rc<RefCell<Vec<String>>>,
        cx: &'a mut VisualTestContext,
    }

    fn rig(hosts: Vec<ssh::Host>, cx: &mut TestAppContext) -> Rig<'_> {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::sidebar::init(cx);
        });
        let made = Rc::new(RefCell::new(None));
        let (_, cx) = cx.add_window_view({
            let made = made.clone();
            move |window, cx| {
                let panel = cx.new(|cx| {
                    let mut panel = HostsPanel::new(Vec::new(), window, cx);
                    panel.hosts = hosts;
                    panel.read = true;
                    panel
                });
                *made.borrow_mut() = Some(panel.clone());
                gpui_component::Root::new(panel, window, cx)
            }
        });
        let panel = made.take().expect("the window built the panel");
        let opened = Rc::new(RefCell::new(Vec::new()));
        cx.update(|window, cx| {
            let opened = opened.clone();
            cx.subscribe(&panel, move |_, event: &HostsEvent, _| {
                if let HostsEvent::Open(id) = event {
                    opened.borrow_mut().push(id.clone());
                }
            })
            .detach();
            panel.update(cx, |panel, cx| panel.focus(window, cx));
        });
        let mut rig = Rig { panel, opened, cx };
        rig.draw();
        rig
    }

    impl Rig<'_> {
        fn draw(&mut self) {
            self.cx.run_until_parked();
            self.cx.update(|window, cx| window.draw(cx).clear(cx));
        }

        fn keys(&mut self, keys: &str) {
            self.cx.simulate_keystrokes(keys);
            self.draw();
        }

        /// The ID of the host drawn as selected.
        fn selected(&mut self) -> Option<String> {
            self.panel.read_with(self.cx, |panel, cx| {
                panel.current(cx).map(|ix| panel.hosts[ix].id.clone())
            })
        }

        /// The IDs of the listed hosts, top to bottom.
        fn listed(&mut self) -> Vec<String> {
            self.panel.read_with(self.cx, |panel, cx| {
                let listed = panel.listed(cx);
                listed
                    .iter()
                    .map(|&ix| panel.hosts[ix].id.clone())
                    .collect()
            })
        }

        fn filter(&mut self) -> String {
            self.panel.read_with(self.cx, |panel, cx| {
                panel.filter.read(cx).value().to_string()
            })
        }

        fn opened(&self) -> Vec<String> {
            self.opened.borrow().clone()
        }
    }

    /// Spec 0018, flow 4 and AC5: Up and Down select along the list as it
    /// is drawn, with the keyboard in the filter field, stopping at its
    /// ends; Enter asks to open the selected host and nothing else does.
    #[gpui::test]
    fn up_and_down_select_in_the_filter_field_and_enter_opens_the_selection(
        cx: &mut TestAppContext,
    ) {
        let mut rig = rig(three_hosts(), cx);
        assert_eq!(rig.listed(), ["ssh:web", "ssh:db", "ssh:cache"]);
        assert_eq!(rig.selected().as_deref(), Some("ssh:web"));

        rig.keys("down");
        assert_eq!(rig.selected().as_deref(), Some("ssh:db"));
        rig.keys("down down down");
        assert_eq!(rig.selected().as_deref(), Some("ssh:cache"));
        rig.keys("up");
        assert_eq!(rig.selected().as_deref(), Some("ssh:db"));
        rig.keys("up up up");
        assert_eq!(rig.selected().as_deref(), Some("ssh:web"));
        assert_eq!(rig.filter(), "", "the arrows typed into the filter");
        assert_eq!(rig.opened(), [] as [&str; 0], "selecting opened a host");

        rig.keys("down enter");
        assert_eq!(rig.opened(), ["ssh:db"]);
    }

    /// Spec 0018, AC5 and AC6: what is typed with the keyboard in the
    /// panel goes into the filter field and narrows the list, here to one
    /// group by part of its name; Enter then opens the first host left.
    /// A filter nothing matches leaves nothing for Enter to open.
    #[gpui::test]
    fn typing_narrows_the_list_and_enter_opens_the_first_host_left(cx: &mut TestAppContext) {
        let mut rig = rig(three_hosts(), cx);
        rig.keys("down");
        assert_eq!(rig.selected().as_deref(), Some("ssh:db"));

        rig.keys("s t a g");
        assert_eq!(rig.filter(), "stag");
        assert_eq!(rig.listed(), ["ssh:cache"]);
        assert_eq!(rig.selected().as_deref(), Some("ssh:cache"));
        rig.keys("enter");
        assert_eq!(rig.opened(), ["ssh:cache"]);

        rig.keys("z z z");
        assert_eq!(rig.listed(), [] as [&str; 0]);
        rig.keys("enter");
        assert_eq!(rig.opened(), ["ssh:cache"], "Enter opened an unlisted host");
    }

    /// Spec 0018, flow 4: with no hosts there is nothing to select or
    /// open.
    #[gpui::test]
    fn enter_and_the_arrows_do_nothing_without_hosts(cx: &mut TestAppContext) {
        let mut rig = rig(Vec::new(), cx);
        rig.keys("down up enter");
        assert_eq!(rig.selected(), None);
        assert_eq!(rig.opened(), [] as [&str; 0]);
    }

    /// Spec 0018, flow 4, AC5 and §Security & privacy: a single click on
    /// a host only selects it, so nothing connects by a stray click; a
    /// double click asks to open it. The rows carry no test selector, so
    /// the list is clicked from the top down until the last host is hit:
    /// every click on the way is a single click on something.
    #[gpui::test]
    fn a_single_click_only_selects_and_a_double_click_opens(cx: &mut TestAppContext) {
        let mut rig = rig(three_hosts(), cx);
        let mut row = None;
        for step in 0..100 {
            let at = point(px(60.0), px(4.0 + 4.0 * step as f32));
            rig.cx.simulate_click(at, Modifiers::none());
            rig.draw();
            if rig.selected().as_deref() == Some("ssh:cache") {
                row = Some(at);
                break;
            }
        }
        let row = row.expect("no click selected the last host");
        assert_eq!(
            rig.opened(),
            [] as [&str; 0],
            "a single click opened a host"
        );

        rig.cx.simulate_event(MouseDownEvent {
            position: row,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        rig.cx.simulate_event(MouseUpEvent {
            position: row,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 2,
        });
        rig.draw();
        assert_eq!(rig.opened(), ["ssh:cache"]);
        assert_eq!(rig.selected().as_deref(), Some("ssh:cache"));
    }
}
