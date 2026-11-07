//! A dialog's answers, usable by keyboard as well as by mouse (owner's
//! rule): each is a button with its own focus. The dialog binds Enter to
//! its own confirm, which never reaches a focused button, so its confirm
//! here presses the button that has the keyboard, or the safe answer if
//! none has. Left and Right move between the buttons, Tab too; Esc closes
//! the dialog, which is the safe answer's job anyway.

use gpui::{
    App, FocusHandle, InteractiveElement, KeyBinding, ParentElement, SharedString, Styled, Window,
    actions, div, px,
};
use gpui_component::Sizable;
use gpui_component::WindowExt;
use gpui_component::button::Button;
use gpui_component::dialog::Dialog;
use std::rc::Rc;

/// The key context of a dialog's answers.
const CONTEXT: &str = "DialogAnswers";

actions!(tern, [NextDialogAnswer, PreviousDialogAnswer]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("right", NextDialogAnswer, Some(CONTEXT)),
        KeyBinding::new("left", PreviousDialogAnswer, Some(CONTEXT)),
    ]);
}

/// What an answer does, after the dialog closed.
pub type Act = Rc<dyn Fn(&mut Window, &mut App)>;

/// One answer: its label, its look, and what it does.
pub struct Answer {
    pub label: SharedString,
    pub style: fn(Button) -> Button,
    pub act: Act,
}

/// A dialog's answers, in the order they're shown, and the one Enter
/// means while no button has the keyboard.
#[derive(Clone)]
pub struct Answers {
    answers: Rc<Vec<(Answer, FocusHandle)>>,
    safe: usize,
}

impl Answers {
    pub fn new(answers: Vec<Answer>, safe: usize, cx: &mut App) -> Self {
        let answers = answers
            .into_iter()
            .map(|answer| (answer, cx.focus_handle()))
            .collect();
        Self {
            answers: Rc::new(answers),
            safe,
        }
    }

    /// Puts the safe answer's button under the keyboard once the dialog
    /// is on screen.
    pub fn focus_safe(&self, window: &mut Window) {
        if let Some((_, focus)) = self.answers.get(self.safe) {
            let focus = focus.clone();
            window.on_next_frame(move |window, cx| focus.focus(window, cx));
        }
    }

    /// The answer whose button has the keyboard.
    fn focused(&self, window: &Window) -> Option<usize> {
        self.answers
            .iter()
            .position(|(_, focus)| focus.is_focused(window))
    }

    /// Moves the keyboard `step` buttons on, stopping at either end.
    fn step(&self, step: isize, window: &mut Window, cx: &mut App) {
        let last = self.answers.len().saturating_sub(1);
        let next = match self.focused(window) {
            Some(at) => at.saturating_add_signed(step).min(last),
            None => self.safe,
        };
        if let Some((_, focus)) = self.answers.get(next) {
            focus.focus(window, cx);
        }
    }

    /// `dialog` with these answers as its footer, and its confirm pressing
    /// the focused one.
    pub fn apply(&self, dialog: Dialog) -> Dialog {
        let confirm = self.clone();
        let buttons = self
            .answers
            .iter()
            .enumerate()
            .map(|(ix, (answer, focus))| {
                let act = Rc::clone(&answer.act);
                (answer.style)(Button::new(("dialog-answer", ix)))
                    .track_focus(focus)
                    .label(answer.label.clone())
                    .small()
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        act(window, cx);
                    })
            });
        let (next, previous) = (self.clone(), self.clone());
        dialog
            .on_ok(move |_, window, cx| {
                let ix = confirm.focused(window).unwrap_or(confirm.safe);
                if let Some((answer, _)) = confirm.answers.get(ix) {
                    (answer.act)(window, cx);
                }
                true
            })
            .footer(
                div()
                    .key_context(CONTEXT)
                    .on_action(move |_: &NextDialogAnswer, window, cx| next.step(1, window, cx))
                    .on_action(move |_: &PreviousDialogAnswer, window, cx| {
                        previous.step(-1, window, cx)
                    })
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .children(buttons),
            )
    }
}
