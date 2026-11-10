//! Drawing the browser: the header, the path row, the status, the tree's
//! rows, and where each area's own parts go.

use super::{
    Browser, BrowserEvent, CONTEXT, Collapse, Connection, DeleteSelected, Dismiss,
    DownloadSelected, DownloadSelectedTo, EditSelected, Expand, FocusNextControl, FocusPath,
    FocusPreviousControl, GoUp, INDENT, LeavePath, NewFolder, OpenSelected, PATH_CONTEXT,
    ROW_HEIGHT, Refresh, RenameSelected, SelectNext, SelectPrevious, ShowMenu, Source, UploadFiles,
};
use crate::files::MAX_ENTRIES;
use crate::files::tree::Row;
use crate::terminal::clean_title;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, ClickEvent, Context, ExternalPaths, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, anchored,
    deferred, div, px, uniform_list,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::menu::{ContextMenuExt, DropdownMenu};
use gpui_component::{ActiveTheme, Disableable, Icon, IconName, Sizable};
use std::rc::Rc;

impl Browser {
    fn render_row(&self, ix: usize, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger, selected_bg, hover, drop) = (
            theme.muted_foreground,
            theme.danger,
            theme.list_active,
            theme.list_hover,
            theme.drag_border,
        );
        let indent = px(8. + INDENT * row.depth() as f32);
        let line = |id: &'static str| {
            div()
                .id((id, ix))
                .h(px(ROW_HEIGHT))
                .pl(indent)
                .pr(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_xs()
                .text_color(muted)
        };
        let entry = match row {
            Row::Loading { .. } => {
                return line("files-loading").child("Loading…").into_any_element();
            }
            Row::Truncated { .. } => {
                let note = format!("Showing the first {MAX_ENTRIES} entries");
                return line("files-truncated").child(note).into_any_element();
            }
            Row::Entry { .. } => self.entry(ix),
        };
        let Some((entry, path)) = entry else {
            return line("files-gone").into_any_element();
        };
        let folder = entry.is_folder();
        let expanded = folder && self.tree.is_expanded(&path);
        let failed = folder.then(|| self.tree.error(&path)).flatten();
        let icon = if folder {
            IconName::Folder
        } else {
            IconName::File
        };
        let chevron = if expanded {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };
        let size = entry
            .size
            .filter(|_| !folder)
            .map(crate::files::human_size)
            .unwrap_or_default();
        let modified = entry.modified.map(crate::files::date).unwrap_or_default();
        let detail = match failed {
            Some(error) => div()
                .flex_shrink_0()
                .max_w(px(220.))
                .truncate()
                .text_xs()
                .text_color(danger)
                .child(error.to_owned())
                .into_any_element(),
            None => div()
                .flex()
                .flex_shrink_0()
                .gap(px(6.))
                .text_xs()
                .text_color(muted)
                .child(size)
                .child(modified)
                .into_any_element(),
        };
        let focus = self.focus.clone();
        let (local, sftp) = (self.is_local(), !self.is_local());
        div()
            .id(("files-entry", ix))
            .h(px(ROW_HEIGHT))
            .pl(indent)
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(4.))
            .when(self.selected == Some(ix), |row| row.bg(selected_bg))
            .hover(|row| row.bg(hover))
            // First, so it opens at the row's start.
            .children(
                self.key_menu
                    .as_ref()
                    .filter(|open| open.ix == ix)
                    .map(|open| {
                        deferred(
                            anchored()
                                .snap_to_window_with_margin(px(8.))
                                .child(div().mt(px(ROW_HEIGHT)).child(open.menu.clone())),
                        )
                        .with_priority(1)
                    }),
            )
            .child(
                div()
                    .id(("files-chevron", ix))
                    .w(px(14.))
                    .flex_shrink_0()
                    .when(folder, |slot| {
                        slot.child(Icon::new(chevron).xsmall().text_color(muted))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                window.focus(&this.focus, cx);
                                this.selected = Some(ix);
                                if expanded {
                                    this.collapse(ix, cx);
                                } else {
                                    this.expand(ix, false, cx);
                                }
                            }))
                    }),
            )
            .child(Icon::new(icon).xsmall().text_color(muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(entry.link, |name| name.italic())
                    .child(clean_title(&entry.name)),
            )
            .child(detail)
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.selected = Some(ix);
                if event.click_count() >= 2 {
                    this.open(ix, cx);
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| {
                    window.focus(&this.focus, cx);
                    this.selected = Some(ix);
                    cx.notify();
                }),
            )
            // Files dropped on a remote folder row go into that folder
            // (spec 0008, flow 5).
            .when(folder && sftp, |row| {
                row.drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(drop))
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                        cx.stop_propagation();
                        this.upload(paths.paths().to_vec(), path.clone(), cx);
                    }))
            })
            .context_menu(move |menu, _, _| Self::entry_menu(menu, focus.clone(), folder, local))
            .into_any_element()
    }

    fn render_status(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let line = |text: SharedString| div().px(px(8.)).py(px(4.)).text_xs().child(text);
        let connect = |id: &'static str, label: &'static str, cx: &mut Context<Self>| {
            div().px(px(8.)).child(
                Button::new(id)
                    .label(label)
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))),
            )
        };
        match &self.connection {
            Connection::Idle if self.is_local() => None,
            Connection::Idle => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(line("Not connected".into()).text_color(muted))
                    .child(connect("files-connect", "Connect", cx))
                    .into_any_element(),
            ),
            Connection::Connecting => Some(
                line("Connecting…".into())
                    .text_color(muted)
                    .into_any_element(),
            ),
            Connection::Ended(reason) => {
                let said = if self.is_local() { "" } else { "ssh said:" };
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .when(!said.is_empty(), |status| {
                            status.child(line(said.into()).text_color(muted))
                        })
                        .child(line(reason.clone()).text_color(danger).whitespace_normal())
                        .child(connect("files-reconnect", "Connect again", cx))
                        .into_any_element(),
                )
            }
            Connection::Connected { .. } => match (&self.error, &self.loading) {
                (Some(error), _) => Some(line(error.clone()).text_color(danger).into_any_element()),
                (None, Some(_)) => {
                    Some(line("Loading…".into()).text_color(muted).into_any_element())
                }
                (None, None) => None,
            },
        }
    }

    /// The header's title: the target, as a dropdown in a files pane.
    fn render_title(&self) -> AnyElement {
        let label = clean_title(&self.target);
        match &self.title_menu {
            Some(menu) => {
                let menu = Rc::clone(menu);
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        Button::new("files-target")
                            .label(label)
                            .icon(IconName::ChevronDown)
                            .ghost()
                            .small()
                            .tooltip("Switch between Local and your hosts")
                            .dropdown_menu(move |popup, window, cx| menu(popup, window, cx)),
                    )
                    .into_any_element()
            }
            None => div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .child(label)
                .into_any_element(),
        }
    }
}

