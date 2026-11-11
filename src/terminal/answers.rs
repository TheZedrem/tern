//! Answering ssh's prompts (spec 0006, flow 4): from the keychain where
//! it holds the secret, else in a dialog over the pane.

use super::prompt::{Answer, PromptDialog, Remember};
use super::{Asker, PaneEvent, TerminalPane};
use crate::secrets::{self, Key};
use crate::ssh::askpass::{Prompt, PromptKind};
use crate::ssh::target::Target;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::oneshot;
use futures::future::{Either, select};
use gpui::{AppContext, BackgroundExecutor, Context, SharedString, Task, Window};
use std::collections::HashSet;
use std::pin::pin;
use std::rc::Rc;
use std::time::Duration;
use zeroize::Zeroizing;

/// How long a keychain call may take, an unlock prompt included, before it
/// counts as nothing saved (TM-askpass-17).
const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `ssh -G` may take before prompts are answered without it, so
/// nothing is filled in.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

impl TerminalPane {
    /// Opens the dialog for `prompt` over the pane. The receiver gets the
    /// answer, or fails when the dialog is left without one.
    fn open_dialog(
        &mut self,
        prompt: &Prompt,
        remember: Option<Remember>,
        asker: Asker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Answer> {
        let (answer, answered) = oneshot::channel();
        let connection = match asker {
            Asker::Terminal => SharedString::from(self.profile.name.clone()),
            Asker::Sftp => format!("{} (SFTP)", self.profile.name).into(),
            Asker::Forward(id) => {
                let summary = self.forward_askpass.get(&id).map(|(summary, _)| summary);
                let summary = summary.map_or("", |summary| summary.as_ref());
                format!("{} (port forward {summary})", self.profile.name).into()
            }
        };
        let dialog = cx.new(|cx| {
            PromptDialog::new(
                connection,
                &prompt.text,
                prompt.kind,
                remember,
                answer,
                window,
                cx,
            )
        });
        self.dialog = Some((asker, dialog));
        if let Asker::Forward(id) = asker {
            cx.emit(PaneEvent::ForwardPrompt { id, open: true });
        }
        cx.emit(PaneEvent::DialogChanged);
        cx.notify();
        answered
    }

    /// Closes the dialog if it's `asker`'s.
    pub(super) fn close_dialog(&mut self, asker: Asker, cx: &mut Context<Self>) {
        if self
            .dialog
            .as_ref()
            .is_some_and(|(owner, _)| *owner == asker)
        {
            self.dialog = None;
            if let Asker::Forward(id) = asker {
                cx.emit(PaneEvent::ForwardPrompt { id, open: false });
            }
            cx.emit(PaneEvent::DialogChanged);
            cx.notify();
        }
    }

    /// Ends the terminal connection's prompts; helpers still waiting exit
    /// 1. The SFTP connection's have their own token and go on.
    pub(super) fn end_prompts(&mut self, cx: &mut Context<Self>) {
        self.askpass = None;
        self.close_dialog(Asker::Terminal, cx);
    }
}

impl PromptHost for TerminalPane {
    type Asker = Asker;

    fn open_prompt(
        &mut self,
        prompt: &Prompt,
        remember: Option<Remember>,
        asker: Asker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Answer> {
        self.open_dialog(prompt, remember, asker, window, cx)
    }

    fn close_prompt(&mut self, asker: Asker, cx: &mut Context<Self>) {
        self.close_dialog(asker, cx);
    }

    fn prompt_notice(&mut self, notice: PromptNotice, cx: &mut Context<Self>) {
        cx.emit(match notice {
            PromptNotice::Say(notice) => PaneEvent::Notice(notice),
            PromptNotice::NoKeychain => PaneEvent::NoKeychain,
        });
    }

    fn prompt_declined(&mut self, asker: Asker, cx: &mut Context<Self>) {
        if let Asker::Forward(id) = asker {
            cx.emit(PaneEvent::ForwardDeclined(id));
        }
    }
}

/// A view that answers its ssh connections' prompts in a dialog of its
/// own (spec 0006, flow 4): a terminal pane, or a files pane for its SFTP
/// sessions (spec 0013).
pub trait PromptHost: Sized + 'static {
    /// Whose connection a prompt is, when several take turns at the one
    /// dialog.
    type Asker: Copy + PartialEq + 'static;

