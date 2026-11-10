//! Saving the session (spec 0003, flow 5) and restoring what it saved:
//! the debounced save, the last save at exit, and the restored panes that
//! start when their tab is first shown (flow 6).

use super::tree::LeafId;
use super::{Workspace, show_notice};
use crate::config::{Config, Profile, ProfileKind};
use crate::files::pane::Target;
use crate::session::{self, SavedFiles, SavedLeaf, SavedNode, SavedTab, Session, WindowSize};
use crate::terminal::pty::Command;
use crate::terminal::{Launch, Start, connect_prompt};
use gpui::{App, AppContext, Context, Window};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// How long the layout must stay put before the session is saved (spec
/// 0003, flow 5).
const SAVE_DEBOUNCE: Duration = Duration::from_secs(1);

/// What a restored files pane shows (spec 0013, Saved): its SSH profile
/// if that's still one, disconnected, else Local at home, with a notice.
pub fn restored_files(
    config: &Config,
    saved: &SavedFiles,
    notices: &mut Vec<String>,
) -> (Target, Option<String>) {
    match &saved.target {
        None => (Target::Local, saved.folder.clone()),
        Some(id) => match config.profile(id).map(|profile| profile.kind) {
            Some(ProfileKind::Ssh(_)) => (Target::Ssh(id.clone()), saved.folder.clone()),
            _ => {
                notices.push(format!(
                    "A restored files pane's host `{id}` no longer exists, so it shows Local."
                ));
                (Target::Local, None)
            }
        },
    }
}

/// The profile a restored pane names, or `default_profile`, with a notice,
/// if that profile no longer exists (spec 0003, flow 6).
pub fn restored_profile(config: &Config, id: &str, notices: &mut Vec<String>) -> Profile {
    config.profile(id).unwrap_or_else(|| {
        let instead = config.default_profile();
        notices.push(format!(
            "A restored pane's profile `{id}` no longer exists, so it opened `{}`.",
            instead.display_name()
        ));
        instead
    })
}

/// The window's size in logical pixels, as the session keeps it.
pub(super) fn window_size(window: &Window) -> WindowSize {
    let size = window.window_bounds().get_bounds().size;
    WindowSize {
        width: f32::from(size.width),
        height: f32::from(size.height),
    }
}

