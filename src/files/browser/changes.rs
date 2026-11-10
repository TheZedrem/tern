//! What writes to the target: rename, delete and new folder, the name
//! field they type into, and the questions asked before anything is
//! replaced, deleted or run.

use super::{
    ASK_CONTEXT, Ask, Browser, BrowserEvent, CancelName, Job, NAME_CONTEXT, NextAnswer,
    PreviousAnswer,
};
use crate::files::local;
use crate::terminal::clean_title;
use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement, ParentElement,
    Styled, Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Sizable};

/// The field a new name is typed into, and what it names.
pub(super) struct Naming {
    kind: NameKind,
    input: Entity<InputState>,
    _events: Subscription,
}

enum NameKind {
    Rename { folder: String, from: String },
    NewFolder { folder: String },
}

impl Browser {
    /// Asks before removing the entry at `ix`.
    pub(super) fn ask_delete(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some((entry, path)) = self.entry(ix) else {
            return;
        };
        // A link to a folder is removed as a link (TM-sftp-10).
        let folder = entry.is_folder() && !entry.link;
        self.asks.push_back(Ask::Delete { path, folder });
        cx.notify();
    }

    /// Answers the first question: `yes` goes ahead.
    pub(super) fn answer(&mut self, yes: bool, cx: &mut Context<Self>) {
        let Some(ask) = self.asks.pop_front() else {
            return;
        };
        self.asks_answered += 1;
        match (ask, yes) {
            (Ask::Replace(job), true) => self.start_job(job, true, cx),
            (Ask::Delete { path, folder }, true) => {
                if let Some(link) = &self.link {
                    link.delete(path, folder);
                }
            }
            (Ask::Rename { from, to }, true) => {
                if let Some(link) = &self.link {
                    link.rename(from, to, true);
                }
            }
            (Ask::Run(file), true) => cx.emit(BrowserEvent::Run(file)),
            (Ask::Overwrite(id), true) => self.upload_edit(id, true, cx),
            (Ask::Discard(id), true) => self.remove_edit(id, cx),
            // A declined overwrite leaves the edit changed; a close that
            // waits on it gives up.
            (Ask::Overwrite(_), false) => cx.emit(BrowserEvent::Settled),
            (_, false) => {}
        }
        cx.notify();
    }

    /// The Edit answer to a Run question: the file opens as plain text.
    pub(super) fn answer_edit(&mut self, cx: &mut Context<Self>) {
        if let Some(Ask::Run(file)) = self.asks.pop_front() {
            self.asks_answered += 1;
            self.edit_local(file, cx);
        }
        cx.notify();
    }

    /// Enter while a question is open. A Run question opens the file as
    /// text: running takes the button, never a key pressed in passing.
    pub(super) fn confirm(&mut self, cx: &mut Context<Self>) {
        match self.asks.front() {
            Some(Ask::Run(_)) => self.answer_edit(cx),
            Some(_) => self.answer(true, cx),
            None => {}
        }
    }

