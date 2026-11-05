//! The dialog over an SSH pane that answers one of ssh's prompts (spec
//! 0006, flow 4): a secret to type, a yes/no question, or a notice while
//! ssh waits. It is app chrome over the pane and names the connection, so
//! nothing the remote side prints can open, read or fill it (architecture
//! §Security 1).

use super::clean_prompt;
use crate::secrets::Key;
use crate::ssh::askpass::PromptKind;
use futures::channel::oneshot;
use gpui::{
    App, AppContext, ClickEvent, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyBinding, MouseButton, ParentElement, Render, SharedString, Styled,
    Subscription, Window, actions, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Sizable};
use zeroize::Zeroizing;

/// The dialog's key context.
const CONTEXT: &str = "Prompt";
/// The widest a dialog gets, in logical pixels.
const WIDTH: f32 = 460.0;

actions!(zed_term, [CancelPrompt]);

/// Binds Esc, which declines whatever the dialog asks.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", CancelPrompt, Some(CONTEXT))]);
}

/// How the user answered. Leaving the dialog any other way (Esc, Cancel,
/// No) drops its sender, which declines: the helper exits 1.
pub enum Answer {
    /// What was typed, and whether to save it in the keychain.
    Typed {
        text: Zeroizing<String>,
        save: bool,
    },
    Yes,
}

/// The keychain option of a dialog that asks for the connection's own
/// password or passphrase.
pub struct Remember {
    pub key: Key,
    /// The saved secret was filled in and failed: the option replaces it,
    /// and starts checked.
    pub replace: bool,
}

pub struct PromptDialog {
    focus: FocusHandle,
    /// Where a yes/no question starts, so Enter means No (TM-askpass-07).
    no: FocusHandle,
    connection: SharedString,
    text: SharedString,
    kind: PromptKind,
    input: Option<Entity<InputState>>,
    /// The keychain option, and whether it's checked.
    remember: Option<(Remember, bool)>,
    answer: Option<oneshot::Sender<Answer>>,
    _enter: Option<Subscription>,
}

impl PromptDialog {
    /// A dialog for `prompt` of a `kind` that ssh asks on `connection`,
    /// answered through `answer`.
    pub fn new(
        connection: SharedString,
        prompt: &str,
        kind: PromptKind,
        remember: Option<Remember>,
        answer: oneshot::Sender<Answer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = (kind == PromptKind::Secret)
            .then(|| cx.new(|cx| InputState::new(window, cx).masked(true)));
        let enter = input.as_ref().map(|input| {
            cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.submit(window, cx);
                }
            })
        });
        Self {
            focus: cx.focus_handle(),
            no: cx.focus_handle(),
            connection,
            text: clean_prompt(prompt).trim_end().to_owned().into(),
            kind,
            input,
            remember: remember.map(|remember| {
                let checked = remember.replace;
                (remember, checked)
            }),
            answer: Some(answer),
            _enter: enter,
        }
    }

    /// What takes the keyboard while the dialog is open: the text field, No,
    /// or the notice itself.
    pub fn focus_target(&self, cx: &App) -> FocusHandle {
        match (&self.input, self.kind) {
            (Some(input), _) => input.read(cx).focus_handle(cx),
            (None, PromptKind::YesNo) => self.no.clone(),
            (None, _) => self.focus.clone(),
        }
    }

    /// Hands over what was typed. The field lets go of its copy at once.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = &self.input else {
            return;
        };
        let text = Zeroizing::new(input.read(cx).text().to_string());
        input.update(cx, |input, cx| input.clean(window, cx));
        let save = self.remember.as_ref().is_some_and(|(_, checked)| *checked);
        if let Some(answer) = self.answer.take() {
            let _ = answer.send(Answer::Typed { text, save });
        }
    }

    fn yes(&mut self) {
        if let Some(answer) = self.answer.take() {
            let _ = answer.send(Answer::Yes);
        }
    }

    /// Declines what ssh asked.
    fn decline(&mut self) {
        self.answer = None;
    }

    fn buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let row = div().flex().justify_end().gap(px(8.));
        match self.kind {
            PromptKind::YesNo => row
                .child(
                    Button::new("no")
                        .label("No")
                        .small()
                        .track_focus(&self.no)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, _| this.decline())),
                )
                .child(
                    Button::new("yes")
                        .label("Yes")
                        .small()
                        .on_click(cx.listener(|this, _: &ClickEvent, _, _| this.yes())),
                ),
            _ => row
                .child(
                    Button::new("cancel")
                        .label("Cancel")
                        .small()
                        .on_click(cx.listener(|this, _: &ClickEvent, _, _| this.decline())),
                )
                .child(Button::new("ok").label("OK").primary().small().on_click(
                    cx.listener(|this, _: &ClickEvent, window, cx| this.submit(window, cx)),
                )),
        }
    }

    /// "Remember in keychain", with the warning the owner signed off on
    /// (OQ-016, TM-askpass-14).
    fn remember(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (remember, checked) = self.remember.as_ref()?;
        let password = matches!(remember.key, Key::Password(_));
        let label = match (remember.replace, password) {
            (false, _) => "Remember in keychain",
            (true, true) => "Replace the saved password",
            (true, false) => "Replace the saved passphrase",
        };
        let warning = if password {
            "Any program you run can read what's saved in the keychain. \
             Logging in with a private key is safer than a saved password."
        } else {
            "Any program you run can read what's saved in the keychain."
        };
        let muted = cx.theme().muted_foreground;
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    Checkbox::new("remember")
                        .label(label)
                        .checked(*checked)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            if let Some((_, remember)) = &mut this.remember {
                                *remember = *checked;
                            }
                            cx.notify();
                        })),
                )
                .child(div().text_xs().text_color(muted).child(warning)),
        )
    }
}

impl Render for PromptDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, background) = (theme.muted_foreground, theme.border, theme.popover);
        let failed = self
            .remember
            .as_ref()
            .filter(|(remember, _)| remember.replace)
            .map(|(remember, _)| match &remember.key {
                Key::Password(target) => format!("The password saved for {target} didn't work."),
                Key::Passphrase(file) => format!("The passphrase saved for {file} didn't work."),
            });
        let card = div()
            .w(px(WIDTH))
            .max_w_full()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(14.))
            .rounded(px(8.))
            .border_1()
            .border_color(border)
            .bg(background)
            .shadow_lg()
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("ssh · {}", self.connection)),
            )
            .child(div().text_sm().child(self.text.clone()))
            .children(failed.map(|failed| div().text_xs().text_color(muted).child(failed)))
            .children(
                self.input
                    .as_ref()
                    .map(|input| Input::new(input).mask_toggle()),
            )
            .children(self.remember(cx))
            .children((self.kind != PromptKind::Notice).then(|| self.buttons(cx)));
        // Over the whole pane: clicking beside the card keeps the keyboard in
        // it, since the grid takes no input while ssh waits.
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(32.))
            .px(px(12.))
            .bg(gpui::black().opacity(0.3))
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &CancelPrompt, _, _| this.decline()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.focus_target(cx), cx)),
            )
            .child(card)
    }
}
