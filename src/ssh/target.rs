//! What ssh resolves a connection to (spec 0006 §Model, Target): the user,
//! host and port it writes into its prompts, and its key files. Read from
//! `ssh -G` only when the user connects to a host or forgets its password,
//! never when listing hosts: `Match exec` lines in `~/.ssh/config` run
//! commands (TM-askpass-20).

use super::PROGRAM;
use crate::secrets::Key;
use std::path::Path;
use std::process::{Command, Stdio};

/// Where ssh cuts a key path in its passphrase prompt (`%.100s`). A path
/// in a prompt this long may be a longer one cut short, so only shorter
/// ones are filled in.
const MAX_PROMPT_PATH: usize = 100;

/// Who and where ssh logs in, as its prompts name them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub user: String,
    /// `hostkeyalias` if set, else `hostname`.
    pub host: String,
    pub port: u16,
    /// The key files ssh may ask a passphrase for, as it opens them. One
    /// whose name needs an expansion Tern doesn't do is left out.
    pub identity_files: Vec<String>,
}

impl Target {
    /// What `ssh -G` says the connection with these arguments resolves to.
    /// `None` if it can't say; then nothing is filled in automatically.
    pub fn resolve(args: &[String]) -> Option<Self> {
        let output = Command::new(PROGRAM)
            .arg("-G")
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let config = String::from_utf8(output.stdout)
            .ok()
            .filter(|_| output.status.success())?;
        Self::parse(&config, &dirs::home_dir()?)
    }

    /// The target in `ssh -G`'s output, where `home` is what `~` and `%d`
    /// stand for.
    pub fn parse(config: &str, home: &Path) -> Option<Self> {
        let (mut user, mut hostname, mut alias, mut port) = (None, None, None, None);
        let mut files = Vec::new();
        for line in config.lines() {
            let Some((keyword, value)) = line.split_once(' ') else {
                continue;
            };
            match keyword {
                "user" => user = Some(value),
                "hostname" => hostname = Some(value),
                "hostkeyalias" => alias = Some(value),
                "port" => port = value.parse().ok(),
                "identityfile" => files.push(value),
                _ => {}
            }
        }
        let (user, hostname, port) = (user?, hostname?, port?);
        let identity_files = files
            .into_iter()
            .filter_map(|file| expand(file, home.to_str()?, user, hostname, port))
            .collect();
        Some(Self {
            user: user.to_owned(),
            host: alias.unwrap_or(hostname).to_owned(),
            port,
            identity_files,
        })
    }

    /// The saved secret that may answer `prompt`: the connection's password
    /// or the passphrase of one of its keys. The whole prompt must be one
    /// ssh writes for them; a server's prompt names the server's target, and
    /// a jump host's names the jump host (TM-askpass-04, -19).
    pub fn secret_for(&self, prompt: &str) -> Option<Key> {
        let account = format!("{}@{}", self.user, self.host);
        let keyboard_interactive = prompt
            .strip_prefix(&format!("({account}) "))
            .is_some_and(|server| server.trim_matches(' ').eq_ignore_ascii_case("password:"));
        if keyboard_interactive || prompt == format!("{account}'s password: ") {
            return Some(self.password());
        }
        let file = prompt
            .strip_prefix("Enter passphrase for key '")?
            .strip_suffix("': ")?;
        self.passphrases()
            .find(|key| *key == Key::Passphrase(file.to_owned()))
    }

    /// Everything that may be saved for the connection: its password, then
    /// its keys' passphrases.
    pub fn secrets(&self) -> Vec<Key> {
        std::iter::once(self.password())
            .chain(self.passphrases())
            .collect()
    }

    fn password(&self) -> Key {
        Key::Password(format!("{}@{}:{}", self.user, self.host, self.port))
    }

    fn passphrases(&self) -> impl Iterator<Item = Key> + '_ {
        self.identity_files
            .iter()
            .filter(|file| file.len() < MAX_PROMPT_PATH)
            .map(|file| Key::Passphrase(file.clone()))
    }
}

