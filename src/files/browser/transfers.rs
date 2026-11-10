//! Downloads and uploads (spec 0008, flows 4 and 5): the OS's dialogs,
//! dropped files, the jobs and their rows.

use super::{Browser, BrowserEvent, Job, Transfer};
use crate::sftp::client::{Download, Upload};
use crate::terminal::clean_title;
use gpui::{Context, IntoElement, ParentElement, PathPromptOptions, Styled, Window, div, px};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::progress::Progress;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable};
use std::path::PathBuf;

impl Browser {
    pub(super) fn transfer(&mut self, id: u64) -> Option<&mut Transfer> {
        self.transfers.iter_mut().find(|transfer| transfer.id == id)
    }

    /// Downloads the file at `ix` into `folder`, the Downloads folder if
    /// `None` (spec 0008, flow 4). Its local name escapes whatever could
    /// make it a path (TM-sftp-03).
    pub(super) fn download(&mut self, ix: usize, folder: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some((entry, remote)) = self.entry(ix).filter(|(entry, _)| !entry.is_folder()) else {
            return;
        };
        let name = entry.name.clone();
        let Some(folder) = folder.or_else(dirs::download_dir).or_else(dirs::home_dir) else {
            cx.emit(BrowserEvent::Notice("There's no Downloads folder".into()));
            return;
        };
        let Some(target) = crate::sftp::local_path(&folder, &name, cfg!(windows)) else {
            let notice = format!("{} can't be saved under its name", clean_title(&name));
            cx.emit(BrowserEvent::Notice(notice));
            return;
        };
        self.start_job(Job::Download { remote, target }, false, cx);
    }

    /// Asks the OS for a folder, then downloads the file at `ix` there.
    pub(super) fn download_to(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, remote)) = self.entry(ix) else {
            return;
        };
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Download here".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let folder = match picked.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(error)) => {
                    let notice = format!("Couldn't open the file dialog: {error}");
                    let _ = this.update(cx, |_, cx| cx.emit(BrowserEvent::Notice(notice)));
                    None
                }
                _ => None,
            };
            let Some(folder) = folder else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                // The tree may have changed while the dialog was open.
                let ix = this
                    .entry(ix)
                    .filter(|(_, path)| *path == remote)
                    .map(|_| ix);
                if let Some(ix) = ix {
                    this.download(ix, Some(folder), cx);
                }
            });
        })
        .detach();
    }

    /// Asks the OS for files, then uploads them into the root folder.
    pub(super) fn pick_uploads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.tree.root().map(str::to_owned) else {
            return;
        };
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Upload".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let paths = match picked.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Err(error)) => {
                    let notice = format!("Couldn't open the file dialog: {error}");
                    let _ = this.update(cx, |_, cx| cx.emit(BrowserEvent::Notice(notice)));
                    return;
                }
                _ => return,
            };
            let _ = this.update(cx, |this, cx| this.upload(paths, folder, cx));
        })
        .detach();
    }

    /// Uploads local files into remote `folder` (spec 0008, flow 5). A
    /// folder among them fails with a reason.
    pub(super) fn upload(&mut self, paths: Vec<PathBuf>, folder: String, cx: &mut Context<Self>) {
        for local in paths {
            let Some(name) = local
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            let job = Job::Upload {
                local,
                folder: folder.clone(),
                name,
            };
            self.start_job(job, false, cx);
        }
    }

    /// Starts `job`; with `replace`, the user confirmed writing over what's
    /// there.
    pub(super) fn start_job(&mut self, job: Job, replace: bool, cx: &mut Context<Self>) {
        if self.sftp().is_none() {
            return;
        }
        let id = self.next_id();
        let Some(client) = self.sftp() else {
            return;
        };
        let name = match &job {
            Job::Download { remote, .. } => remote.rsplit('/').next().unwrap_or(remote),
            Job::Upload { name, .. } => name,
        };
        let name = clean_title(name).into();
        match job.clone() {
            Job::Download { remote, target } => client.download(Download {
                id,
                remote,
                target,
                replace,
                private: false,
            }),
            Job::Upload {
                local,
                folder,
                name,
            } => client.upload(Upload {
                id,
                local,
                folder,
                name,
                replace,
                expect: None,
            }),
        }
        self.transfers.push(Transfer {
            id,
            job,
            name,
            done: 0,
            total: None,
            error: None,
        });
        cx.notify();
    }

    /// Stops a running transfer, or dismisses a failed one.
    fn cancel(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(client) = self.sftp() {
            client.cancel(id);
        }
        self.transfers.retain(|transfer| transfer.id != id);
        cx.notify();
    }

    pub(super) fn render_transfers(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.transfers.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let (border, muted, danger) = (theme.border, theme.muted_foreground, theme.danger);
        let rows = self.transfers.iter().map(|transfer| {
            let id = transfer.id;
            let icon = match transfer.job {
                Job::Download { .. } => IconName::ArrowDown,
                Job::Upload { .. } => IconName::ArrowUp,
            };
            let percent = transfer
                .total
                .filter(|&total| total > 0)
                .map(|total| transfer.done as f32 * 100.0 / total as f32);
            let amount = match transfer.total {
                Some(total) => format!(
                    "{} / {}",
                    crate::files::human_size(transfer.done),
                    crate::files::human_size(total)
                ),
                None => crate::files::human_size(transfer.done),
            };
            let status = match &transfer.error {
                Some(error) => div()
                    .text_color(danger)
                    .whitespace_normal()
                    .child(error.clone())
                    .into_any_element(),
                None => Progress::new(("files-progress", id))
                    .value(percent.unwrap_or_default())
                    .loading(percent.is_none() && transfer.done == 0)
                    .into_any_element(),
            };
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
                        .gap(px(6.))
                        .child(Icon::new(icon).xsmall().text_color(muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(transfer.name.clone()),
                        )
                        .child(div().flex_shrink_0().text_color(muted).child(amount))
                        .child(
                            Button::new(("files-cancel", id))
                                .icon(IconName::Close)
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, _, cx| this.cancel(id, cx))),
                        ),
                )
                .child(status)
        });
        Some(
            div()
                .flex_shrink_0()
                .max_h(px(200.))
                .border_t_1()
                .border_color(border)
                .text_xs()
                .children(rows),
        )
    }
}
