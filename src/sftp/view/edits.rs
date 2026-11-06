//! A session's edits (spec 0008, flow 3): a remote file downloaded to a
//! private copy, opened in the OS's default app, and watched. A save marks
//! it changed; it uploads as its mode says, only if the host's file is
//! still the one fetched, else the sidebar asks. Copies outlive a dropped
//! connection and go when the session does.

use super::{Ask, SftpEvent, SftpView};
use crate::config::UploadMode;
use crate::sftp::client::{Download, RemoteStat, Upload};
use crate::sftp::{copies, local_name, local_path, runs_when_opened};
use crate::terminal::clean_title;
use gpui::{Context, IntoElement, ParentElement, SharedString, Styled, div, px};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
use gpui_component::progress::Progress;
use gpui_component::{ActiveTheme, Disableable, Icon, IconName, Sizable};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How often the copies are looked at.
const POLL: Duration = Duration::from_secs(1);

/// A copy's size and modification time: what tells a save.
type LocalStat = (u64, SystemTime);

pub(super) struct Edit {
    id: u64,
    /// The remote file, and its folder and name to upload to.
    remote: String,
    folder: String,
    name: String,
    /// Its name, by the title rules.
    label: SharedString,
    copy: PathBuf,
    mode: UploadMode,
    state: State,
    /// The host's file as fetched or as last uploaded.
    fetched: Option<RemoteStat>,
    /// The copy as fetched or as last uploaded: what "no changes" means.
    baseline: Option<LocalStat>,
    /// The copy as the last look saw it, and whether it moved since the
    /// look before. A change counts once it holds still for a look, so a
    /// save in progress isn't uploaded half-written (TM-sftp-09).
    seen: Option<LocalStat>,
    moving: bool,
    /// The copy as it was when the running upload started.
    sending: Option<LocalStat>,
    pub(super) done: u64,
    pub(super) total: Option<u64>,
    uploaded: Option<Instant>,
}

#[derive(Clone, PartialEq)]
enum State {
    /// Downloading to the copy.
    Opening,
    /// The host has what the copy has.
    Clean,
    Changed,
    Uploading,
    /// The last upload failed, and why; the next trigger tries again.
    Failed(SharedString),
}

impl Edit {
    /// The copy has changes the host doesn't.
    fn unsent(&self) -> bool {
        matches!(
            self.state,
            State::Changed | State::Uploading | State::Failed(_)
        )
    }

    /// Takes in what a look at the copy found. `true` when a save just
    /// counted: the copy changed, then held still for a look, and differs
    /// from what the host has; the edit is then `changed`. The first look
    /// after the fetch is the baseline; nothing counts while it's opening
    /// or uploading.
    fn look(&mut self, stat: LocalStat) -> bool {
        if self.state == State::Opening {
            return false;
        }
        if self.baseline.is_none() {
            self.baseline = Some(stat);
            self.seen = Some(stat);
            return false;
        }
        if self.seen != Some(stat) {
            self.seen = Some(stat);
            self.moving = true;
            return false;
        }
        if !self.moving || self.state == State::Uploading {
            return false;
        }
        self.moving = false;
        if self.baseline == Some(stat) {
            return false;
        }
        if matches!(self.state, State::Clean | State::Failed(_)) {
            self.state = State::Changed;
        }
        true
    }
}

impl SftpView {
    pub(super) fn edit_mut(&mut self, id: u64) -> Option<&mut Edit> {
        self.edits.iter_mut().find(|edit| edit.id == id)
    }

    pub(super) fn edit_label(&self, id: u64) -> SharedString {
        self.edits
            .iter()
            .find(|edit| edit.id == id)
            .map(|edit| edit.label.clone())
            .unwrap_or_default()
    }