/// Key file `file` as ssh opens it: a leading `~` and the tokens `%d`
/// (home), `%h` (host name), `%r` (user), `%p` (port) and `%%` expanded.
/// `None` for anything else ssh would expand, like `~other` or `${VAR}`.
fn expand(file: &str, home: &str, user: &str, hostname: &str, port: u16) -> Option<String> {
    let file = match file.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        Some(_) => return None,
        None => file.to_owned(),
    };
    if file.contains("${") {
        return None;
    }
    let mut expanded = String::with_capacity(file.len());
    let mut chars = file.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            expanded.push(c);
            continue;
        }
        match chars.next()? {
            '%' => expanded.push('%'),
            'd' => expanded.push_str(home),
            'h' => expanded.push_str(hostname),
            'r' => expanded.push_str(user),
            'p' => expanded.push_str(&port.to_string()),
            _ => return None,
        }
    }
    Some(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `~` and `%d` stand for in these tests.
    const HOME: &str = "/home/alice";

    /// Some of what `ssh -G prod` prints for `Host prod` with `HostName
    /// 192.0.2.10`, `User alice`, `Port 2200`, `HostKeyAlias prod-key` and
    /// two key files, shuffled: ssh prints some 90 lines in an order of its
    /// own, which nothing may rely on.
    const PROD: &str = concat!(
        "identityfile ~/.ssh/id_ed25519\n",
        "addressfamily any\n",
        "port 2200\n",
        "hostkeyalias prod-key\n",
        "userknownhostsfile /home/alice/.ssh/known_hosts /home/alice/.ssh/known_hosts2\n",
        "host prod\n",
        "identityfile ~/.ssh/id_rsa\n",
        "hostname 192.0.2.10\n",
        "ipqos ef cs0\n",
        "user alice\n",
        "stricthostkeychecking ask\n",
    );

    /// `ssh -G` output parsed with [`HOME`] as the home directory.
    fn parse(config: &str) -> Option<Target> {
        Target::parse(config, Path::new(HOME))
    }

    /// `config` without its `keyword` lines.
    fn without(config: &str, keyword: &str) -> String {
        config
            .lines()
            .filter(|line| line.split(' ').next() != Some(keyword))
            .map(|line| format!("{line}\n"))
            .collect()
    }

    /// The key files of `PROD` with these `identityfile` lines instead.
    fn identity_files(files: &[&str]) -> Vec<String> {
        let lines: String = files
            .iter()
            .map(|file| format!("identityfile {file}\n"))
            .collect();
        let config = without(PROD, "identityfile") + &lines;
        parse(&config).expect("a target").identity_files
    }

    /// alice on prod.example.com, port 22, with two key files.
    fn prod() -> Target {
        Target {
            user: "alice".into(),
            host: "prod.example.com".into(),
            port: 22,
            identity_files: vec![
                "/home/alice/.ssh/id_ed25519".into(),
                "/home/alice/.ssh/id_rsa".into(),
            ],
        }
    }

    /// The password `prod()` is saved under.
    fn prods_password() -> Option<Key> {
        Some(Key::Password("alice@prod.example.com:22".into()))
    }

    /// Spec 0006 §Model: the target is user, host and port as ssh resolves
    /// them, and the host is `hostkeyalias` if set, else `hostname`, the
    /// name ssh writes into its prompts.
    #[test]
    fn a_target_is_user_host_and_port_from_ssh_g_with_hostkeyalias_over_hostname() {
        let expected = Target {
            user: "alice".into(),
            host: "prod-key".into(),
            port: 2200,
            identity_files: vec![
                "/home/alice/.ssh/id_ed25519".into(),
                "/home/alice/.ssh/id_rsa".into(),
            ],
        };
        assert_eq!(parse(PROD), Some(expected));
        let host = parse(&without(PROD, "hostkeyalias")).map(|target| target.host);
        assert_eq!(host.as_deref(), Some("192.0.2.10"));
    }

    /// The key files as ssh opens them, so its passphrase prompts can be
    /// told: `~` and `%d` are the home directory, `%h` the host name (not
    /// the alias, nor the `HostKeyAlias`), `%r` the user, `%p` the port and
    /// `%%` a `%`. A path with a space stays whole.
    #[test]
    fn key_files_are_expanded_as_ssh_opens_them() {
        let files = identity_files(&[
            "~/.ssh/%h_%r_%p",
            "%d/keys/100%%",
            "/home/alice/my keys/id_ed25519",
        ]);
        let expected = [
            "/home/alice/.ssh/192.0.2.10_alice_2200",
            "/home/alice/keys/100%",
            "/home/alice/my keys/id_ed25519",
        ];
        assert_eq!(files, expected);
    }

    /// A key file whose name needs an expansion Tern doesn't do is left
    /// out, so its passphrase is never filled in, rather than a wrong
    /// guess at what ssh opens.
    #[test]
    fn key_files_with_an_expansion_tern_doesnt_do_are_left_out() {
        let files = identity_files(&[
            "~/.ssh/%C",
            "~/.ssh/id_%l",
            "~bob/.ssh/id_ed25519",
            "${HOME}/.ssh/id_ed25519",
            "~/.ssh/id_ed25519",
        ]);
        assert_eq!(files, ["/home/alice/.ssh/id_ed25519"]);
    }

    /// Without the user, host name or port, ssh's prompts can't be told,
    /// and nothing is filled in.
    #[test]
    fn without_a_user_a_hostname_or_a_port_there_is_no_target() {
        for keyword in ["user", "hostname", "port"] {
            assert_eq!(parse(&without(PROD, keyword)), None, "without {keyword}");
        }
        let unparsable = without(PROD, "port") + "port ssh\n";
        assert_eq!(parse(&unparsable), None, "port ssh");
        assert_eq!(parse(""), None, "nothing");
    }

    /// Spec 0006 §Model: passwords are saved under `user@host:port`, so a
    /// profile and an `~/.ssh/config` alias for the same server share one,
    /// whatever else differs between them.
    #[test]
    fn a_profile_and_an_alias_for_the_same_server_share_its_saved_password() {
        // `ssh -G -l alice prod.example.com` and `ssh -G prod`.
        let profile = concat!(
            "host prod.example.com\n",
            "user alice\n",
            "hostname prod.example.com\n",
            "port 22\n",
            "identityfile ~/.ssh/id_rsa\n",
            "identityfile ~/.ssh/id_ed25519\n",
        );
        let alias = concat!(
            "host prod\n",
            "user alice\n",
            "hostname prod.example.com\n",
            "port 22\n",
            "identityfile ~/.ssh/prod_key\n",
        );
        let password = |config: &str| parse(config).expect("a target").secrets()[0].clone();
        assert_eq!(password(profile), password(alias));
        assert_eq!(prods_password(), Some(password(alias)));
        let elsewhere = [
            alias.replace("user alice", "user bob"),
            alias.replace("port 22", "port 2222"),
        ];
        for other in elsewhere {
            assert_ne!(password(&other), password(alias), "{other}");
        }
    }

    /// Spec 0006, flow 7: what may be saved for a connection, and so what
    /// "Forget saved password…" forgets: its password, then the passphrase
    /// of each of its key files.
    #[test]
    fn a_connections_secrets_are_its_password_then_each_key_files_passphrase() {
        let expected = [
            Key::Password("alice@prod.example.com:22".into()),
            Key::Passphrase("/home/alice/.ssh/id_ed25519".into()),
            Key::Passphrase("/home/alice/.ssh/id_rsa".into()),
        ];
        assert_eq!(prod().secrets(), expected);
    }

    /// Spec 0006, flow 4 (TM-askpass-04): ssh's own password prompt for the
    /// connection's target, exactly, is filled with its password.
    #[test]
    fn the_password_prompt_ssh_writes_for_the_target_is_filled_with_its_password() {
        let prompt = "alice@prod.example.com's password: ";
        assert_eq!(prod().secret_for(prompt), prods_password());
    }

    /// Spec 0006, flow 4 (TM-askpass-04): a keyboard-interactive prompt is
    /// ssh's `(user@host) ` and the server's text. A text of `password:`,
    /// in any case and with spaces around it, asks for the password.
    #[test]
    fn a_keyboard_interactive_password_prompt_for_the_target_is_filled_in_any_case_and_spacing() {
        let texts = [
            "Password: ",
            "password:",
            "PASSWORD:",
            "pAsSwOrD: ",
            "  Password:  ",
        ];
        for text in texts {
            let prompt = format!("(alice@prod.example.com) {text}");
            assert_eq!(prod().secret_for(&prompt), prods_password(), "{prompt:?}");
        }
    }

    /// TM-askpass-04: the server words its keyboard-interactive prompts,
    /// and any text but `password:` gets a dialog, never the saved
    /// password: one-time codes, another account's password, a prompt
    /// that only starts or ends like a password prompt.
    #[test]
    fn a_servers_texts_other_than_password_are_never_filled() {
        let texts = [
            "Password for root@other.example.com: ",
            "Verification code: ",
            "One-time password: ",
            "Password: Password: ",
            "Pass word: ",
            "Passwort: ",
            "Password",
            "",
        ];
        for text in texts {
            let prompt = format!("(alice@prod.example.com) {text}");
            assert_eq!(prod().secret_for(&prompt), None, "{prompt:?}");
        }
    }

    /// TM-askpass-04, -19: a prompt that isn't exactly ssh's for this
    /// target is never filled. A jump host's prompts name the jump host,
    /// another account names another user, and a lookalike host is
    /// another host.
    #[test]
    fn a_prompt_naming_another_user_or_host_is_never_filled() {
        let prompts = [
            "alice@jump.example.com's password: ",
            "(alice@jump.example.com) Password: ",
            "bob@prod.example.com's password: ",
            "(root@prod.example.com) Password: ",
            "alice@prod.example.com.evil.example's password: ",
            "alice@prod's password: ",
            "alice@prod.example.com's password:",
            "alice@prod.example.com's password: \n",
            "Login as alice@prod.example.com's password: ",
            "(alice@prod.example.com)Password: ",
        ];
        for prompt in prompts {
            assert_eq!(prod().secret_for(prompt), None, "{prompt:?}");
        }
    }

    /// Spec 0006 §Model: with a `HostKeyAlias`, ssh's prompts name the
    /// alias, so only those are filled, not ones naming the host name.
    #[test]
    fn with_a_hostkeyalias_only_prompts_naming_it_are_filled() {
        let target = parse(PROD).expect("a target");
        let password = Some(Key::Password("alice@prod-key:2200".into()));
        assert_eq!(target.secret_for("alice@prod-key's password: "), password);
        assert_eq!(target.secret_for("(alice@prod-key) Password: "), password);
        assert_eq!(target.secret_for("alice@192.0.2.10's password: "), None);
        assert_eq!(target.secret_for("(alice@192.0.2.10) Password: "), None);
    }

    /// Spec 0006, flow 4: a passphrase prompt is filled only for one of the
    /// connection's own key files, with that file's passphrase.
    #[test]
    fn only_the_passphrase_prompts_of_the_connections_own_key_files_are_filled() {
        let prompt = |file: &str| format!("Enter passphrase for key '{file}': ");
        for file in ["/home/alice/.ssh/id_ed25519", "/home/alice/.ssh/id_rsa"] {
            let passphrase = Some(Key::Passphrase(file.into()));
            assert_eq!(prod().secret_for(&prompt(file)), passphrase, "{file}");
        }
        let others = [
            prompt("/home/alice/.ssh/id_ecdsa"),
            prompt("/home/bob/.ssh/id_ed25519"),
            prompt("~/.ssh/id_ed25519"),
            "Enter passphrase for key '/home/alice/.ssh/id_ed25519':".into(),
            "Enter passphrase for /home/alice/.ssh/id_ed25519: ".into(),
        ];
        for other in others {
            assert_eq!(prod().secret_for(&other), None, "{other:?}");
        }
    }

    /// ssh writes at most 100 bytes of a key's path into its passphrase
    /// prompt (`%.100s`), so a path of 100 bytes there may be a longer one
    /// that ssh cut. Only shorter paths are filled, or counted as saved.
    #[test]
    fn a_key_file_of_100_bytes_or_more_is_never_filled() {
        let dir = "/home/alice/.ssh/";
        let short = format!("{dir}{}", "a".repeat(99 - dir.len()));
        let exact = format!("{dir}{}", "b".repeat(100 - dir.len()));
        // A longer key whose path, cut to 100 bytes, is `exact`.
        let long = format!("{exact}-2");
        let target = Target {
            identity_files: vec![short.clone(), exact.clone(), long.clone()],
            ..prod()
        };
        let prompt = |file: &str| format!("Enter passphrase for key '{file}': ");
        assert_eq!(
            target.secret_for(&prompt(&short)),
            Some(Key::Passphrase(short.clone()))
        );
        // What ssh asks for `long` and for `exact` alike.
        assert_eq!(prompt(&long[..100]), prompt(&exact));
        assert_eq!(target.secret_for(&prompt(&exact)), None);
        assert_eq!(target.secret_for(&prompt(&long)), None);
        let saved = target.secrets();
        assert!(saved.contains(&Key::Passphrase(short)), "{saved:?}");
        for unsaved in [exact, long] {
            assert!(!saved.contains(&Key::Passphrase(unsaved)), "{saved:?}");
        }
    }

    /// TM-askpass-08: matching takes the prompt as ssh wrote it, so an
    /// invisible or direction character anywhere in it stops the fill.
    #[test]
    fn a_prompt_with_an_invisible_or_direction_character_is_never_filled() {
        let prompts = [
            "alice@prod.example.com\u{200B}'s password: ",
            "\u{202E}alice@prod.example.com's password: ",
            "alice@prod.example.com's password: \u{FEFF}",
            "(alice@prod.example.com) Pass\u{200B}word: ",
            "(alice@prod.example.com) \u{2066}Password:\u{2069} ",
            "(alice@prod.example.com) Password:\u{200E} ",
            "Enter passphrase for key '/home/alice/.ssh/id_ed25519\u{200F}': ",
        ];
        for prompt in prompts {
            assert_eq!(prod().secret_for(prompt), None, "{prompt:?}");
        }
    }
}
