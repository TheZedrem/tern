//! SSH through the system `ssh` (spec 0006): the hosts the picker lists,
//! what connects to them, and ssh's prompts answered in the app
//! ([`askpass`]) from what [`target`] resolves a connection to.

pub mod askpass;
pub mod config;
pub mod target;

use crate::config::{Profile, ProfileKind, SshProfile};
use crate::forwards::Forward;
use crate::secrets::{self, Key};
use crate::terminal::pty::{self, Command};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use target::Target;

/// The program Tern connects with: the OpenSSH client on `PATH`.
pub const PROGRAM: &str = "ssh";

/// What ssh runs with when Tern starts it outside a pane on Windows: no
/// console window. tern.exe is a GUI program with no console to share, so
/// Windows would open one for ssh, which flashes up and vanishes.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The group of profiles that name none.
const PROFILE_GROUP: &str = "Hosts";
/// The group of the hosts from `~/.ssh/config`.
const SSH_CONFIG_GROUP: &str = "SSH config";

/// One line of the host picker. The split picker lists local profiles as
/// lines too, with no destination (spec 0007, flow 6).
#[derive(Clone, Debug, PartialEq)]
pub struct Host {
    /// The ID of the profile that connects to it; `config::Config::profile`
    /// finds it, `ssh:<alias>` for a host from `~/.ssh/config` too.
    pub id: String,
    pub label: String,
    pub group: String,
    /// Where it connects, as `[user@]host[:port]`.
    pub destination: String,
}

/// `~/.ssh/config`, or `%USERPROFILE%\.ssh\config` on Windows.
pub fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".ssh").join("config"))
}

/// Every host the picker offers (spec 0006): the SSH profiles in
/// `profiles`, then the aliases of `ssh_config` that no profile already
/// connects to, so each destination is listed once. Also a notice per ssh
/// config file that couldn't be read.
pub fn hosts(profiles: &[Profile], ssh_config: &Path) -> (Vec<Host>, Vec<String>) {
    let mut hosts: Vec<Host> = profiles
        .iter()
        .filter_map(|profile| {
            let ProfileKind::Ssh(ssh) = &profile.kind else {
                return None;
            };
            let user = ssh.user.as_ref().map(|user| format!("{user}@"));
            let port = ssh.port.map(|port| format!(":{port}"));
            Some(Host {
                id: profile.id.clone(),
                label: profile.display_name().to_owned(),
                group: ssh.group.clone().unwrap_or_else(|| PROFILE_GROUP.into()),
                destination: format!(
                    "{}{}{}",
                    user.unwrap_or_default(),
                    ssh.host,
                    port.unwrap_or_default()
                ),
            })
        })
        .collect();
    let (aliases, notices) = config::aliases(ssh_config);
    let taken: Vec<&str> = profiles
        .iter()
        .filter_map(|profile| match &profile.kind {
            ProfileKind::Ssh(ssh) => Some(ssh.host.as_str()),
            ProfileKind::Local(_) => None,
        })
        .collect();
    let from_config: Vec<Host> = aliases
        .into_iter()
        .filter(|alias| !taken.contains(&alias.as_str()))
        .filter_map(|alias| Profile::ssh_alias(&alias))
        .map(|profile| Host {
            label: profile.display_name().to_owned(),
            destination: profile.display_name().to_owned(),
            id: profile.id,
            group: SSH_CONFIG_GROUP.into(),
        })
        .collect();
    hosts.extend(from_config);
    (hosts, notices)
}

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

/// The SFTP subsystem on the host `args` connect to (spec 0008): `args`
/// are [`command`]'s, so the host is last; the same options come first,
/// then `-s <host> sftp`. `None` for an empty list.
pub fn sftp_args(args: &[String]) -> Option<Vec<String>> {
    let (host, options) = args.split_last()?;
    let mut sftp = options.to_vec();
    sftp.extend(["-s".to_owned(), host.clone(), "sftp".to_owned()]);
    Some(sftp)
}

/// A port forward to the host `args` connect to (spec 0017, Command): the
/// same options first, then `-N -o ExitOnForwardFailure=yes`, the forward,
/// and the host. `None` for an empty list.
pub fn forward_args(args: &[String], forward: &Forward) -> Option<Vec<String>> {
    let (host, options) = args.split_last()?;
    let mut forwarded = options.to_vec();
    forwarded.extend([
        "-N".to_owned(),
        "-o".to_owned(),
        "ExitOnForwardFailure=yes".to_owned(),
        forward.flag().to_owned(),
        forward.arg(),
        host.clone(),
    ]);
    Some(forwarded)
}