    /// Edits the file at `ix`: fetches it to its copy, then opens it. A
    /// file already being edited opens again; one the OS would run is
    /// downloaded instead, and the sidebar says so (TM-sftp-05).
    pub(super) fn edit(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some((entry, remote)) = self.entry(ix).filter(|(entry, _)| !entry.is_folder()) else {
            return;
        };
        let name = entry.name.clone();
        if let Some(edit) = self.edits.iter().find(|edit| edit.remote == remote) {
            if edit.state != State::Opening {
                cx.open_with_system(&edit.copy);
            }
            return;
        }
        let windows = cfg!(windows);
        let pathext = std::env::var("PATHEXT").ok();
        if runs_when_opened(&local_name(&name, windows), windows, pathext.as_deref()) {
            let notice = format!(
                "{} would run rather than open, so it was downloaded instead",
                clean_title(&name)
            );
            cx.emit(SftpEvent::Notice(notice));
            self.download(ix, None, cx);
            return;
        }
        let Some(copies) = self.copies.clone() else {
            cx.emit(SftpEvent::Notice(
                "There's no data folder for edit copies".into(),
            ));
            return;
        };
        if self.client.is_none() {
            return;
        }
        let id = self.next_id();
        let Some(copy) = local_path(&copies.join(id.to_string()), &name, windows) else {
            return;
        };
        let folder = super::super::parent(&remote);
        if let Some(client) = &self.client {
            client.download(Download {
                id,
                remote: remote.clone(),
                target: copy.clone(),
                replace: true,
                private: true,
            });
        }
        self.edits.push(Edit {
            id,
            remote,
            folder,
            label: clean_title(&name).into(),
            name,
            copy,
            mode: self.settings.upload,
            state: State::Opening,
            fetched: None,
            baseline: None,
            seen: None,
            moving: false,
            sending: None,
            done: 0,
            total: None,
            uploaded: None,
        });
        self.start_poll(cx);
        cx.notify();
    }

    /// A fetch or an upload of edit `id` is complete.
    pub(super) fn edit_done(&mut self, id: u64, stat: Option<RemoteStat>, cx: &mut Context<Self>) {
        let Some(edit) = self.edit_mut(id) else {
            return;
        };
        match edit.state {
            State::Opening => {
                edit.fetched = stat;
                edit.state = State::Clean;
                let copy = edit.copy.clone();
                cx.open_with_system(&copy);
            }
            State::Uploading => {
                edit.fetched = stat;
                edit.baseline = edit.sending.take();
                edit.uploaded = Some(Instant::now());
                edit.state = State::Clean;
                // Saved again while it uploaded: that counts on the next
                // look.
                edit.moving = edit.seen != edit.baseline;
                self.settle(cx);
            }
            _ => {}
        }
    }

    /// A fetch or an upload of edit `id` failed. A failed fetch drops the
    /// edit; a failed upload stays changed and says why.
    pub(super) fn edit_failed(&mut self, id: u64, error: String, cx: &mut Context<Self>) {
        let Some(edit) = self.edit_mut(id) else {
            return;
        };
        match edit.state {
            State::Opening => {
                let notice = format!("Couldn't open {}: {error}", edit.label);
                self.remove_edit(id, cx);
                cx.emit(SftpEvent::Notice(notice));
            }
            State::Uploading => {
                edit.sending = None;
                edit.state = State::Failed(error.into());
                self.settle(cx);
            }
            _ => {}
        }
    }

    /// The host's file changed since edit `id` fetched it: nothing was
    /// written, and the sidebar asks before overwriting (flow 3).
    pub(super) fn edit_conflict(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(edit) = self.edit_mut(id) else {
            return;
        };
        edit.sending = None;
        edit.state = State::Changed;
        if !self
            .asks
            .iter()
            .any(|ask| matches!(ask, Ask::Overwrite(ask) if *ask == id))
        {
            self.asks.push_back(Ask::Overwrite(id));
        }
        cx.notify();
    }