impl Workspace {
    /// Saves the session once the layout has stayed put for a second (spec
    /// 0003, flow 5), writing off the UI thread. Not in bench runs, nor with
    /// `restore_session: false`.
    pub(super) fn save_soon(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(path) = self.session_path.clone() else {
            return;
        };
        if !self.config.restore_session {
            return;
        }
        // Replacing the waiting save cancels it.
        self.save_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            let Ok(session) = this.update(cx, |this, cx| {
                this.set_aside_bad_session();
                this.snapshot(cx)
            }) else {
                return;
            };
            let saved = cx
                .background_spawn(async move { session::save(&path, &session) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match saved {
                Ok(()) => this.save_failed = false,
                Err(error) if !std::mem::replace(&mut this.save_failed, true) => {
                    show_notice(
                        format!("The session couldn't be saved: {error}"),
                        window,
                        cx,
                    );
                }
                Err(_) => {}
            });
        }));
    }

    /// The last save, as the window closes: the session as it stands
    /// (spec 0003, flow 5). Then every program stops, so no shell outlives
    /// the window (spec 0002, flow 10).
    pub(super) fn save_on_exit(&mut self, cx: &mut App) {
        let session = self.snapshot(cx);
        self.save_last(session);
        for pane in self.panes.values() {
            pane.update(cx, |pane, _| pane.shutdown());
        }
    }

    /// Writes `session` as the last save, or deletes the saved one with
    /// `restore_session: false`. Once only, right away: nothing runs after.
    pub(super) fn save_last(&mut self, session: Session) {
        let Some(path) = self.session_path.take() else {
            return;
        };
        self.save_task = None;
        self.set_aside_bad_session();
        let result = if self.config.restore_session {
            session::save(&path, &session)
        } else {
            session::delete(&path)
        };
        if let Err(error) = result {
            eprintln!("tern: the session couldn't be saved: {error}");
        }
    }

    /// The session as it stands: every tab's layout with each pane's
    /// profile, directory and title (spec 0003). Never what a pane shows.
    fn snapshot(&self, cx: &App) -> Session {
        let tabs = self
            .tabs
            .tabs()
            .iter()
            .map(|tab| SavedTab {
                name: tab.name.clone(),
                root: SavedNode::of(&tab.root, &mut |leaf| self.saved_leaf(leaf, cx)),
                focused: tab.focused,
                zoomed: tab.zoomed,
            })
            .collect();
        Session::new(self.window_size, tabs, self.tabs.active_index())
    }

    fn saved_leaf(&self, leaf: LeafId, cx: &App) -> SavedLeaf {
        if let Some(saved) = self.pending.get(&leaf) {
            return saved.clone();
        }
        if let Some(files) = self.files.get(&leaf) {
            let (target, folder) = files.pane.read(cx).saved(cx);
            return SavedLeaf {
                id: leaf,
                profile: self.config.default_profile().id,
                cwd: None,
                title: None,
                files: Some(SavedFiles { target, folder }),
            };
        }
        let (profile, cwd, title) = match self.panes.get(&leaf) {
            Some(pane) => {
                let pane = pane.read(cx);
                let title = pane.title().map(str::to_owned);
                (pane.profile().id.clone(), pane.cwd(), title)
            }
            None => (self.config.default_profile().id, None, None),
        };
        SavedLeaf {
            id: leaf,
            profile,
            cwd,
            title,
            files: None,
        }
    }

    /// Shows a restored pane's last title until its program sets one.
    pub(super) fn restore_title(&self, leaf: LeafId, title: Option<&str>, cx: &mut Context<Self>) {
        if let (Some(pane), Some(_)) = (self.panes.get(&leaf), title) {
            pane.update(cx, |pane, _| pane.set_title(title));
        }
    }

    /// Starts the shown tab's restored panes that haven't run yet: its tab
    /// is shown for the first time, or was dragged into view (spec 0003,
    /// flow 6; spec 0007, flow 7). SSH panes wait for Enter.
    pub(super) fn start_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for leaf in self.tabs.active().root.leaves() {
            let Some(saved) = self.pending.remove(&leaf) else {
                continue;
            };
            // Shown now, after the first frame: Local lists its folder.
            if let Some(files) = &saved.files {
                let mut notices = Vec::new();
                let (target, folder) = restored_files(&self.config, files, &mut notices);
                for notice in notices {
                    show_notice(notice, window, cx);
                }
                self.add_files(leaf, target, folder, false, window, cx);
                if let Some(files) = self.files.get(&leaf) {
                    files.pane.update(cx, |pane, cx| pane.start(window, cx));
                }
                continue;
            }
            let mut notices = Vec::new();
            let profile = restored_profile(&self.config, &saved.profile, &mut notices);
            for notice in notices {
                show_notice(notice, window, cx);
            }
            let cwd = saved
                .cwd
                .clone()
                .or_else(|| profile.cwd().map(Path::to_path_buf));
            if matches!(profile.kind, ProfileKind::Ssh(_)) {
                let launch = Launch {
                    command: Command::of_profile(&profile),
                    cwd,
                    timing: Arc::clone(&self.timing),
                };
                let theme = self.theme_for(profile.theme.as_deref(), window, cx);
                let start = Start::OnEnter(connect_prompt(profile.display_name()));
                self.add_pane(leaf, start, launch, (&profile).into(), theme, window, cx);
            } else {
                self.spawn_pane(leaf, profile, cwd, window, cx);
            }
            self.restore_title(leaf, saved.title.as_deref(), cx);
        }
    }

    /// Renames a session that couldn't be restored to `session.json.bad`,
    /// once: after the first frame, or before a save that comes first.
    pub(super) fn set_aside_bad_session(&mut self) {
        if let Some(path) = self.bad_session.take()
            && let Err(error) = session::keep_bad(&path)
        {
            eprintln!("tern: the unrestorable session couldn't be set aside: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    /// Spec 0003, flow 6: a restored pane whose profile no longer exists
    /// opens `default_profile`, with one notice naming the profile that's
    /// gone. A pane whose profile is still there opens it without one.
    #[test]
    fn a_restored_pane_whose_profile_is_gone_opens_default_profile_with_a_notice() {
        let config = config::parse(concat!(
            "default_profile: htop\n",
            "profiles:\n",
            "  htop:\n",
            "    type: local\n",
            "    command: [htop]\n",
            "  vim:\n",
            "    type: local\n",
            "    command: [vim]\n",
        ))
        .config;
        let mut notices = Vec::new();
        let opened = restored_profile(&config, "old-project", &mut notices);
        assert_eq!(opened.id, "htop");
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("old-project"), "{}", notices[0]);
        notices.clear();
        assert_eq!(restored_profile(&config, "vim", &mut notices).id, "vim");
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// Spec 0013, Saved: a restored files pane shows Local at its folder,
    /// or its SSH profile at its folder (disconnected, so no connection);
    /// a profile that no longer exists shows Local at home, with one
    /// notice naming it.
    #[test]
    fn a_restored_files_pane_whose_host_is_gone_shows_local_at_home_with_a_notice() {
        let config = config::parse(concat!(
            "profiles:\n",
            "  prod:\n",
            "    type: ssh\n",
            "    host: prod.example.com\n",
        ))
        .config;
        let saved = |target: Option<&str>| SavedFiles {
            target: target.map(str::to_owned),
            folder: Some("/srv".into()),
        };
        let mut notices = Vec::new();
        assert_eq!(
            restored_files(&config, &saved(None), &mut notices),
            (Target::Local, Some("/srv".into()))
        );
        assert_eq!(
            restored_files(&config, &saved(Some("prod")), &mut notices),
            (Target::Ssh("prod".into()), Some("/srv".into()))
        );
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(
            restored_files(&config, &saved(Some("old-host")), &mut notices),
            (Target::Local, None)
        );
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("old-host"), "{}", notices[0]);
    }
}
