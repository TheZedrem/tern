//! The workspace's side of port forwards (spec 0017): the app's registry,
//! made with the first forward; the sidebar's Port forwards view; and the
//! command `forwards.stop` (`pane.portForwards` is `super::sidebar`'s).

use super::files::SidebarView;
use super::{Workspace, show_notice};
use crate::commands::StopForward;
use crate::commands::palette::{Entry, Palette, Quick};
use crate::forwards::registry::Registry;
use crate::forwards::view::{ForwardsEvent, ForwardsView, start};
use crate::forwards::{self, CLASH, Forward};
use crate::terminal::TerminalPane;
use gpui::{App, AppContext, Context, Entity, Window};

impl Workspace {
    /// The app's port forwards, made now if this is the first use: nothing
    /// is restored, so nothing needs it at startup (spec 0017, Startup
    /// impact).
    fn forward_registry(&mut self, cx: &mut Context<Self>) -> Entity<Registry> {
        self.forwards
            .get_or_insert_with(|| cx.new(Registry::new))
            .clone()
    }

    /// The Port forwards view the sidebar draws for `pane`, on its host.
    pub(super) fn sidebar_forwards(
        &mut self,
        pane: &Entity<TerminalPane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<SidebarView<ForwardsView>> {
        let host = pane.read(cx).forward_host()?;
        let registry = self.forward_registry(cx);
        let pane = pane.downgrade();
        let view = cx.new(|cx| ForwardsView::new(pane, host, registry, window, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            move |this, _, event, window, cx| match event {
                ForwardsEvent::Leave => this.focus_active(window, cx),
            },
        );
        Some(SidebarView {
            view,
            _events: events,
        })
    }

    /// Lists every running forward as `<label>: <summary>`; picking one
    /// stops it, whether or not its tabs are open (spec 0017, flow 7).
    pub(super) fn stop_forward(
        &mut self,
        _: &StopForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries: Vec<Entry> = self.forwards.as_ref().map_or_else(Vec::new, |forwards| {
            forwards
                .read(cx)
                .all()
                .iter()
                .map(|entry| Entry::Forward {
                    id: entry.id,
                    title: format!("{}: {}", entry.host.label, entry.forward.summary()),
                })
                .collect()
        });
        if entries.is_empty() {
            show_notice("No port forwards are running".into(), window, cx);
            return;
        }
        self.show_palette(None, window, cx, |window, cx| {
            Palette::new(entries, "Stop a port forward", None, window, cx)
        });
    }

    /// The palette's `pf` shortcut for the focused pane (spec 0017, flow
    /// 10): a query like `pf 13305:13305` becomes one entry that adds that
    /// forward, or says why it can't.
    pub(super) fn forward_shortcut(&self, cx: &App) -> Quick {
        let pane = self.panes.get(&self.tabs.active().focused);
        let host = pane.and_then(|pane| pane.read(cx).forward_host());
        let registry = self.forwards.clone();
        Box::new(move |query, cx| {
            let parsed = forwards::shortcut(query)?.and_then(|forward| {
                let host = host.as_ref().ok_or("Port forwards need an SSH pane")?;
                let clashes = registry
                    .as_ref()
                    .is_some_and(|registry| registry.read(cx).clashes(&host.args, &forward));
                if clashes { Err(CLASH) } else { Ok(forward) }
            });
            Some(match parsed {
                Ok(forward) => Entry::AddForward {
                    title: format!("Add port forward {}", forward.summary()),
                    forward: Some(forward),
                },
                Err(reason) => Entry::AddForward {
                    title: format!("Can't add this port forward: {reason}"),
                    forward: None,
                },
            })
        })
    }

    /// Adds `forward` to the focused SSH pane's host, from the palette's
    /// `pf` shortcut, and says so (spec 0017, flow 10).
    pub(super) fn add_forward(
        &mut self,
        forward: Forward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.panes.get(&self.tabs.active().focused).cloned() else {
            return;
        };
        let summary = forward.summary();
        let registry = self.forward_registry(cx);
        match start(&registry, &pane, forward, window, cx) {
            Some(_) => show_notice(format!("Port forward {summary} is starting"), window, cx),
            None => show_notice("Port forwards need an SSH pane".into(), window, cx),
        }
    }
}