    /// The connection ended: fetches are dropped, uploads failed. The
    /// rest stay, to upload once it's back (flow 7).
    pub(super) fn edits_disconnected(&mut self, cx: &mut Context<Self>) {
        let opening: Vec<u64> = self
            .edits
            .iter()
            .filter(|edit| edit.state == State::Opening)
            .map(|edit| edit.id)
            .collect();
        for id in opening {
            self.remove_edit(id, cx);
        }
        let mut failed = false;
        for edit in &mut self.edits {
            if edit.state == State::Uploading {
                edit.sending = None;
                edit.state = State::Failed("The connection closed".into());
                failed = true;
            }
        }
        if failed {
            self.settle(cx);
        }
    }

    /// Tern's window gained focus: `focus` edits with changes upload.
    pub(super) fn window_focused(&mut self, cx: &mut Context<Self>) {
        let due: Vec<u64> = self
            .edits
            .iter()
            .filter(|edit| edit.mode == UploadMode::Focus)
            .filter(|edit| matches!(edit.state, State::Changed | State::Failed(_)))
            .map(|edit| edit.id)
            .collect();
        for id in due {
            self.upload_edit(id, false, cx);
        }
    }

    /// Uploads edit `id` over its remote file: only if that's still as
    /// fetched, unless `force` (the user said so after being asked).
    pub(super) fn upload_edit(&mut self, id: u64, force: bool, cx: &mut Context<Self>) {
        let Some(client) = &self.client else {
            let notice = format!("{}: connect again to upload", self.target);
            cx.emit(SftpEvent::Notice(notice));
            return;
        };
        let Some(edit) = self.edits.iter_mut().find(|edit| edit.id == id) else {
            return;
        };
        if matches!(edit.state, State::Opening | State::Uploading) {
            return;
        }
        edit.state = State::Uploading;
        edit.sending = edit.seen;
        edit.done = 0;
        edit.total = None;
        client.upload(Upload {
            id,
            local: edit.copy.clone(),
            folder: edit.folder.clone(),
            name: edit.name.clone(),
            replace: true,
            expect: if force { None } else { edit.fetched },
        });
        cx.notify();
    }

    /// Forgets edit `id` and deletes its copy; a running fetch or upload
    /// stops.
    pub(super) fn remove_edit(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self.edits.iter().position(|edit| edit.id == id) else {
            return;
        };
        let edit = self.edits.remove(ix);
        if let Some(client) = &self.client
            && matches!(edit.state, State::Opening | State::Uploading)
        {
            client.cancel(id);
        }
        if let Some(folder) = edit.copy.parent() {
            copies::remove_dir(folder.to_path_buf());
        }
        self.asks
            .retain(|ask| !matches!(ask, Ask::Overwrite(ask) | Ask::Discard(ask) if *ask == id));
        self.settle(cx);
        cx.notify();
    }

    /// The edit row's close button: a fetch stops, an upload stops and
    /// leaves the edit changed, changes not on the host ask first.
    fn close_edit(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(edit) = self.edit_mut(id) else {
            return;
        };
        match edit.state {
            State::Opening | State::Clean => self.remove_edit(id, cx),
            State::Uploading => {
                edit.sending = None;
                edit.state = State::Changed;
                if let Some(client) = &self.client {
                    client.cancel(id);
                }
                self.settle(cx);
            }
            State::Changed | State::Failed(_) => {
                if !self
                    .asks
                    .iter()
                    .any(|ask| matches!(ask, Ask::Discard(ask) if *ask == id))
                {
                    self.asks.push_back(Ask::Discard(id));
                }
            }
        }
        cx.notify();
    }

    /// Sets edit `id`'s upload mode, for as long as it lives (spec 0008,
    /// Model). Switching to `save` uploads what's changed.
    fn set_mode(&mut self, id: u64, mode: UploadMode, cx: &mut Context<Self>) {
        let Some(edit) = self.edit_mut(id) else {
            return;
        };
        edit.mode = mode;
        if edit.mode == UploadMode::Save && matches!(edit.state, State::Changed) {
            self.upload_edit(id, false, cx);
        }
        cx.notify();
    }