/// Who and where [`command`]'s `args` connect to, as the SFTP sidebar's
/// header names it: `user@host`, or the host alone when the profile names
/// no user and `~/.ssh/config` decides (TM-sftp-01).
pub fn target_label(args: &[String]) -> String {
    let host = args.last().map(String::as_str).unwrap_or_default();
    let user = args
        .windows(2)
        .find(|pair| pair[0] == "-l")
        .map(|pair| pair[1].as_str());
    match user {
        Some(user) => format!("{user}@{host}"),
        None => host.to_owned(),
    }
}

/// Forgets what the keychain holds for the connection with `args` (spec
/// 0006, flow 7): its target's password and its keys' passphrases. Returns
/// what was forgotten. Runs `ssh -G` and keychain calls, so it blocks.
pub fn forget(args: &[String]) -> Result<Vec<Key>, String> {
    let target = Target::resolve(args).ok_or("ssh couldn't say where it connects")?;
    let mut forgotten = Vec::new();
    for key in target.secrets() {
        if secrets::delete(&key)? {
            forgotten.push(key);
        }
    }
    Ok(forgotten)
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
        "Tern connects through the OpenSSH client, and there is no `ssh` on PATH. To connect, {install}."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::fuzzy;
    use crate::config::Config;
    use crate::config::tests::{TempDir, ssh_to};
    use std::fs;
    use std::num::NonZeroU16;
    use std::path::PathBuf;

    /// The command line `command` runs: the program, then its arguments.
    fn argv(command: &Command) -> Vec<&str> {
        std::iter::once(command.program.as_str())
            .chain(command.args.iter().map(String::as_str))
            .collect()
    }

    /// SSH profile `id`, with no name or theme.
    fn ssh_profile(id: &str, ssh: SshProfile) -> Profile {
        Profile {
            id: id.into(),
            kind: ProfileKind::Ssh(ssh),
            ..Profile::shell()
        }
    }

    /// An ssh config in `dir` holding `text`.
    fn ssh_config(dir: &TempDir, text: &str) -> PathBuf {
        let path = dir.path().join("config");
        fs::write(&path, text).expect("write the ssh config");
        path
    }

    /// The IDs of `hosts`, in order.
    fn ids(hosts: &[Host]) -> Vec<&str> {
        hosts.iter().map(|host| host.id.as_str()).collect()
    }

    /// What the picker matches a query against (spec 0006, flow 1), as the
    /// palette writes it: the label, where the host connects, its group.
    fn haystack(host: &Host) -> String {
        format!("{} {} {}", host.label, host.destination, host.group)
    }

    /// AC1 (spec 0006).
    #[test]
    fn a_host_only_in_an_included_file_is_listed_and_host_star_and_host_not_foo_are_not() {
        let dir = TempDir::new("ssh-hosts-ac1");
        fs::write(
            dir.path().join("work.conf"),
            "Host work-box\n  HostName 10.0.0.1\n",
        )
        .expect("write work.conf");
        let path = ssh_config(
            &dir,
            concat!(
                "Host *\n",
                "  ServerAliveInterval 30\n",
                "Host !foo\n",
                "  User alice\n",
                "Include work.conf\n",
            ),
        );
        let (listed, notices) = hosts(&[], &path);
        let labels: Vec<&str> = listed.iter().map(|host| host.label.as_str()).collect();
        assert_eq!(labels, ["work-box"]);
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// Profiles in file order (spec 0005), then `~/.ssh/config` in its own.
    #[test]
    fn ssh_profiles_come_first_in_profile_order_then_the_ssh_config_hosts_in_file_order() {
        let dir = TempDir::new("ssh-hosts-order");
        let path = ssh_config(&dir, "Host yankee\nHost bravo\n");
        let profiles = [
            ssh_profile("zulu", ssh_to("zulu.example.com")),
            ssh_profile("alpha", ssh_to("alpha.example.com")),
        ];
        let (listed, _) = hosts(&profiles, &path);
        assert_eq!(ids(&listed), ["zulu", "alpha", "ssh:yankee", "ssh:bravo"]);
    }

    /// Spec 0006 §Model: the profile ID, its `name` (default: the ID) and
    /// its `group` (default: "Hosts").
    #[test]
    fn a_profile_is_listed_by_its_id_its_name_and_its_group() {
        let dir = TempDir::new("ssh-hosts-profile");
        let db = Profile {
            name: Some("Postgres".into()),
            ..ssh_profile(
                "db",
                SshProfile {
                    group: Some("Databases".into()),
                    ..ssh_to("db.example.com")
                },
            )
        };
        let staging = ssh_profile("staging", ssh_to("staging.example.com"));
        let (listed, _) = hosts(&[db, staging], &dir.path().join("config"));
        let expected = [
            Host {
                id: "db".into(),
                label: "Postgres".into(),
                group: "Databases".into(),
                destination: "db.example.com".into(),
            },
            Host {
                id: "staging".into(),
                label: "staging".into(),
                group: "Hosts".into(),
                destination: "staging.example.com".into(),
            },
        ];
        assert_eq!(listed, expected);
    }

    /// Where a host connects: `[user@]host[:port]`, with what the profile
    /// sets. The key it logs in with isn't part of that.
    #[test]
    fn a_profiles_destination_is_user_at_host_colon_port_with_only_what_it_sets() {
        let dir = TempDir::new("ssh-hosts-destination");
        let alice = || Some("alice".to_owned());
        let cases = [
            (ssh_to("prod"), "prod"),
            (
                SshProfile {
                    user: alice(),
                    ..ssh_to("prod")
                },
                "alice@prod",
            ),
            (
                SshProfile {
                    port: NonZeroU16::new(2222),
                    ..ssh_to("prod")
                },
                "prod:2222",
            ),
            (
                SshProfile {
                    user: alice(),
                    port: NonZeroU16::new(2222),
                    identity_file: Some(PathBuf::from("/home/alice/.ssh/id_ed25519")),
                    ..ssh_to("prod")
                },
                "alice@prod:2222",
            ),
        ];
        for (ssh, destination) in cases {
            let (listed, _) = hosts(
                &[ssh_profile("prod", ssh.clone())],
                &dir.path().join("config"),
            );
            let destinations: Vec<&str> = listed
                .iter()
                .map(|host| host.destination.as_str())
                .collect();
            assert_eq!(destinations, [destination], "{ssh:?}");
        }
    }

    /// Spec 0006 §Model.
    #[test]
    fn an_ssh_config_host_is_its_alias_in_the_group_ssh_config() {
        let dir = TempDir::new("ssh-hosts-alias");
        let path = ssh_config(&dir, "Host web\n  HostName web.internal.example.com\n");
        let (listed, _) = hosts(&[], &path);
        let expected = Host {
            id: "ssh:web".into(),
            label: "web".into(),
            group: "SSH config".into(),
            destination: "web".into(),
        };
        assert_eq!(listed, [expected]);
    }

    /// Spec 0006 §Model: each destination is listed once.
    #[test]
    fn a_profile_whose_host_is_an_ssh_config_alias_hides_that_alias() {
        let dir = TempDir::new("ssh-hosts-hidden-alias");
        let path = ssh_config(&dir, "Host web db\n");
        let deploy = ssh_profile(
            "deploy",
            SshProfile {
                user: Some("deploy".into()),
                ..ssh_to("web")
            },
        );
        let (listed, _) = hosts(&[deploy], &path);
        assert_eq!(ids(&listed), ["deploy", "ssh:db"]);
    }

    /// The picker lists `type: ssh` profiles; the command palette opens
    /// the local ones.
    #[test]
    fn local_profiles_are_not_hosts() {
        let dir = TempDir::new("ssh-hosts-local");
        let profiles = [
            Profile::shell(),
            Profile {
                id: "htop".into(),
                ..Profile::shell()
            },
            ssh_profile("prod", ssh_to("prod.example.com")),
        ];
        let (listed, _) = hosts(&profiles, &dir.path().join("config"));
        assert_eq!(ids(&listed), ["prod"]);
    }

    /// Spec 0006, flow 8: what `~/.ssh/config` can't give is a notice, and
    /// the rest is still listed.
    #[test]
    fn the_ssh_configs_notices_come_with_its_hosts() {
        let dir = TempDir::new("ssh-hosts-notices");
        // It includes itself until `Include` nests too deep.
        let path = ssh_config(&dir, "Host loop\nInclude config\n");
        let (listed, notices) = hosts(&[], &path);
        assert_eq!(ids(&listed), ["ssh:loop"]);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(notices, config::aliases(&path).1);
    }

    /// AC2 (spec 0006), the matcher's part: typing part of a group name
    /// narrows the picker to that group.
    #[test]
    fn typing_part_of_a_group_name_keeps_exactly_that_groups_hosts() {
        let dir = TempDir::new("ssh-hosts-ac2-groups");
        let path = ssh_config(&dir, "Host web1 cache\n");
        let grouped = |group: &str, ssh: SshProfile| SshProfile {
            group: Some(group.into()),
            ..ssh
        };
        let admin = SshProfile {
            user: Some("admin".into()),
            ..ssh_to("10.0.0.6")
        };
        let profiles = [
            Profile {
                name: Some("API".into()),
                ..ssh_profile("api", grouped("Production", ssh_to("10.0.0.5")))
            },
            Profile {
                name: Some("Database".into()),
                ..ssh_profile("db", grouped("Production", admin))
            },
            Profile {
                name: Some("CI runner".into()),
                ..ssh_profile("ci", grouped("Staging", ssh_to("10.1.0.7")))
            },
            ssh_profile("lab", ssh_to("10.2.0.8")),
        ];
        let (listed, _) = hosts(&profiles, &path);
        let haystacks: Vec<String> = listed.iter().map(haystack).collect();
        for (query, group) in [
            ("produ", "Production"),
            ("stag", "Staging"),
            ("host", "Hosts"),
            ("ssh conf", "SSH config"),
        ] {
            let in_group: Vec<usize> = (0..listed.len())
                .filter(|&ix| listed[ix].group == group)
                .collect();
            assert!(!in_group.is_empty(), "no host is in {group}: {listed:?}");
            let mut kept = fuzzy::rank(query, haystacks.iter().map(String::as_str));
            kept.sort_unstable();
            assert_eq!(kept, in_group, "`{query}` over {haystacks:?}");
        }
    }

    /// AC2 (spec 0006): Enter opens the host's ID as a profile, and a new
    /// tab is named after its profile, so after the host.
    #[test]
    fn every_listed_host_opens_an_ssh_profile_named_like_it() {
        let dir = TempDir::new("ssh-hosts-ac2-open");
        let path = ssh_config(&dir, "Host web db\n");
        let config = Config {
            profiles: vec![
                Profile {
                    name: Some("Production".into()),
                    ..ssh_profile("prod", ssh_to("prod.example.com"))
                },
                ssh_profile("staging", ssh_to("staging.example.com")),
            ],
            ..Config::default()
        };
        let (listed, _) = hosts(&config.profiles, &path);
        assert_eq!(listed.len(), 4, "{listed:?}");
        for host in &listed {
            let profile = config
                .profile(&host.id)
                .unwrap_or_else(|| panic!("`{}` opens no profile", host.id));
            assert_eq!(profile.display_name(), host.label, "{host:?}");
            assert!(
                matches!(profile.kind, ProfileKind::Ssh(_)),
                "`{}` opens {profile:?}",
                host.id
            );
        }
    }

    /// Spec 0006 §Model [load-bearing]: a host from `~/.ssh/config`
    /// connects as `ssh <alias>`, so all of the alias's settings apply.
    #[test]
    fn an_ssh_config_host_runs_exactly_ssh_and_its_alias() {
        let profile = Config::default()
            .profile("ssh:web")
            .expect("`ssh:web` opens a profile");
        assert_eq!(argv(&Command::of_profile(&profile)), ["ssh", "web"]);
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

    /// Spec 0008, Model, and TM-sftp-19: the SFTP connection runs the
    /// pane's own options, then `-s <host> sftp`, with the host from
    /// `command` and never anything after it from config.
    #[test]
    fn the_sftp_connection_uses_the_panes_options_then_s_host_sftp() {
        let profile = SshProfile {
            user: Some("alice".into()),
            port: NonZeroU16::new(2222),
            identity_file: Some(PathBuf::from("/home/alice/.ssh/id_ed25519")),
            ..ssh_to("myserver.example.com")
        };
        let expected = [
            "-l",
            "alice",
            "-p",
            "2222",
            "-i",
            "/home/alice/.ssh/id_ed25519",
            "-s",
            "myserver.example.com",
            "sftp",
        ];
        assert_eq!(
            sftp_args(&command(&profile).args),
            Some(expected.map(String::from).to_vec())
        );
        assert_eq!(
            sftp_args(&command(&ssh_to("web")).args),
            Some(["-s", "web", "sftp"].map(String::from).to_vec())
        );
    }

    /// Without a host there's nothing to connect the sidebar to.
    #[test]
    fn no_arguments_give_no_sftp_connection() {
        assert_eq!(sftp_args(&[]), None);
    }

    /// Spec 0017, Command, and AC1: a forward runs the pane's own options,
    /// then `-N -o ExitOnForwardFailure=yes` and one forward, then the host
    /// last, as `command` put it.
    #[test]
    fn a_forward_runs_the_panes_options_then_n_exit_on_failure_the_forward_and_the_host() {
        let pane = command(&SshProfile {
            user: Some("me".into()),
            ..ssh_to("bastion")
        })
        .args;
        let forward = |kind, listen, to, network| {
            let forward = crate::forwards::Forward::parse(kind, listen, to, network)
                .expect("a valid forward");
            forward_args(&pane, &forward).expect("a host to forward through")
        };
        use crate::forwards::Kind;
        let expected = |flag: &str, value: &str| {
            [
                "-l",
                "me",
                "-N",
                "-o",
                "ExitOnForwardFailure=yes",
                flag,
                value,
                "bastion",
            ]
            .map(String::from)
            .to_vec()
        };
        assert_eq!(
            forward(Kind::Local, "5432", "db.internal:5432", false),
            expected("-L", "127.0.0.1:5432:db.internal:5432")
        );
        assert_eq!(
            forward(Kind::Remote, "9000", "localhost:3000", false),
            expected("-R", "127.0.0.1:9000:localhost:3000")
        );
        assert_eq!(
            forward(Kind::Socks, "1080", "", false),
            expected("-D", "127.0.0.1:1080")
        );
        assert_eq!(
            forward(Kind::Local, "5432", "db.internal:5432", true),
            expected("-L", "0.0.0.0:5432:db.internal:5432")
        );
    }

    /// TM-fwd-06: a forward through a profile with a port and key keeps
    /// every option before the host, and nothing comes after the host.
    #[test]
    fn a_forward_keeps_every_pane_option_before_the_host() {
        let pane = command(&SshProfile {
            port: NonZeroU16::new(2222),
            identity_file: Some(PathBuf::from("/home/me/.ssh/id_ed25519")),
            ..ssh_to("web")
        })
        .args;
        let forward =
            crate::forwards::Forward::parse(crate::forwards::Kind::Socks, "1080", "", false)
                .expect("a valid forward");
        let args = forward_args(&pane, &forward).expect("a host to forward through");
        assert_eq!(&args[..4], ["-p", "2222", "-i", "/home/me/.ssh/id_ed25519"]);
        assert_eq!(args.last().map(String::as_str), Some("web"));
    }

    /// Without a host there's nothing to forward through.
    #[test]
    fn no_arguments_give_no_forward() {
        let forward =
            crate::forwards::Forward::parse(crate::forwards::Kind::Socks, "1080", "", false)
                .expect("a valid forward");
        assert_eq!(forward_args(&[], &forward), None);
    }

    /// Spec 0008, Model: the sidebar's header names its target as
    /// `user@host`, or the host alone when `~/.ssh/config` picks the user.
    #[test]
    fn the_sidebar_names_its_target_user_at_host_or_the_host_alone() {
        let full = SshProfile {
            user: Some("alice".into()),
            port: NonZeroU16::new(2222),
            identity_file: Some(PathBuf::from("/home/alice/.ssh/id_ed25519")),
            ..ssh_to("myserver.example.com")
        };
        let port_only = SshProfile {
            port: NonZeroU16::new(2222),
            ..ssh_to("prod")
        };
        for (profile, expected) in [
            (full, "alice@myserver.example.com"),
            (ssh_to("web"), "web"),
            (port_only, "prod"),
        ] {
            assert_eq!(target_label(&command(&profile).args), expected);
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