    /// Opens the dialog for `prompt`. The receiver gets the answer, or
    /// fails when the dialog is left without one.
    fn open_prompt(
        &mut self,
        prompt: &Prompt,
        remember: Option<Remember>,
        asker: Self::Asker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Answer>;

    /// Closes the dialog if it's `asker`'s.
    fn close_prompt(&mut self, asker: Self::Asker, cx: &mut Context<Self>);

    /// Says something about the prompts, as a notice.
    fn prompt_notice(&mut self, notice: PromptNotice, cx: &mut Context<Self>);

    /// The user left `asker`'s dialog without an answer (Esc, Cancel, No).
    fn prompt_declined(&mut self, _asker: Self::Asker, _cx: &mut Context<Self>) {}
}

/// What answering prompts has to say.
pub enum PromptNotice {
    Say(String),
    /// There is no keychain; said once per window (spec 0006, flow 8).
    NoKeychain,
}

/// Answers the prompts of the connection whose ssh runs with `args`, one
/// at a time (spec 0006, flow 4). The connection's own password and
/// passphrase prompts are filled in from the keychain once each, with a
/// notice; everything else gets `host`'s dialog when it's `asker`'s
/// `turn`. Dropping the task ends the prompts.
pub fn answer_prompts<H: PromptHost>(
    mut prompts: UnboundedReceiver<Prompt>,
    args: Vec<String>,
    asker: H::Asker,
    turn: Rc<futures::lock::Mutex<()>>,
    window: &mut Window,
    cx: &mut Context<H>,
) -> Task<()> {
    let executor = cx.background_executor().clone();
    let target = secrets::off_thread(move || Target::resolve(&args));
    cx.spawn_in(window, async move |this, cx| {
        let target = within(target, RESOLVE_TIMEOUT, &executor)
            .await
            .and_then(Result::ok)
            .flatten();
        let mut offers = Offers::default();
        let say = |notice| move |host: &mut H, cx: &mut Context<H>| host.prompt_notice(notice, cx);
        while let Some(mut prompt) = prompts.next().await {
            let key = target
                .as_ref()
                .filter(|_| prompt.kind == PromptKind::Secret)
                .and_then(|target| target.secret_for(&prompt.text));
            if let Some(key) = key.as_ref().filter(|key| offers.ask(key)) {
                match lookup(key, &executor).await {
                    Lookup::Found(secret) => {
                        prompt.answer(&secret);
                        offers.filled.insert(key.clone());
                        let notice = PromptNotice::Say(filled_notice(key));
                        let _ = this.update(cx, say(notice));
                        continue;
                    }
                    Lookup::Nothing => {}
                    Lookup::Failed(error) => {
                        let notice = format!("Couldn't read the keychain: {error}");
                        let _ = this.update(cx, say(PromptNotice::Say(notice)));
                    }
                    Lookup::NoKeychain => {
                        offers.no_keychain = true;
                        let _ = this.update(cx, say(PromptNotice::NoKeychain));
                    }
                }
            }
            let remember = offers.remember(key.clone());
            let _turn = turn.lock().await;
            let Ok(answered) = this.update_in(cx, |host, window, cx| {
                host.open_prompt(&prompt, remember, asker, window, cx)
            }) else {
                break;
            };
            let (answer, declined) = match select(answered, pin!(prompt.ended())).await {
                Either::Left((Ok(answer), _)) => (Some(answer), false),
                Either::Left((Err(_), _)) => (None, true),
                Either::Right(_) => (None, false),
            };
            let _ = this.update(cx, |host, cx| {
                host.close_prompt(asker, cx);
                if declined {
                    host.prompt_declined(asker, cx);
                }
            });
            match answer {
                Some(Answer::Typed { text, save }) => {
                    prompt.answer(&text);
                    if let Some(key) = key.filter(|_| save) {
                        let _ = this.update(cx, |_, cx| save_secret(key, text, cx));
                    }
                }
                Some(Answer::Yes) => prompt.answer("yes"),
                // Declined, or ssh is done with it: dropping it ends the
                // helper.
                None => {}
            }
        }
    })
}

/// Saves what was typed under `key`, off the UI thread. Only a failure
/// makes a notice.
fn save_secret<H: PromptHost>(key: Key, secret: Zeroizing<String>, cx: &mut Context<H>) {
    let executor = cx.background_executor().clone();
    let described = key.describe();
    let saving = secrets::off_thread(move || secrets::set(&key, &secret));
    cx.spawn(async move |this, cx| {
        let error = match within(saving, KEYCHAIN_TIMEOUT, &executor).await {
            Some(Ok(Ok(()))) => return,
            Some(Ok(Err(error))) => error,
            _ => "the keychain didn't answer".to_owned(),
        };
        let notice = PromptNotice::Say(format!("Couldn't save {described}: {error}"));
        let _ = this.update(cx, |host, cx| host.prompt_notice(notice, cx));
    })
    .detach();
}

/// What a connection's prompts got from the keychain so far (spec 0006,
/// flow 4).
#[derive(Default)]
struct Offers {
    /// Asked for once each: a second prompt for one gets a dialog, never
    /// another fill, so a wrong saved password can't loop (AC5).
    asked: HashSet<Key>,
    /// Filled in from the keychain. A second prompt for one means the saved
    /// secret failed.
    filled: HashSet<Key>,
    /// There is no keychain: nothing is asked or offered to remember.
    no_keychain: bool,
}

impl Offers {
    /// Whether to ask the keychain for `key` now: once per connection, and
    /// only with a keychain.
    fn ask(&mut self, key: &Key) -> bool {
        !self.no_keychain && self.asked.insert(key.clone())
    }