    /// Opens the name field to rename the entry at `ix`.
    pub(super) fn start_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some((entry, _)), Some(folder)) = (self.entry(ix), self.tree.folder_of(ix)) else {
            return;
        };
        let kind = NameKind::Rename {
            folder: folder.to_owned(),
            from: entry.name.clone(),
        };
        let name = entry.name.clone();
        self.start_naming(kind, &name, window, cx);
    }

    pub(super) fn start_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.tree.root().map(str::to_owned) else {
            return;
        };
        self.start_naming(NameKind::NewFolder { folder }, "", window, cx);
    }

    fn start_naming(
        &mut self,
        kind: NameKind,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = match kind {
            NameKind::Rename { .. } => "New name",
            NameKind::NewFolder { .. } => "Folder name",
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(clean_title(value))
        });
        let events = cx.subscribe_in(
            &input,
            window,
            |this, input, event, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let name = input.read(cx).value().trim().to_owned();
                    this.finish_naming(Some(name), window, cx);
                }
                InputEvent::Blur => this.finish_naming(None, window, cx),
                _ => {}
            },
        );
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.naming = Some(Naming {
            kind,
            input,
            _events: events,
        });
        cx.notify();
    }

    /// Ends the name field; `Some` name goes ahead if it's one path
    /// component (spec 0013, flow 5).
    fn finish_naming(&mut self, name: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(naming) = self.naming.take() else {
            return;
        };
        window.focus(&self.focus, cx);
        cx.notify();
        let Some(name) = name else {
            return;
        };
        let windows = self.is_local() && cfg!(windows);
        if let Err(why) = crate::files::valid_name(&name, windows) {
            cx.emit(BrowserEvent::Notice(why.into()));
            return;
        }
        let Some(link) = &self.link else {
            return;
        };
        let join = |folder: &str, name: &str| {
            if self.is_local() {
                local::join(folder, name)
            } else {
                crate::sftp::join(folder, name)
            }
        };
        match naming.kind {
            NameKind::Rename { folder, from } => {
                if name != from {
                    link.rename(join(&folder, &from), join(&folder, &name), false);
                }
            }
            NameKind::NewFolder { folder } => link.make_folder(join(&folder, &name)),
        }
    }

    /// The first open question, naming what goes and where (TM-sftp-01).
    pub(super) fn render_ask(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let ask = self.asks.front()?;
        let theme = cx.theme();
        let (border, background) = (theme.border, theme.popover);
        let target = &self.target;
        let (question, yes) = match ask {
            Ask::Replace(Job::Download { target, .. }) => {
                (format!("Replace {}?", target.display()), "Replace")
            }
            Ask::Replace(Job::Upload { folder, name, .. }) => (
                format!(
                    "Replace {} on {target}?",
                    clean_title(&crate::sftp::join(folder, name))
                ),
                "Replace",
            ),
            Ask::Delete { path, folder } => {
                let what = clean_title(path);
                let question = if *folder {
                    format!(
                        "Delete the folder {what} on {target}? Only an empty folder can be \
                         deleted. This can't be undone."
                    )
                } else {
                    format!("Delete {what} on {target}? This can't be undone.")
                };
                (question, "Delete")
            }
            Ask::Rename { to, .. } => (
                format!("{} exists on {target}. Replace it?", clean_title(to)),
                "Replace",
            ),
            Ask::Run(file) => (
                format!(
                    "{} is a program. Run it, or open it as text?",
                    clean_title(&file.to_string_lossy())
                ),
                "Run",
            ),
            Ask::Overwrite(id) => {
                let name = self.edit_label(*id);
                let question = format!(
                    "{name} changed on {target} since you opened it. Upload your copy over it?"
                );
                (question, "Upload anyway")
            }
            Ask::Discard(id) => {
                let name = self.edit_label(*id);
                let question =
                    format!("Close {name}? Your changes aren't on {target} and will be lost.");
                (question, "Discard")
            }
        };
        let run = matches!(ask, Ask::Run(_));
        Some(
            div()
                .key_context(ASK_CONTEXT)
                .on_action(cx.listener(|this, _: &NextAnswer, window, cx| {
                    this.step_answer(1, window, cx);
                }))
                .on_action(cx.listener(|this, _: &PreviousAnswer, window, cx| {
                    this.step_answer(-1, window, cx);
                }))
                .m(px(6.))
                .p(px(8.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .rounded(px(4.))
                .border_1()
                .border_color(border)
                .bg(background)
                .text_xs()
                .child(div().whitespace_normal().child(question))
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            Button::new("files-ask-yes")
                                .track_focus(&self.ask_buttons[0])
                                .label(yes)
                                .danger()
                                .xsmall()
                                .on_click(cx.listener(|this, _, _, cx| this.answer(true, cx))),
                        )
                        .when(run, |buttons| {
                            buttons.child(
                                Button::new("files-ask-edit")
                                    .track_focus(&self.ask_buttons[1])
                                    .label("Edit")
                                    .xsmall()
                                    .on_click(cx.listener(|this, _, _, cx| this.answer_edit(cx))),
                            )
                        })
                        .child(
                            Button::new("files-ask-no")
                                .track_focus(&self.ask_buttons[2])
                                .label("Cancel")
                                .xsmall()
                                .on_click(cx.listener(|this, _, _, cx| this.answer(false, cx))),
                        ),
                ),
        )
    }

    pub(super) fn render_naming(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let naming = self.naming.as_ref()?;
        Some(
            div()
                .key_context(NAME_CONTEXT)
                .on_action(cx.listener(|this, _: &CancelName, window, cx| {
                    this.finish_naming(None, window, cx);
                }))
                .px(px(6.))
                .py(px(4.))
                .child(Input::new(&naming.input).xsmall()),
        )
    }

    /// Which of the open question's buttons has the keyboard: 0 yes, 1
    /// Edit, 2 Cancel.
    pub(super) fn focused_answer(&self, window: &Window) -> Option<usize> {
        self.asks.front()?;
        self.ask_buttons
            .iter()
            .position(|button| button.is_focused(window))
    }

    /// Left and Right between the open question's buttons, stopping at
    /// either end.
    fn step_answer(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let run = matches!(self.asks.front(), Some(Ask::Run(_)));
        let shown: Vec<&FocusHandle> = self
            .ask_buttons
            .iter()
            .enumerate()
            .filter(|(ix, _)| run || *ix != 1)
            .map(|(_, button)| button)
            .collect();
        let Some(at) = shown.iter().position(|button| button.is_focused(window)) else {
            return;
        };
        let next = at.saturating_add_signed(step).min(shown.len() - 1);
        let target = shown[next].clone();
        window.focus(&target, cx);
    }

    /// Moves the keyboard to a question as it opens, onto its safe answer:
    /// Edit for a Run question, else the one Enter has always meant (spec
    /// 0008, flow 6). Once the questions are answered, it goes back to the
    /// tree if nothing else took it. Only while the browser has the
    /// keyboard.
    pub(super) fn follow_asks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = match self.asks.front() {
            Some(_) if self.ask_focused == Some(self.asks_answered) => return,
            Some(ask) => {
                if !self.focus.contains_focused(window, cx) {
                    return;
                }
                self.ask_focused = Some(self.asks_answered);
                let button = if matches!(ask, Ask::Run(_)) { 1 } else { 0 };
                self.ask_buttons[button].clone()
            }
            None if self.ask_focused.take().is_some() => {
                // Back to the tree, unless the answer moved the keyboard
                // elsewhere, like Run into its new pane.
                cx.defer_in(window, |this, window, cx| {
                    let in_question = this.ask_buttons.iter().any(|b| b.is_focused(window));
                    if in_question || window.focused(cx).is_none() {
                        window.focus(&this.focus, cx);
                    }
                });
                return;
            }
            None => return,
        };
        cx.defer_in(window, move |_, window, cx| window.focus(&target, cx));
    }
}
