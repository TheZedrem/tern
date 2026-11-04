//! SSH through the system `ssh` (spec 0006): what a `type: ssh` profile
//! runs. The host list and picker come with T-013, answering ssh's prompts
//! in the app with T-014; until then ssh asks in the terminal as usual.

use crate::config::SshProfile;
use crate::terminal::pty::{self, Command};
use std::collections::BTreeMap;

/// The program zed-term connects with: the OpenSSH client on `PATH`.
pub const PROGRAM: &str = "ssh";

/// The exit code ssh reports for a connection that failed or dropped,
/// which a pane offers to reconnect after (spec 0006, flow 5).
pub const CONNECTION_LOST: i32 = 255;

/// What connects to `profile`: `ssh [-l user] [-p port] [-i file] <host>`,
/// with only the options the profile sets, so `~/.ssh/config` still
/// applies to the rest (spec 0006, load-bearing).
pub fn command(profile: &SshProfile) -> Command {
    let mut args = Vec::new();
    if let Some(user) = &profile.user {
        args.extend(["-l".to_owned(), user.clone()]);
    }
    if let Some(port) = profile.port {
        args.extend(["-p".to_owned(), port.to_string()]);
    }
    if let Some(file) = &profile.identity_file {
        args.extend(["-i".to_owned(), file.display().to_string()]);
    }
    args.push(profile.host.clone());
    Command {
        program: PROGRAM.into(),
        args,
        env: BTreeMap::new(),
    }
}

/// Whether the OpenSSH client is installed: `ssh` is on `PATH`.
pub fn installed() -> bool {
    pty::on_path(if cfg!(windows) { "ssh.exe" } else { PROGRAM })
}

/// What an SSH pane says when there's no `ssh` to connect with (spec
/// 0006, flow 8).
pub fn not_installed() -> String {
    let install = if cfg!(windows) {
        "turn on Settings › System › Optional features › OpenSSH Client"
    } else if cfg!(target_os = "macos") {
        "check that /usr/bin, where macOS keeps it, is on PATH"
    } else {
        "install your distribution's OpenSSH client \
         (`openssh-clients` on Fedora, `openssh-client` on Debian and Ubuntu)"
    };
    format!(
        "zed-term connects through the OpenSSH client, and there is no `ssh` on PATH. To connect, {install}."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::ssh_to;
    use std::num::NonZeroU16;
    use std::path::PathBuf;

    /// The command line `command` runs: the program, then its arguments.
    fn argv(command: &Command) -> Vec<&str> {
        std::iter::once(command.program.as_str())
            .chain(command.args.iter().map(String::as_str))
            .collect()
    }

    /// AC3 (spec 0006).
    #[test]
    fn a_profile_with_user_port_and_identity_file_runs_exactly_ssh_l_p_i_host() {
        let profile = SshProfile {
            user: Some("alice".into()),
            port: NonZeroU16::new(2222),
            identity_file: Some(PathBuf::from("/home/alice/.ssh/id_ed25519")),
            ..ssh_to("myserver.example.com")
        };
        let expected = [
            "ssh",
            "-l",
            "alice",
            "-p",
            "2222",
            "-i",
            "/home/alice/.ssh/id_ed25519",
            "myserver.example.com",
        ];
        assert_eq!(argv(&command(&profile)), expected);
    }

    /// What a profile leaves out, `~/.ssh/config` decides (spec 0006
    /// §Model). A key path with a space stays one argument.
    #[test]
    fn only_the_options_a_profile_sets_go_on_the_command_line() {
        let key = || Some(PathBuf::from("/home/alice/my keys/id_ed25519"));
        let cases = [
            (ssh_to("prod"), vec!["ssh", "prod"]),
            (
                SshProfile {
                    user: Some("alice".into()),
                    ..ssh_to("prod")
                },
                vec!["ssh", "-l", "alice", "prod"],
            ),
            (
                SshProfile {
                    port: NonZeroU16::new(2222),
                    ..ssh_to("prod")
                },
                vec!["ssh", "-p", "2222", "prod"],
            ),
            (
                SshProfile {
                    identity_file: key(),
                    ..ssh_to("prod")
                },
                vec!["ssh", "-i", "/home/alice/my keys/id_ed25519", "prod"],
            ),
            // The order holds with one left out of the middle.
            (
                SshProfile {
                    user: Some("alice".into()),
                    identity_file: key(),
                    ..ssh_to("prod")
                },
                vec![
                    "ssh",
                    "-l",
                    "alice",
                    "-i",
                    "/home/alice/my keys/id_ed25519",
                    "prod",
                ],
            ),
        ];
        for (profile, expected) in cases {
            assert_eq!(argv(&command(&profile)), expected, "{profile:?}");
        }
    }

    /// Spec 0006, flow 8: the pane says what's missing and how to get it
    /// on this OS.
    #[test]
    fn without_ssh_the_pane_says_how_to_install_the_openssh_client_here() {
        let text = not_installed();
        assert!(text.contains("OpenSSH client"), "{text}");
        assert!(text.contains("PATH"), "{text}");
        let how = if cfg!(windows) {
            "Optional features"
        } else if cfg!(target_os = "macos") {
            "/usr/bin"
        } else {
            "openssh-client"
        };
        assert!(text.contains(how), "{text}");
    }
}