    /// Tells a close waiting on uploads that none is running now.
    fn settle(&mut self, cx: &mut Context<Self>) {
        if !self.edits.iter().any(|edit| edit.state == State::Uploading) {
            cx.emit(SftpEvent::Settled);
        }
    }

    /// Looks at the copies every [`POLL`] while the session lives, off the
    /// UI thread.
    fn start_poll(&mut self, cx: &mut Context<Self>) {
        if self.poll.is_some() {
            return;
        }
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let Ok(copies) = this.update(cx, |this, _| {
                    this.edits
                        .iter()
                        .filter(|edit| edit.state != State::Opening)
                        .map(|edit| (edit.id, edit.copy.clone()))
                        .collect::<Vec<_>>()
                }) else {
                    break;
                };
                let looked = cx
                    .background_executor()
                    .spawn(async move {
                        copies
                            .into_iter()
                            .map(|(id, copy)| (id, local_stat(&copy)))
                            .collect::<Vec<_>>()
                    })
                    .await;
                if this.update(cx, |this, cx| this.looked(looked, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// What a look at the copies found. A copy that changed and then held
    /// still is changed; in `save` mode it uploads.
    fn looked(&mut self, looked: Vec<(u64, Option<LocalStat>)>, cx: &mut Context<Self>) {
        let mut due = Vec::new();
        for (id, stat) in looked {
            // Windows' apps may hold a copy locked: the next look tries again.
            let Some(stat) = stat else {
                continue;
            };
            let waiting = self
                .asks
                .iter()
                .any(|ask| matches!(ask, Ask::Overwrite(ask) if *ask == id));
            let Some(edit) = self.edits.iter_mut().find(|edit| edit.id == id) else {
                continue;
            };
            if edit.look(stat) && edit.mode == UploadMode::Save && !waiting {
                due.push(id);
            }
        }
        for id in due {
            self.upload_edit(id, false, cx);
        }
        // Also keeps "uploaded … ago" current.
        cx.notify();
    }

    /// The edits a close would delete: each one's name, and whether its
    /// changes aren't on the host (spec 0008, flow 3).
    pub fn open_edits(&self) -> Vec<(SharedString, bool)> {
        self.edits
            .iter()
            .filter(|edit| edit.state != State::Opening)
            .map(|edit| (edit.label.clone(), edit.unsent()))
            .collect()
    }

    /// Uploads every edit whose changes aren't on the host, for a close
    /// that waits on them.
    pub fn upload_unsent(&mut self, cx: &mut Context<Self>) {
        let unsent: Vec<u64> = self
            .edits
            .iter()
            .filter(|edit| matches!(edit.state, State::Changed | State::Failed(_)))
            .map(|edit| edit.id)
            .collect();
        for id in unsent {
            self.upload_edit(id, false, cx);
        }
    }

    /// Whether a close waiting on uploads can go on: `Some(true)` when the
    /// host has every change, `Some(false)` when one failed or waits on a
    /// question, `None` while one is still uploading.
    pub fn settled(&self) -> Option<bool> {
        if self.edits.iter().any(|edit| edit.state == State::Uploading) {
            None
        } else {
            Some(!self.edits.iter().any(Edit::unsent))
        }
    }

    pub(super) fn render_edits(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.edits.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let (border, muted, danger) = (theme.border, theme.muted_foreground, theme.danger);
        let rows = self.edits.iter().map(|edit| {
            let id = edit.id;
            let status = match &edit.state {
                State::Opening => "Opening…".to_owned(),
                State::Clean => match edit.uploaded {
                    Some(uploaded) => format!("Uploaded {}", ago(uploaded)),
                    None => "No changes".to_owned(),
                },
                State::Changed => "Changed, not uploaded".to_owned(),
                State::Uploading => "Uploading…".to_owned(),
                State::Failed(error) => format!("Upload failed: {error}"),
            };
            let failed = matches!(edit.state, State::Failed(_));
            let current = edit.mode;
            let mode = mode_label(current);
            let view = cx.entity().downgrade();
            let can_upload = matches!(edit.state, State::Changed | State::Failed(_));
            let progress = matches!(edit.state, State::Opening | State::Uploading).then(|| {
                let percent = edit
                    .total
                    .filter(|&total| total > 0)
                    .map(|total| edit.done as f32 * 100.0 / total as f32);
                Progress::new(("sftp-edit-progress", id))
                    .value(percent.unwrap_or_default())
                    .loading(percent.is_none())
            });
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(px(8.))
                .py(px(4.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(Icon::new(IconName::FileText).xsmall().text_color(muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(edit.label.clone()),
                        )
                        .child(
                            Button::new(("sftp-edit-mode", id))
                                .label(mode)
                                .icon(IconName::ChevronDown)
                                .ghost()
                                .xsmall()
                                .tooltip("When this edit uploads")
                                .dropdown_menu(move |menu, _, _| {
                                    MODES.iter().fold(menu, |menu, &choice| {
                                        let view = view.clone();
                                        menu.item(
                                            PopupMenuItem::new(mode_label(choice))
                                                .checked(choice == current)
                                                .on_click(move |_, _, cx| {
                                                    let _ = view.update(cx, |this, cx| {
                                                        this.set_mode(id, choice, cx)
                                                    });
                                                }),
                                        )
                                    })
                                }),
                        )
                        .child(
                            Button::new(("sftp-edit-upload", id))
                                .icon(IconName::ArrowUp)
                                .ghost()
                                .xsmall()
                                .tooltip("Upload")
                                .disabled(!can_upload)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.upload_edit(id, false, cx)
                                })),
                        )
                        .child(
                            Button::new(("sftp-edit-open", id))
                                .icon(IconName::ExternalLink)
                                .ghost()
                                .xsmall()
                                .tooltip("Open again")
                                .disabled(edit.state == State::Opening)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(edit) = this.edits.iter().find(|e| e.id == id) {
                                        cx.open_with_system(&edit.copy);
                                    }
                                })),
                        )
                        .child(
                            Button::new(("sftp-edit-close", id))
                                .icon(IconName::Close)
                                .ghost()
                                .xsmall()
                                .tooltip("Close edit")
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.close_edit(id, cx)),
                                ),
                        ),
                )
                .child(
                    div()
                        .text_color(if failed { danger } else { muted })
                        .whitespace_normal()
                        .child(status),
                )
                .children(progress)
        });
        Some(
            div()
                .flex_shrink_0()
                .border_t_1()
                .border_color(border)
                .text_xs()
                .child(div().px(px(8.)).pt(px(4.)).text_color(muted).child("Edits"))
                .children(rows),
        )
    }
}