impl Render for Browser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.follow_asks(window, cx);
        let theme = cx.theme();
        let (border, muted, drop) = (theme.border, theme.muted_foreground, theme.drag_border);
        let shown = self.link.is_some() && self.tree.root().is_some();
        let sftp = !self.is_local();
        let closable = matches!(self.source, Source::Pane(_));
        let header = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(8.))
            .py(px(4.))
            // In a files pane, the split's grab handle sits in the top
            // right corner (spec 0007): the buttons stay clear of it.
            .when(self.title_menu.is_some(), |header| header.pr(px(32.)))
            .border_b_1()
            .border_color(border)
            .child(self.render_title())
            .when(sftp, |header| {
                header.child(
                    Button::new("files-upload")
                        .icon(IconName::Plus)
                        .ghost()
                        .xsmall()
                        .tooltip("Upload files… (Ctrl+U)")
                        .disabled(!shown)
                        .on_click(cx.listener(|this, _, window, cx| this.pick_uploads(window, cx))),
                )
            })
            .child(
                Button::new("files-new-folder")
                    .icon(IconName::FolderClosed)
                    .ghost()
                    .xsmall()
                    .tooltip("New folder (Ctrl+N)")
                    .disabled(!shown)
                    .on_click(cx.listener(|this, _, window, cx| this.start_new_folder(window, cx))),
            )
            .child(
                Button::new("files-refresh")
                    .icon(IconName::RefreshCw)
                    .ghost()
                    .xsmall()
                    .tooltip("Refresh (F5)")
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .when(closable, |header| {
                header.child(
                    Button::new("files-close")
                        .icon(IconName::Close)
                        .ghost()
                        .xsmall()
                        .tooltip("Close sidebar")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(BrowserEvent::Close))),
                )
            });
        let path_row = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(6.))
            .py(px(4.))
            .child(
                Button::new("files-up")
                    .icon(IconName::ArrowUp)
                    .ghost()
                    .xsmall()
                    .tooltip("Up (Backspace)")
                    .on_click(cx.listener(|this, _, _, cx| this.go_up(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .key_context(PATH_CONTEXT)
                    .on_action(cx.listener(|this, _: &LeavePath, window, cx| {
                        window.focus(&this.focus, cx);
                    }))
                    .child(Input::new(&self.path).xsmall()),
            );
        let rows = self.tree.rows().len();
        let list = match self.tree.root_listing() {
            Some(listing) if listing.entries.is_empty() => div()
                .px(px(8.))
                .py(px(4.))
                .text_xs()
                .text_color(muted)
                .child("Empty folder")
                .into_any_element(),
            Some(_) => uniform_list(
                "files-rows",
                rows,
                cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                    range
                        .filter_map(|ix| {
                            let row = this.tree.rows().get(ix)?.clone();
                            Some(this.render_row(ix, &row, cx))
                        })
                        .collect::<Vec<AnyElement>>()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element(),
            None => div().into_any_element(),
        };
        // Files dropped anywhere else on a remote tree go into its root
        // folder (spec 0008, flow 5).
        let list = div()
            .id("files-drop")
            .flex_1()
            .min_h_0()
            .when(sftp, |list| {
                list.drag_over::<ExternalPaths>(move |style, _, _, _| {
                    style.border_1().border_color(drop)
                })
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                    if let Some(folder) = this.tree.root().map(str::to_owned) {
                        this.upload(paths.paths().to_vec(), folder, cx);
                    }
                }))
            })
            .child(list);
        div()
            .id("file-browser")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            // The tree is the first tab stop; the controls follow in order.
            .tab_group()
            .tab_index(0)
            .on_action(cx.listener(|_, _: &FocusNextControl, window, cx| {
                window.focus_next(cx);
            }))
            .on_action(cx.listener(|_, _: &FocusPreviousControl, window, cx| {
                window.focus_prev(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.select(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.select(1, cx)))
            .on_action(cx.listener(|this, _: &Expand, _, cx| {
                if let Some(ix) = this.selected {
                    this.expand(ix, true, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Collapse, _, cx| {
                if let Some(ix) = this.selected {
                    this.collapse(ix, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSelected, window, cx| {
                // Enter on a question's button presses that button.
                if let Some(button) = this.focused_answer(window) {
                    match button {
                        0 => this.answer(true, cx),
                        1 => this.answer_edit(cx),
                        _ => this.answer(false, cx),
                    }
                } else if !this.focus.is_focused(window) {
                    // Another control has the keyboard: Enter is its click.
                    cx.propagate();
                } else if !this.asks.is_empty() {
                    this.confirm(cx);
                } else if matches!(this.connection, Connection::Idle | Connection::Ended(_)) {
                    this.connect(window, cx);
                } else if let Some(ix) = this.selected {
                    this.open(ix, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &EditSelected, _, cx| {
                if let Some(ix) = this.selected.filter(|_| !this.is_local()) {
                    this.edit(ix, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DownloadSelected, _, cx| {
                if let Some(ix) = this.selected.filter(|_| !this.is_local()) {
                    this.download(ix, None, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DownloadSelectedTo, window, cx| {
                if let Some(ix) = this.selected.filter(|_| !this.is_local()) {
                    this.download_to(ix, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &UploadFiles, window, cx| {
                if !this.is_local() {
                    this.pick_uploads(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NewFolder, window, cx| {
                this.start_new_folder(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowMenu, window, cx| {
                this.show_menu(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPath, window, cx| {
                this.path.update(cx, |input, cx| {
                    input.focus(window, cx);
                    input.select_all(window, cx);
                });
            }))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| this.answer(false, cx)))
            .on_action(cx.listener(|this, _: &GoUp, _, cx| this.go_up(cx)))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.refresh(cx)))
            .on_action(cx.listener(|this, _: &DeleteSelected, _, cx| {
                // One question at a time: a second Delete doesn't queue another.
                if let Some(ix) = this.selected.filter(|_| this.asks.is_empty()) {
                    this.ask_delete(ix, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &RenameSelected, window, cx| {
                if let Some(ix) = this.selected {
                    this.start_rename(ix, window, cx);
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .text_sm()
            .child(header)
            .child(path_row)
            .children(self.render_status(cx))
            .children(self.render_ask(cx))
            .children(self.render_naming(cx))
            .child(list)
            .children(self.render_edits(cx))
            .children(self.render_transfers(cx))
    }
}
