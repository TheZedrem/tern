//! The sidebar's SSH hosts panel (spec 0018, Model): the host picker's
//! list (spec 0006) under its groups, with a filter. A single click only
//! selects; Enter or a double click asks the workspace to open the host
//! in a new tab. A host's menu (a right click, the Menu key or Shift+F10)
//! asks it to edit or delete a profile; a `~/.ssh/config` host is never
//! written. Listing reads `config.yaml`'s profiles and `~/.ssh/config`
//! off the UI thread, connects nothing and never runs `ssh -G`
//! (TM-askpass-20).

use crate::commands::fuzzy;
use crate::config::Profile;
use crate::ssh;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, ClickEvent, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, IntoElement, KeyBinding, MouseButton, ParentElement, Render,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, actions, anchored, deferred,
    div, px,
};
use gpui_component::ActiveTheme;
use gpui_component::Sizable;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt, PopupMenu};

const CONTEXT: &str = "SidebarHosts";
/// The list's own context, where Enter and Delete mean the selected host
/// rather than the filter field's text.
const LIST_CONTEXT: &str = "SidebarHostsList";
/// What a `~/.ssh/config` host's menu says instead of editing it.
const READ_ONLY: &str = "Defined in ~/.ssh/config";

actions!(
    sidebar_hosts,
    [
        SelectPrevious,
        SelectNext,
        Reload,
        ShowMenu,
        ConnectSelected,
        EditSelected,
        DeleteSelected
    ]
);

/// Binds the panel's keys; they exist only while the keyboard is in it.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, Some(CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("f5", Reload, Some(CONTEXT)),
        KeyBinding::new("shift-f10", ShowMenu, Some(CONTEXT)),
        KeyBinding::new("menu", ShowMenu, Some(CONTEXT)),
        KeyBinding::new("enter", ConnectSelected, Some(LIST_CONTEXT)),
        KeyBinding::new("delete", DeleteSelected, Some(LIST_CONTEXT)),
        KeyBinding::new("backspace", DeleteSelected, Some(LIST_CONTEXT)),
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
    /// Edit the profile with this ID.
    Edit(String),
    /// Ask whether to delete the profile with this ID, named `label`.
    Delete { id: String, label: String },
    /// An ssh config file couldn't be read: said once per read.
    Notice(String),
}

/// The menu the keyboard opened, drawn at the selected host's row.
struct KeyMenu {
    id: String,
    menu: Entity<PopupMenu>,
    _dismissed: Subscription,
}

pub struct HostsPanel {
    /// The list's, which has the keyboard after a click on a host.
    focus: FocusHandle,
    filter: Entity<InputState>,
    profiles: Vec<Profile>,
    hosts: Vec<ssh::Host>,
    /// The selected host's ID; the first listed one counts if it's gone.
    selected: Option<String>,
    /// Whether the list was read at least once.
    read: bool,
    reading: Option<Task<()>>,
    key_menu: Option<KeyMenu>,
    _filter_events: Subscription,
}