    /// The keychain option of the dialog for a prompt answered by `key`, if
    /// any: after the saved secret failed, it offers to replace it.
    fn remember(&self, key: Option<Key>) -> Option<Remember> {
        let key = key.filter(|_| !self.no_keychain)?;
        Some(Remember {
            replace: self.filled.contains(&key),
            key,
        })
    }
}

/// What the keychain holds for a prompt.
enum Lookup {
    Found(Zeroizing<String>),
    Nothing,
    NoKeychain,
    /// It couldn't answer: asked like nothing saved, with this notice.
    Failed(String),
}

/// Asks the keychain for `key` on a thread of its own. No answer within
/// [`KEYCHAIN_TIMEOUT`] counts as nothing saved (TM-askpass-17), with a
/// notice saying so.
async fn lookup(key: &Key, executor: &BackgroundExecutor) -> Lookup {
    let key = key.clone();
    let found = secrets::off_thread(move || secrets::get(&key));
    match within(found, KEYCHAIN_TIMEOUT, executor).await {
        Some(Ok(Ok(Some(secret)))) => Lookup::Found(secret),
        Some(Ok(Ok(None))) => Lookup::Nothing,
        Some(Ok(Err(secrets::GetError::Unavailable))) => Lookup::NoKeychain,
        Some(Ok(Err(secrets::GetError::Failed(error)))) => Lookup::Failed(error),
        Some(Err(_)) => Lookup::Failed("the keychain lookup couldn't start".into()),
        None => Lookup::Failed("the keychain didn't answer".into()),
    }
}

/// `future`'s output, unless it takes longer than `limit`.
async fn within<T>(
    future: impl Future<Output = T>,
    limit: Duration,
    executor: &BackgroundExecutor,
) -> Option<T> {
    match select(pin!(future), executor.timer(limit)).await {
        Either::Left((output, _)) => Some(output),
        Either::Right(_) => None,
    }
}

/// The notice when the keychain answered a prompt: it names what it used,
/// never the secret (TM-askpass-10). It can't say the secret worked: a
/// wrong one gets the replace dialog right after.
fn filled_notice(key: &Key) -> String {
    match key {
        Key::Password(target) => format!("Used the password saved for {target}"),
        Key::Passphrase(file) => format!("Used the passphrase saved for {file}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TM-askpass-10: each fill from the keychain says what it used, in the
    /// words of spec 0006 for a password. It names, and has, no secret.
    #[test]
    fn a_fill_notice_names_the_target_or_key_file_it_used() {
        assert_eq!(
            filled_notice(&Key::Password("alice@prod.example.com:22".into())),
            "Used the password saved for alice@prod.example.com:22"
        );
        let file = "/home/alice/.ssh/id_ed25519";
        let notice = filled_notice(&Key::Passphrase(file.into()));
        assert!(
            notice.contains(file) && notice.contains("passphrase"),
            "{notice}"
        );
    }

    /// The connection's password and its key's passphrase, as
    /// `Target::secret_for` names them.
    fn password() -> Key {
        Key::Password("alice@prod.example.com:22".into())
    }

    fn passphrase() -> Key {
        Key::Passphrase("/home/alice/.ssh/id_ed25519".into())
    }

    /// What the keychain of an `Offers` test holds for any key it's asked.
    #[derive(Clone, Copy)]
    enum Keychain {
        Saved,
        Empty,
        /// There is none: no Secret Service is running.
        Missing,
    }

    /// What became of a prompt.
    #[derive(Debug, PartialEq)]
    enum Outcome {
        /// Filled in from the keychain, with no dialog.
        Filled,
        /// A dialog. `asked`: whether the keychain was asked first.
        /// `remember`: its keychain option, as the key and whether it
        /// replaces what's saved.
        Dialog {
            asked: bool,
            remember: Option<(Key, bool)>,
        },
    }

    /// A prompt for `key` (`None`: no secret of the connection's own) taken
    /// through `offers` as `answer_prompts` takes it: the keychain is asked
    /// only when `offers` says so, what it has fills the prompt, and every
    /// prompt not filled gets a dialog.
    fn take(offers: &mut Offers, key: Option<&Key>, keychain: Keychain) -> Outcome {
        let asked = key.filter(|key| offers.ask(key));
        match (asked, keychain) {
            (Some(key), Keychain::Saved) => {
                offers.filled.insert(key.clone());
                return Outcome::Filled;
            }
            (Some(_), Keychain::Missing) => offers.no_keychain = true,
            _ => {}
        }
        let remember = offers.remember(key.cloned());
        Outcome::Dialog {
            asked: asked.is_some(),
            remember: remember.map(|remember| (remember.key, remember.replace)),
        }
    }

    /// AC5 (spec 0006, flow 4): the saved password is filled in once. When
    /// it was wrong, ssh asks again, and that prompt gets a dialog instead
    /// of the keychain, so there is no retry loop, and the dialog offers to
    /// replace what's saved. The key's passphrase is a secret of its own,
    /// still asked for once.
    #[test]
    fn a_wrong_saved_password_is_filled_once_then_its_dialog_offers_to_replace_it() {
        let mut offers = Offers::default();
        let filled = take(&mut offers, Some(&password()), Keychain::Saved);
        assert_eq!(filled, Outcome::Filled);
        let replace = Outcome::Dialog {
            asked: false,
            remember: Some((password(), true)),
        };
        // ssh asks up to `NumberOfPasswordPrompts` (3) times.
        for attempt in 2..=3 {
            let again = take(&mut offers, Some(&password()), Keychain::Saved);
            assert_eq!(again, replace, "attempt {attempt}");
        }
        let unlocked = take(&mut offers, Some(&passphrase()), Keychain::Saved);
        assert_eq!(unlocked, Outcome::Filled);
    }

    /// Spec 0006, flow 4: with nothing saved, the connection's own password
    /// prompt gets a dialog offering "Remember in keychain", a first save
    /// rather than a replacement, and so does ssh's next prompt for it,
    /// without asking the keychain again.
    #[test]
    fn with_nothing_saved_the_dialog_offers_to_remember_the_password() {
        let mut offers = Offers::default();
        let first = take(&mut offers, Some(&password()), Keychain::Empty);
        let remember = Some((password(), false));
        assert_eq!(
            first,
            Outcome::Dialog {
                asked: true,
                remember: remember.clone()
            }
        );
        let again = take(&mut offers, Some(&password()), Keychain::Empty);
        assert_eq!(
            again,
            Outcome::Dialog {
                asked: false,
                remember
            }
        );
    }

    /// Spec 0006, flow 4: a prompt that asks for no secret of the
    /// connection's own, like a one-time code or a jump host's password,
    /// gets a dialog without the keychain being asked, and nothing is
    /// offered to remember.
    #[test]
    fn a_prompt_for_no_secret_of_the_connections_own_gets_a_dialog_with_nothing_to_remember() {
        let mut offers = Offers::default();
        let outcome = take(&mut offers, None, Keychain::Saved);
        let bare = Outcome::Dialog {
            asked: false,
            remember: None,
        };
        assert_eq!(outcome, bare);
    }

    /// Spec 0006, flow 8: once a lookup finds no keychain, nothing more is
    /// asked of it, and no dialog offers to remember: there's nowhere to
    /// save. Connecting still works, through the dialogs.
    #[test]
    fn without_a_keychain_nothing_more_is_asked_and_nothing_is_offered_to_remember() {
        let mut offers = Offers::default();
        let first = take(&mut offers, Some(&password()), Keychain::Missing);
        assert_eq!(
            first,
            Outcome::Dialog {
                asked: true,
                remember: None
            }
        );
        let bare = Outcome::Dialog {
            asked: false,
            remember: None,
        };
        for key in [passphrase(), password()] {
            let outcome = take(&mut offers, Some(&key), Keychain::Missing);
            assert_eq!(outcome, bare, "{key:?}");
        }
    }
}
