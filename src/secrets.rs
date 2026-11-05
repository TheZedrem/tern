//! The OS keychain (spec 0006): Secret Service on Linux, Credential Manager
//! on Windows, through `keyring`. It holds only what the user chose to
//! remember, under the service `zed-term`. Every call may block, on an
//! unlock prompt or a hung D-Bus, so callers run it through [`off_thread`],
//! never on the UI thread (TM-askpass-17).

use futures::channel::oneshot;
use zeroize::Zeroizing;

/// The service every keychain item of zed-term is saved under.
const SERVICE: &str = "zed-term";

/// What a secret is saved under (spec 0006 §Model): a password by the
/// target ssh logs in to, so a profile and an alias of the same server
/// share it, and a passphrase by its key file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// `user@host:port`.
    Password(String),
    /// The key file, as ssh names it in its prompt.
    Passphrase(String),
}

impl Key {
    /// The keychain item's account, which keychain apps show next to the
    /// service.
    fn account(&self) -> String {
        match self {
            Key::Password(target) => format!("password:{target}"),
            Key::Passphrase(file) => format!("passphrase:{file}"),
        }
    }

    /// How notices name the secret, which they never show.
    pub fn describe(&self) -> String {
        match self {
            Key::Password(target) => format!("the password for {target}"),
            Key::Passphrase(file) => format!("the passphrase of {file}"),
        }
    }
}

/// There is no keychain to use: no Secret Service is running (spec 0006,
/// flow 8).
#[derive(Debug, PartialEq, Eq)]
pub struct Unavailable;

/// What a notice says when there is no keychain.
pub const UNAVAILABLE: &str = "There is no keychain to save passwords in (no Secret Service is running). Connecting still works.";

/// Why the keychain gave no secret, besides having none saved.
#[derive(Debug, PartialEq, Eq)]
pub enum GetError {
    Unavailable,
    /// It couldn't answer, say a locked one whose unlock was dismissed.
    Failed(String),
}

/// The secret saved under `key`, `None` if there is none.
pub fn get(key: &Key) -> Result<Option<Zeroizing<String>>, GetError> {
    let entry = entry(key).map_err(|Unavailable| GetError::Unavailable)?;
    match entry.get_password() {
        Ok(secret) => Ok(Some(Zeroizing::new(secret))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(GetError::Failed(error.to_string())),
    }
}

/// Saves `secret` under `key`, replacing what was there.
pub fn set(key: &Key, secret: &str) -> Result<(), String> {
    entry(key)
        .map_err(|_| UNAVAILABLE.to_owned())?
        .set_password(secret)
        .map_err(|error| error.to_string())
}

/// Forgets what's saved under `key`: whether there was anything.
pub fn delete(key: &Key) -> Result<bool, String> {
    let entry = entry(key).map_err(|_| UNAVAILABLE.to_owned())?;
    match entry.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

/// The keychain item of `key`. Failing means no keychain: `keyring` sets up
/// its store at the first call, and only that can fail here.
fn entry(key: &Key) -> Result<keyring::Entry, Unavailable> {
    keyring::Entry::new(SERVICE, &key.account()).map_err(|_| Unavailable)
}

/// Runs `job` on a thread of its own, so a call that hangs holds neither
/// the UI nor an executor thread: a keychain call waiting on an unlock
/// prompt or a hung D-Bus, or `ssh -G` on a `Match exec` that never returns.
/// The receiver fails if the thread couldn't start.
pub fn off_thread<T: Send + 'static>(
    job: impl FnOnce() -> T + Send + 'static,
) -> oneshot::Receiver<T> {
    let (sender, receiver) = oneshot::channel();
    let _ = std::thread::Builder::new()
        .name("keychain".into())
        .spawn(move || {
            let _ = sender.send(job());
        });
    receiver
}

// No test here calls `get`, `set` or `delete`: they reach the user's real
// keychain.
#[cfg(test)]
mod tests {
    use super::*;

    /// Spec 0006 §Model: passwords are saved by target and passphrases by
    /// key file, as two kinds of item that never meet, even under the same
    /// name. Keychain apps show the account, so it names what it holds.
    #[test]
    fn a_password_and_a_passphrase_under_the_same_name_are_different_items() {
        let name = "alice@prod.example.com:22";
        let (password, passphrase) = (Key::Password(name.into()), Key::Passphrase(name.into()));
        assert_ne!(password.account(), passphrase.account());
        for key in [password, passphrase] {
            assert!(key.account().contains(name), "{}", key.account());
        }
    }

    /// Spec 0006, flows 4 and 7: notices name the secret by what it opens,
    /// the target of a password or the file of a passphrase.
    #[test]
    fn a_secret_is_described_by_its_target_or_its_key_file() {
        let password = Key::Password("alice@prod.example.com:22".into()).describe();
        assert!(
            password.contains("password for alice@prod.example.com:22"),
            "{password}"
        );
        let passphrase = Key::Passphrase("/home/alice/.ssh/id_ed25519".into()).describe();
        assert!(
            passphrase.contains("passphrase of /home/alice/.ssh/id_ed25519"),
            "{passphrase}"
        );
    }

    /// TM-askpass-17: a keychain call that hangs holds up no caller. The
    /// job runs on a thread of its own, the caller gets its receiver at
    /// once, and the answer comes when the job is done.
    #[test]
    fn a_job_off_thread_holds_up_no_caller_and_answers_when_done() {
        let (release, released) = std::sync::mpsc::channel::<()>();
        let caller = std::thread::current().id();
        let mut answer = off_thread(move || {
            let _ = released.recv();
            std::thread::current().id()
        });
        assert_eq!(
            answer.try_recv(),
            Ok(None),
            "answered before the job was done"
        );
        release.send(()).expect("let the job finish");
        let ran_on = futures::executor::block_on(answer).expect("an answer");
        assert_ne!(ran_on, caller);
    }
}
