//! The workspace's side of port forwards (spec 0017): the app's registry,
//! made with the first forward; the sidebar's Port forwards view; and the
//! commands `pane.portForwards` and `forwards.stop`.

use super::files::{Shown, SidebarView};
use super::tree::LeafId;
use super::{Workspace, show_notice};
use crate::commands::palette::{Entry, Palette};
use crate::commands::{ShowPortForwards, StopForward};
use crate::forwards::registry::Registry;
use crate::forwards::view::{ForwardsEvent, ForwardsView};
use crate::terminal::TerminalPane;
use gpui::{AppContext, Context, Entity, Window};

impl Workspace {
    /// The app's port forwards, made now if this is the first use: nothing
    /// is restored, so nothing needs it at startup (spec 0017, Startup
    /// impact).
    fn forward_registry(&mut self, cx: &mut Context<Self>) -> Entity<Registry> {
        self.forwards
            .get_or_insert_with(|| cx.new(Registry::new))
            .clone()
    }

    /// The Port forwards view of `leaf`'s sidebar, on `pane`'s host.
    pub(super) fn sidebar_forwards(
        &mut self,
        leaf: LeafId,
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
                ForwardsEvent::Close => this.close_sidebar(leaf, window, cx),
            },
        );
        Some(SidebarView {
            view,
            _events: events,
        })
    }

    /// Opens the active pane's sidebar on Port forwards, with the keyboard
    /// in the add row (spec 0017, flow 1). A local pane has no host.
    pub(super) fn show_port_forwards(
        &mut self,
        _: &ShowPortForwards,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let leaf = self.tabs.active().focused;
        if !self.show_sidebar(leaf, Shown::Forwards, window, cx) {
            show_notice("Port forwards need an SSH pane".into(), window, cx);
            return;
        }
        if let Some(forwards) = self.sftp.get(&leaf).and_then(|s| s.forwards.as_ref()) {
            forwards
                .view
                .update(cx, |view, cx| view.focus_add(window, cx));
        }
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
}