/// The upload modes, in the order the edit's menu lists them.
const MODES: [UploadMode; 3] = [UploadMode::Manual, UploadMode::Save, UploadMode::Focus];

/// An upload mode as the edit row names it.
fn mode_label(mode: UploadMode) -> &'static str {
    match mode {
        UploadMode::Manual => "On click",
        UploadMode::Save => "On save",
        UploadMode::Focus => "On focus",
    }
}

/// A copy's size and modification time, if it can be read now.
fn local_stat(copy: &Path) -> Option<LocalStat> {
    let metadata = std::fs::metadata(copy).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

/// How long ago `then` was, roughly.
fn ago(then: Instant) -> String {
    match then.elapsed().as_secs() {
        0..60 => "just now".into(),
        seconds @ 60..3600 => format!("{} min ago", seconds / 60),
        seconds => format!("{} h ago", seconds / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    /// An edit in `state` whose fetch is done, nothing looked at yet.
    fn edit(state: State) -> Edit {
        Edit {
            id: 1,
            remote: "/srv/app/.env".into(),
            folder: "/srv/app".into(),
            name: ".env".into(),
            label: ".env".into(),
            copy: PathBuf::from("/nowhere/1/.env"),
            mode: UploadMode::Manual,
            state,
            fetched: None,
            baseline: None,
            seen: None,
            moving: false,
            sending: None,
            done: 0,
            total: None,
            uploaded: None,
        }
    }

    /// A copy of `size` bytes, last modified `seconds` after the epoch.
    fn stat(size: u64, seconds: u64) -> LocalStat {
        (size, UNIX_EPOCH + Duration::from_secs(seconds))
    }

    /// Spec 0008, flow 3: the first look after the fetch is what "no
    /// changes" means; it counts nothing, however often it's seen again.
    #[test]
    fn the_first_look_is_the_baseline_and_counts_nothing() {
        let mut edit = edit(State::Clean);
        for _ in 0..3 {
            assert!(!edit.look(stat(10, 100)));
        }
        assert!(edit.state == State::Clean);
    }

    /// TM-sftp-09: a save counts only once the copy held still for a
    /// look, then the edit is changed.
    #[test]
    fn a_save_counts_once_it_holds_still_for_one_look() {
        let mut edit = edit(State::Clean);
        edit.look(stat(10, 100));
        assert!(!edit.look(stat(12, 101)), "counted while it moved");
        assert!(edit.state == State::Clean);
        assert!(edit.look(stat(12, 101)));
        assert!(edit.state == State::Changed);
    }

    /// TM-sftp-09: a copy still being written isn't counted half-way: a
    /// save counts when it stops moving, not on each step.
    #[test]
    fn a_copy_still_being_written_counts_only_when_it_stops() {
        let mut edit = edit(State::Clean);
        edit.look(stat(10, 100));
        assert!(!edit.look(stat(100, 101)));
        assert!(!edit.look(stat(200, 102)));
        assert!(!edit.look(stat(300, 103)));
        assert!(edit.state == State::Clean);
        assert!(edit.look(stat(300, 103)));
    }

    /// A save is reported once, not again on every look while the copy
    /// stays as saved; the next save is reported again.
    #[test]
    fn each_save_is_reported_exactly_once() {
        let mut edit = edit(State::Clean);
        edit.look(stat(10, 100));
        edit.look(stat(12, 101));
        assert!(edit.look(stat(12, 101)));
        for _ in 0..3 {
            assert!(!edit.look(stat(12, 101)));
        }
        assert!(!edit.look(stat(14, 105)));
        assert!(edit.look(stat(14, 105)));
        assert!(edit.state == State::Changed);
    }

    /// A copy that changed and came back to exactly the baseline has no
    /// changes to upload.
    #[test]
    fn a_change_back_to_the_baseline_counts_nothing() {
        let mut edit = edit(State::Clean);
        edit.look(stat(10, 100));
        edit.look(stat(12, 101));
        assert!(!edit.look(stat(10, 100)));
        assert!(!edit.look(stat(10, 100)));
        assert!(edit.state == State::Clean);
    }

    /// Nothing counts while an upload runs.
    #[test]
    fn nothing_counts_while_uploading() {
        let mut edit = edit(State::Uploading);
        edit.look(stat(10, 100));
        edit.look(stat(12, 101));
        assert!(!edit.look(stat(12, 101)));
        assert!(!edit.look(stat(12, 101)));
        assert!(edit.state == State::Uploading);
    }

    /// Nothing counts while the copy is still being fetched.
    #[test]
    fn nothing_counts_while_opening() {
        let mut edit = edit(State::Opening);
        edit.look(stat(5, 100));
        edit.look(stat(10, 101));
        assert!(!edit.look(stat(10, 101)));
    }

    /// Spec 0008, flow 3: a failed upload stays unsent; the next real
    /// save makes it changed again.
    #[test]
    fn a_failed_edit_becomes_changed_on_the_next_save() {
        let mut edit = edit(State::Failed("The connection closed".into()));
        edit.look(stat(10, 100));
        assert!(!edit.look(stat(10, 100)));
        assert!(matches!(edit.state, State::Failed(_)));
        edit.look(stat(12, 101));
        assert!(edit.look(stat(12, 101)));
        assert!(edit.state == State::Changed);
    }
}