impl Focusable for HostsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
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
            // A tab stop, so Tab from the filter field reaches the list.
            focus: cx.focus_handle().tab_stop(true),
            filter,
            profiles,
            hosts: Vec::new(),
            selected: None,
            read: false,
            reading: None,
            key_menu: None,
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

    /// Whether host `id` is a profile of `config.yaml`, which the panel
    /// may edit; else it comes from `~/.ssh/config`, which Tern never
    /// writes (spec 0006, Security).
    fn editable(&self, id: &str) -> bool {
        self.profiles.iter().any(|profile| profile.id == id)
    }

    fn edit_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.current(cx)
            && self.editable(&self.hosts[ix].id)
        {
            cx.emit(HostsEvent::Edit(self.hosts[ix].id.clone()));
        }
    }

    /// Asks the workspace to ask: nothing is deleted here (spec 0018,
    /// A host's menu).
    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.current(cx)
            && self.editable(&self.hosts[ix].id)
        {
            let host = &self.hosts[ix];
            cx.emit(HostsEvent::Delete {
                id: host.id.clone(),
                label: host.label.clone(),
            });
        }
    }

    /// A host's menu: each entry is the action its key runs, sent to the
    /// list, so it acts on the selected host.
    fn host_menu(menu: PopupMenu, focus: FocusHandle, editable: bool) -> PopupMenu {
        let menu = menu
            .action_context(focus)
            .menu("Connect", Box::new(ConnectSelected))
            .menu_with_disabled("Edit…", Box::new(EditSelected), !editable)
            .menu_with_disabled("Delete…", Box::new(DeleteSelected), !editable);
        if editable {
            menu
        } else {
            menu.separator().label(READ_ONLY)
        }
    }

    /// The Menu key or Shift+F10: the selected host's menu at its row,
    /// with the keyboard in it.
    fn show_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.current(cx) else {
            return;
        };
        let id = self.hosts[ix].id.clone();
        self.selected = Some(id.clone());
        let (focus, editable) = (self.focus.clone(), self.editable(&id));
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            Self::host_menu(menu, focus, editable)
        });
        let dismissed =
            cx.subscribe_in(&menu, window, |this, menu, _: &DismissEvent, window, cx| {
                if menu.focus_handle(cx).is_focused(window) || window.focused(cx).is_none() {
                    window.focus(&this.focus, cx);
                }
                this.key_menu = None;
                cx.notify();
            });
        menu.focus_handle(cx).focus(window, cx);
        self.key_menu = Some(KeyMenu {
            id,
            menu,
            _dismissed: dismissed,
        });
        cx.notify();
    }

    fn render_host(&self, ix: usize, selected: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, active, hover) = (theme.muted_foreground, theme.list_active, theme.list_hover);
        let host = &self.hosts[ix];
        let id = host.id.clone();
        let (focus, editable) = (self.focus.clone(), self.editable(&id));
        let key_menu = self
            .key_menu
            .as_ref()
            .filter(|open| open.id == id)
            .map(|open| {
                deferred(
                    anchored()
                        .snap_to_window_with_margin(px(8.))
                        .child(open.menu.clone()),
                )
                .with_priority(1)
            });
        let pressed = id.clone();
        div()
            .id(("sidebar-host", ix))
            .px(px(8.))
            .py(px(3.))
            .when(selected, |row| row.bg(active))
            .hover(|row| row.bg(hover))
            .children(key_menu)
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
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.selected = Some(id.clone());
                if event.click_count() >= 2 {
                    cx.emit(HostsEvent::Open(id.clone()));
                }
                cx.notify();
            }))
            // A right click selects the host its menu acts on; the menu's
            // entries run only when picked.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| {
                    window.focus(&this.focus, cx);
                    this.selected = Some(pressed.clone());
                    cx.notify();
                }),
            )
            .context_menu(move |menu, _, _| Self::host_menu(menu, focus.clone(), editable))
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
            .key_context(LIST_CONTEXT)
            .track_focus(&self.focus)
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
            .on_action(cx.listener(|this, _: &ShowMenu, window, cx| this.show_menu(window, cx)))
            .on_action(cx.listener(|this, _: &ConnectSelected, _, cx| this.open_selected(cx)))
            .on_action(cx.listener(|this, _: &EditSelected, _, cx| this.edit_selected(cx)))
            .on_action(cx.listener(|this, _: &DeleteSelected, _, cx| this.delete_selected(cx)))
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
        Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point, TestAppContext,
        VisualTestContext, point,
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
        /// Every event but a notice, as `open <id>`, `edit <id>` or
        /// `delete <id> <label>`.
        events: Rc<RefCell<Vec<String>>>,
        cx: &'a mut VisualTestContext,
    }

    fn rig(hosts: Vec<ssh::Host>, cx: &mut TestAppContext) -> Rig<'_> {
        rig_with(Vec::new(), hosts, cx)
    }

    /// [`rig`] with `profiles` as `config.yaml`'s: the hosts whose IDs
    /// are among them are profiles, the others `~/.ssh/config` hosts.
    fn rig_with(profiles: Vec<Profile>, hosts: Vec<ssh::Host>, cx: &mut TestAppContext) -> Rig<'_> {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::sidebar::init(cx);
        });
        let made = Rc::new(RefCell::new(None));
        let (_, cx) = cx.add_window_view({
            let made = made.clone();
            move |window, cx| {
                let panel = cx.new(|cx| {
                    let mut panel = HostsPanel::new(profiles, window, cx);
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
        let events = Rc::new(RefCell::new(Vec::new()));
        cx.update(|window, cx| {
            let (opened, events) = (opened.clone(), events.clone());
            cx.subscribe(&panel, move |_, event: &HostsEvent, _| {
                let said = match event {
                    HostsEvent::Open(id) => {
                        opened.borrow_mut().push(id.clone());
                        format!("open {id}")
                    }
                    HostsEvent::Edit(id) => format!("edit {id}"),
                    HostsEvent::Delete { id, label } => format!("delete {id} {label}"),
                    HostsEvent::Notice(_) => return,
                };
                events.borrow_mut().push(said);
            })
            .detach();
            panel.update(cx, |panel, cx| panel.focus(window, cx));
        });
        let mut rig = Rig {
            panel,
            opened,
            events,
            cx,
        };
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

        fn events(&self) -> Vec<String> {
            self.events.borrow().clone()
        }

        /// The host a click or right click picked, as opposed to the
        /// first listed one the panel draws as selected by default.
        fn picked(&mut self) -> Option<String> {
            self.panel
                .read_with(self.cx, |panel, _| panel.selected.clone())
        }

        fn list_has_the_keyboard(&mut self) -> bool {
            let panel = self.panel.clone();
            self.cx
                .update(|window, cx| panel.read(cx).focus.is_focused(window))
        }

        /// Where each listed host's row is, found by clicking down the
        /// panel from the top: the rows carry no test selector. Every
        /// click is a single click, which only selects. Ends with the
        /// keyboard in the list and the last host selected.
        fn rows(&mut self) -> Vec<(String, Point<Pixels>)> {
            let listed = self.listed();
            let mut rows: Vec<(String, Point<Pixels>)> = Vec::new();
            for step in 0..150 {
                let at = point(px(60.0), px(4.0 + 4.0 * step as f32));
                self.cx.simulate_click(at, Modifiers::none());
                self.draw();
                if let Some(id) = self.picked()
                    && !rows.iter().any(|(found, _)| *found == id)
                {
                    rows.push((id, at));
                }
                if rows.len() == listed.len() {
                    return rows;
                }
            }
            panic!("only found the rows of {rows:?} among {listed:?}");
        }

        fn right_click(&mut self, at: Point<Pixels>) {
            self.cx.simulate_event(MouseDownEvent {
                position: at,
                modifiers: Modifiers::none(),
                button: MouseButton::Right,
                click_count: 1,
                first_mouse: false,
            });
            self.cx.simulate_event(MouseUpEvent {
                position: at,
                modifiers: Modifiers::none(),
                button: MouseButton::Right,
                click_count: 1,
            });
            self.draw();
        }
    }

    /// Two `config.yaml` profiles, `web` and `db` under Production, and
    /// `cache`, a `~/.ssh/config` host under Staging. Listed as web, db,
    /// cache.
    fn profiles_and_a_config_host() -> (Vec<Profile>, Vec<ssh::Host>) {
        let profile = |id: &str| Profile {
            id: id.into(),
            name: None,
            theme: None,
            kind: crate::config::ProfileKind::Ssh(crate::config::tests::ssh_to(id)),
        };
        let listed = |id: &str, label: &str, group: &str| ssh::Host {
            id: id.into(),
            label: label.into(),
            group: group.into(),
            destination: format!("{label}.example"),
        };
        (
            vec![profile("web"), profile("db")],
            vec![
                listed("web", "Web", "Production"),
                listed("db", "Db", "Production"),
                listed("ssh:cache", "cache", "Staging"),
            ],
        )
    }

    fn profile_rig(cx: &mut TestAppContext) -> Rig<'_> {
        let (profiles, hosts) = profiles_and_a_config_host();
        let mut rig = rig_with(profiles, hosts, cx);
        assert_eq!(rig.listed(), ["web", "db", "ssh:cache"]);
        rig
    }

    const NOTHING: [&str; 0] = [];

    /// Spec 0018, A host's menu: with the keyboard in the list after a
    /// click on a profile host, Delete or Backspace asks to delete it,
    /// naming its label; on a `~/.ssh/config` host they do nothing.
    /// Nothing is typed into the filter field.
    #[gpui::test]
    fn delete_or_backspace_in_the_list_asks_to_delete_a_profile_host_only(cx: &mut TestAppContext) {
        let mut rig = profile_rig(cx);
        let rows = rig.rows();
        let (_, web) = rows[0].clone();
        rig.cx.simulate_click(web, Modifiers::none());
        rig.draw();
        assert_eq!(rig.picked().as_deref(), Some("web"));
        assert!(rig.list_has_the_keyboard(), "a click on a host");

        rig.keys("delete");
        assert_eq!(rig.events(), ["delete web Web"]);
        rig.keys("backspace");
        assert_eq!(rig.events(), ["delete web Web", "delete web Web"]);

        let (_, cache) = rows[2].clone();
        rig.cx.simulate_click(cache, Modifiers::none());
        rig.draw();
        assert_eq!(rig.picked().as_deref(), Some("ssh:cache"));
        rig.keys("delete backspace");
        assert_eq!(
            rig.events(),
            ["delete web Web", "delete web Web"],
            "a ~/.ssh/config host asked to be deleted"
        );
        assert_eq!(rig.filter(), "");
    }

    /// Spec 0018, A host's menu: in the filter field, Delete and
    /// Backspace edit its text and ask nothing.
    #[gpui::test]
    fn delete_and_backspace_in_the_filter_field_only_edit_its_text(cx: &mut TestAppContext) {
        let mut rig = profile_rig(cx);
        rig.keys("w e b");
        assert_eq!(rig.filter(), "web");
        rig.keys("backspace");
        assert_eq!(rig.filter(), "we");
        rig.keys("left delete");
        assert_eq!(rig.filter(), "w");
        assert_eq!(rig.events(), NOTHING);
    }

    /// Spec 0018, A host's menu: Tab from the filter field puts the
    /// keyboard in the list, where Delete asks about the selected host.
    /// The sidebar around the panel binds Tab to the window's next focus
    /// stop, which this rig, the panel alone, stands in for.
    #[gpui::test]
    fn tab_from_the_filter_field_lets_delete_ask_about_the_selected_host(cx: &mut TestAppContext) {
        let mut rig = profile_rig(cx);
        rig.keys("down");
        rig.cx.update(|window, cx| window.focus_next(cx));
        rig.draw();
        assert!(rig.list_has_the_keyboard(), "Tab didn't reach the list");
        rig.keys("delete");
        assert_eq!(rig.events(), ["delete db Db"]);
    }

    /// Spec 0018, flow 4 and A host's menu: Enter with the keyboard in the
    /// list opens the selected host.
    #[gpui::test]
    fn enter_in_the_list_opens_the_selected_host(cx: &mut TestAppContext) {
        let mut rig = profile_rig(cx);
        let rows = rig.rows();
        rig.cx.simulate_click(rows[1].1, Modifiers::none());
        rig.draw();
        assert!(rig.list_has_the_keyboard());
        assert_eq!(rig.events(), NOTHING, "a single click did something");
        rig.keys("enter");
        assert_eq!(rig.events(), ["open db"]);
    }

    /// Spec 0018, A host's menu: Shift+F10 with the keyboard in the filter
    /// field opens the selected host's menu, Connect, Edit… and Delete…,
    /// with the keyboard in it; each entry chosen by keyboard does what
    /// it says to that host, and Esc closes it doing nothing.
    #[gpui::test]
    fn shift_f10_in_the_filter_field_opens_the_menu_of_the_selected_host(cx: &mut TestAppContext) {
        let mut rig = profile_rig(cx);
        rig.keys("down");
        assert_eq!(rig.selected().as_deref(), Some("db"));

        rig.keys("shift-f10 escape");
        assert_eq!(rig.events(), NOTHING, "Esc ran an entry");
        rig.keys("shift-f10 down enter");
        assert_eq!(rig.events(), ["open db"]);
        rig.keys("shift-f10 down down enter");
        assert_eq!(rig.events(), ["open db", "edit db"]);
        rig.keys("shift-f10 down down down enter");
        assert_eq!(rig.events(), ["open db", "edit db", "delete db Db"]);
        assert_eq!(rig.selected().as_deref(), Some("db"));
        assert_eq!(rig.filter(), "", "the menu's keys reached the filter");
    }

    /// Spec 0018, A host's menu: Shift+F10 and the Menu key open the menu
    /// with the keyboard in the list too, after a click on a host.
    #[gpui::test]
    fn shift_f10_and_the_menu_key_open_the_menu_from_the_list(cx: &mut TestAppContext) {
        let mut rig = profile_rig(cx);
        let rows = rig.rows();
        rig.cx.simulate_click(rows[0].1, Modifiers::none());
        rig.draw();
        assert!(rig.list_has_the_keyboard());

        rig.keys("shift-f10 down down enter");
        assert_eq!(rig.events(), ["edit web"]);
        rig.keys("menu down down down enter");
        assert_eq!(rig.events(), ["edit web", "delete web Web"]);
    }

    /// Spec 0018, A host's menu, and AC6: a `~/.ssh/config` host's Edit…
    /// and Delete… are disabled, so the keyboard can't reach them in its
    /// menu: every step down lands on Connect.
    #[gpui::test]
    fn a_config_hosts_menu_offers_only_connect(cx: &mut TestAppContext) {
        let mut rig = profile_rig(cx);
        rig.keys("down down");
        assert_eq!(rig.selected().as_deref(), Some("ssh:cache"));
        for downs in ["down", "down down", "down down down", "down down down down"] {
            rig.keys(&format!("shift-f10 {downs} enter"));
        }
        assert_eq!(rig.events(), ["open ssh:cache"; 4]);
    }

    /// Spec 0018, A host's menu: a right click on a host selects it and
    /// opens its menu; no entry runs on the right click alone, and the
    /// entries then act on that host.
    #[gpui::test]
    fn a_right_click_selects_the_host_and_opens_its_menu_without_running_an_entry(
        cx: &mut TestAppContext,
    ) {
        let mut rig = profile_rig(cx);
        let rows = rig.rows();
        assert_eq!(rig.picked().as_deref(), Some("ssh:cache"));
        let (web, db) = (rows[0].1, rows[1].1);

        rig.right_click(web);
        assert_eq!(rig.picked().as_deref(), Some("web"));
        assert_eq!(rig.events(), NOTHING, "a right click ran an entry");
        // Were the menu not open, Down would select db and Enter open it.
        rig.keys("down enter");
        assert_eq!(rig.events(), ["open web"]);

        rig.right_click(db);
        assert_eq!(rig.picked().as_deref(), Some("db"));
        rig.keys("escape");
        assert_eq!(rig.events(), ["open web"], "Esc ran an entry");
        rig.right_click(db);
        rig.keys("down down down enter");
        assert_eq!(rig.events(), ["open web", "delete db Db"]);
        rig.right_click(web);
        rig.keys("down down enter");
        assert_eq!(rig.events(), ["open web", "delete db Db", "edit web"]);
    }

    /// Spec 0018, A host's menu: a right click on a `~/.ssh/config` host
    /// offers Connect only; Edit… and Delete… can't be chosen.
    #[gpui::test]
    fn a_right_click_on_a_config_host_offers_only_connect(cx: &mut TestAppContext) {
        let mut rig = profile_rig(cx);
        let rows = rig.rows();
        let cache = rows[2].1;
        for downs in ["down", "down down", "down down down"] {
            rig.right_click(cache);
            assert_eq!(rig.picked().as_deref(), Some("ssh:cache"));
            rig.keys(&format!("{downs} enter"));
        }
        assert_eq!(rig.events(), ["open ssh:cache"; 3]);
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
