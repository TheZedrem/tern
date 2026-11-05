//! `tern --import-hosts <file> [--dry-run]` (spec 0010): the SSH hosts
//! of a MobaXterm or Tabby file added to `config.yaml` as `type: ssh`
//! profiles, once, with a report on stdout. Runs instead of the app: no
//! window, no PTY, no session. Passwords and vaults are never read into a
//! host; the parsers have no field for them.

mod mobaxterm;
mod tabby;

use crate::config::{self, Config, ProfileKind};
use crate::settings::change::{ProfileForm, new_id};
use crate::settings::yaml;
use crate::ssh;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::Path;

/// Files over this many bytes are refused, unread (spec 0010, flow 2).
const MAX_BYTES: u64 = 10 * 1024 * 1024;

/// What the report says no imported host keeps.
const LEFT_BEHIND: &str =
    "Never imported: passwords, port forwards, X11, proxy and terminal settings.";

/// One SSH host found in the file, as a profile would hold it (spec 0006).
/// Every text is free of control characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    pub name: String,
    pub host: String,
    /// `None` for 22.
    pub port: Option<u16>,
    pub user: Option<String>,
    pub identity_file: Option<String>,
    pub group: Option<String>,
}

/// Why a session of the file isn't imported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Skip {
    NotSsh,
    JumpHost,
    Proxy,
    BadHost,
    BadPort,
}

impl fmt::Display for Skip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Skip::NotSsh => "not SSH",
            Skip::JumpHost => "behind a jump host",
            Skip::Proxy => "behind a proxy",
            Skip::BadHost => "no usable host",
            Skip::BadPort => "an invalid port",
        })
    }
}

/// What a parser found: the hosts in file order, and a skip per session
/// that isn't one.
#[derive(Debug, Default)]
pub struct Parsed {
    pub format: &'static str,
    pub hosts: Vec<Host>,
    pub skipped: Vec<Skip>,
}

impl Parsed {
    fn push(&mut self, session: Result<Host, Skip>) {
        match session {
            Ok(host) => self.hosts.push(host),
            Err(skip) => self.skipped.push(skip),
        }
    }
}

impl Host {
    /// A host from a file's raw values, cleaned: control characters
    /// stripped, empty values `None`, port 22 left out. A name falls back
    /// to the host.
    fn new(
        name: &str,
        host: &str,
        port: Option<u16>,
        user: &str,
        identity_file: &str,
        group: Option<&str>,
    ) -> Result<Self, Skip> {
        let host = clean(host);
        // ssh would read a host starting with `-` as an option.
        if host.is_empty() || host.starts_with('-') || host.contains(char::is_whitespace) {
            return Err(Skip::BadHost);
        }
        if port == Some(0) {
            return Err(Skip::BadPort);
        }
        let some = |text: &str| Some(clean(text)).filter(|text| !text.is_empty());
        Ok(Self {
            name: some(name).unwrap_or_else(|| host.clone()),
            port: port.filter(|&port| port != 22),
            user: some(user),
            identity_file: some(identity_file),
            group: group.and_then(some),
            host,
        })
    }

    /// `user@host:port`, with only what's set, as the host picker shows it.
    fn destination(&self) -> String {
        let user = self.user.as_ref().map(|user| format!("{user}@"));
        let port = self.port.map(|port| format!(":{port}"));
        format!(
            "{}{}{}",
            user.unwrap_or_default(),
            self.host,
            port.unwrap_or_default()
        )
    }

    /// What makes two hosts the same destination: user, host, port.
    fn key(&self) -> (Option<&str>, String, u16) {
        (
            self.user.as_deref(),
            self.host.to_lowercase(),
            self.port.unwrap_or(22),
        )
    }

    fn form(&self) -> ProfileForm {
        ProfileForm {
            name: self.name.clone(),
            ssh: true,
            host: self.host.clone(),
            user: self.user.clone().unwrap_or_default(),
            port: self.port.map(|port| port.to_string()).unwrap_or_default(),
            identity_file: self.identity_file.clone().unwrap_or_default(),
            group: self.group.clone().unwrap_or_default(),
            ..ProfileForm::default()
        }
    }
}

/// `text` without control characters, so nothing in the file can send an
/// escape sequence to the user's terminal or break a config line; trimmed.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

/// What importing into a config does: the hosts it adds under their new
/// IDs, and the ones it leaves out as duplicates, with what they duplicate.
#[derive(Debug, Default)]
pub struct Plan {
    pub imported: Vec<(String, Host)>,
    pub duplicates: Vec<(Host, String)>,
}

/// Plans importing `hosts` into `config`, whose ssh config defines
/// `aliases`. A host is a duplicate of a profile or alias with its user,
/// host and port, or of an earlier host of the file (spec 0010, Model).
pub fn plan(hosts: &[Host], config: &Config, aliases: &[String]) -> Plan {
    let mut taken: Vec<String> = config.profiles.iter().map(|p| p.id.clone()).collect();
    taken.push(config::SHELL.to_owned());
    let profiles: Vec<_> = config
        .profiles
        .iter()
        .filter_map(|profile| match &profile.kind {
            ProfileKind::Ssh(ssh) => Some((
                (
                    ssh.user.as_deref(),
                    ssh.host.to_lowercase(),
                    ssh.port.map_or(22, |port| port.get()),
                ),
                profile.id.as_str(),
            )),
            ProfileKind::Local(_) => None,
        })
        .collect();
    let mut plan = Plan::default();
    for host in hosts {
        let key = host.key();
        let duplicate = if let Some((_, id)) = profiles.iter().find(|(found, _)| *found == key) {
            Some(format!("profile `{id}`"))
        } else if key.0.is_none() && key.2 == 22 && aliases.contains(&host.host) {
            Some("in ~/.ssh/config".to_owned())
        } else if plan
            .imported
            .iter()
            .any(|(_, earlier)| earlier.key() == key)
        {
            Some("earlier in the file".to_owned())
        } else {
            None
        };
        match duplicate {
            Some(of) => plan.duplicates.push((host.clone(), of)),
            None => {
                let id = new_id(&host.name, &taken);
                taken.push(id.clone());
                plan.imported.push((id, host.clone()));
            }
        }
    }
    plan
}

impl Plan {
    /// The config `text` with the imported hosts appended under `profiles:`
    /// and no other line changed, checked to read back with every one of
    /// them and no new notice.
    pub fn apply(&self, text: &str) -> Result<String, String> {
        let edited = self
            .imported
            .iter()
            .try_fold(text.to_owned(), |text, (id, host)| {
                yaml::upsert(&text, &["profiles", id], &host.form().lines(id))
            })
            .ok_or("config.yaml writes `profiles` in a way the import can't add to safely")?;
        let (before, after) = (config::parse(text), config::parse(&edited));
        let holds = after.notices.len() <= before.notices.len()
            && self
                .imported
                .iter()
                .all(|(id, _)| after.config.profiles.iter().any(|p| p.id == *id));
        if holds {
            Ok(edited)
        } else {
            Err("the hosts didn't read back from config.yaml; \
                 fix the problems the app reports in it first"
                .to_owned())
        }
    }
}

/// Runs the import of `file` and prints its report. Returns the exit code.
pub fn run(file: &Path, dry_run: bool) -> i32 {
    match import(file, dry_run) {
        Ok(report) => {
            print!("{report}");
            0
        }
        Err(error) => {
            eprintln!("tern: {error}");
            1
        }
    }
}

fn import(file: &Path, dry_run: bool) -> Result<String, String> {
    let parsed = parse(&read(file)?).ok_or_else(|| {
        format!(
            "{} is neither a MobaXterm session file nor a Tabby config.yaml",
            file.display()
        )
    })?;
    let path = config::default_path().ok_or("this system has no config directory")?;
    let aliases = ssh::config_path()
        .map(|path| ssh::config::aliases(&path).0)
        .unwrap_or_default();
    let plan_for = |text: &str| plan(&parsed.hosts, &config::parse(text).config, &aliases);
    let plan = if dry_run {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => config::TEMPLATE.to_owned(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let plan = plan_for(&text);
        plan.apply(&text)?;
        plan
    } else {
        let planned = RefCell::new(Plan::default());
        config::save_edit(&path, |text| {
            let plan = plan_for(text);
            let edited = plan.apply(text)?;
            planned.replace(plan);
            Ok(edited)
        })?;
        planned.into_inner()
    };
    Ok(report(&parsed, &plan, &path, dry_run))
}

/// Reads `file`, refusing one over [`MAX_BYTES`]. MobaXterm writes
/// CP1252, so text that isn't UTF-8 is read as that.
fn read(file: &Path) -> Result<String, String> {
    let fail = |e: std::io::Error| format!("{}: {e}", file.display());
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .map_err(fail)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(fail)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("{} is over 10 MB", file.display()));
    }
    let text = String::from_utf8(bytes).unwrap_or_else(|e| cp1252(e.as_bytes()));
    Ok(match text.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_owned(),
        None => text,
    })
}

/// Windows-1252 text as a string. Its five unassigned bytes become their
/// C1 controls, which [`clean`] drops.
fn cp1252(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    bytes
        .iter()
        .map(|&byte| match byte {
            0x80..=0x9f => HIGH[usize::from(byte - 0x80)],
            _ => char::from(byte),
        })
        .collect()
}

/// The hosts of `text`, told apart by content: `[Bookmarks…]` sections
/// are MobaXterm, `profiles:` entries with a `type:` are Tabby. `None` for
/// anything else.
pub fn parse(text: &str) -> Option<Parsed> {
    if mobaxterm::detect(text) {
        Some(mobaxterm::parse(text))
    } else {
        tabby::parse(text)
    }
}

/// What the import prints: each imported host, then the duplicates, then
/// the skipped sessions counted by reason.
fn report(parsed: &Parsed, plan: &Plan, config: &Path, dry_run: bool) -> String {
    let mut out = String::new();
    let line =
        |host: &Host, note: String| format!("  {}  {}{note}\n", host.name, host.destination());
    let count = plan.imported.len();
    let hosts = if count == 1 { "host" } else { "hosts" };
    if count == 0 {
        out.push_str(&format!("No new hosts in this {} file.\n", parsed.format));
    } else if dry_run {
        out.push_str(&format!(
            "Would import {count} {hosts} from {} into {} (dry run, nothing written):\n",
            parsed.format,
            config.display()
        ));
    } else {
        out.push_str(&format!(
            "Imported {count} {hosts} from {} into {}:\n",
            parsed.format,
            config.display()
        ));
    }
    for (_, host) in &plan.imported {
        let group = host.group.as_ref().map(|g| format!("  ({g})"));
        out.push_str(&line(host, group.unwrap_or_default()));
    }
    if !plan.duplicates.is_empty() {
        out.push_str("Already there, not imported:\n");
        for (host, of) in &plan.duplicates {
            out.push_str(&line(host, format!("  ({of})")));
        }
    }
    let mut skipped = BTreeMap::<Skip, usize>::new();
    for &skip in &parsed.skipped {
        *skipped.entry(skip).or_default() += 1;
    }
    if !skipped.is_empty() {
        out.push_str("Skipped:\n");
        for (skip, n) in skipped {
            let sessions = if n == 1 { "session" } else { "sessions" };
            out.push_str(&format!("  {n} {sessions}: {skip}\n"));
        }
    }
    out.push_str(LEFT_BEHIND);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TEMPLATE;
    use std::path::PathBuf;

    /// A MobaXterm session value: connection fields as MobaXterm 23 writes
    /// them, with `kind` (0 = SSH), host, port, user, gateway (field 8) and
    /// key (field 14) filled in.
    fn moba(kind: &str, host: &str, port: &str, user: &str, gateway: &str, key: &str) -> String {
        format!(
            "#109#{kind}%{host}%{port}%{user}%%-1%-1%%{gateway}%%%0%0%0%{key}%%-1%0%0%0%%1080%%0%0%1#MobaFont%10%0%0%-1%15%236,236,236%30,30,30%180,180,192%0%-1%0%%xterm%-1%-1%_Std_Colors_0_%80%24%0%1%-1%<none>%%0%0%-1#0# #-1"
        )
    }

    fn host(name: &str, host: &str, user: Option<&str>) -> Host {
        Host {
            name: name.to_owned(),
            host: host.to_owned(),
            port: None,
            user: user.map(str::to_owned),
            identity_file: None,
            group: None,
        }
    }

    fn mobaxterm_ini() -> String {
        [
            "[Bookmarks]".to_owned(),
            "SubRep=".to_owned(),
            "ImgNum=42".to_owned(),
            format!(
                "top={}",
                moba("0", "top.example.com", "22", "alice", "", "")
            ),
            "[Bookmarks_1]".to_owned(),
            "SubRep=Prod\\Web".to_owned(),
            "ImgNum=41".to_owned(),
            format!(
                "web1={}",
                moba(
                    "0",
                    "web1.example.com",
                    "2222",
                    "deploy",
                    "",
                    "_ProfileDir_\\.ssh\\id_rsa"
                )
            ),
            format!(
                "desk={}",
                moba("4", "desk.example.com", "3389", "bob", "", "")
            ),
            format!(
                "inner={}",
                moba("0", "10.0.0.5", "22", "carol", "jump.example.com", "")
            ),
            "[Passwords]".to_owned(),
            "ssh22:deploy@web1.example.com=MOBA-SECRET-PASSWORD".to_owned(),
            "[Credentials]".to_owned(),
            "stored=deploy:MOBA-SECRET-CREDENTIAL".to_owned(),
        ]
        .join("\r\n")
    }

    const TABBY: &str = r#"
version: 3
profiles:
  - type: ssh
    name: DB primary
    group: 4f1c-group-id
    options:
      host: db.example.com
      port: 2200
      user: postgres
      password: TABBY-SECRET-PASSWORD
      privateKeys:
        - file:///home/someone/.ssh/id_db
  - type: ssh
    name: Default user
    group: Legacy Name
    options:
      host: plain.example.com
  - type: local
    name: Zsh
    options:
      command: zsh
  - type: telnet
    name: Router
    options:
      host: router.example.com
      port: 23
groups:
  - id: 4f1c-group-id
    name: Databases
vault:
  version: 1
  contents: TABBY-SECRET-VAULT
  keySalt: abc
  iv: def
"#;

    /// Whether every line of `before` is in `after`, in order.
    fn keeps_every_line(before: &str, after: &str) -> bool {
        let mut after = after.lines();
        before.lines().all(|line| after.any(|found| found == line))
    }

    /// AC1, AC8: nested folders become `Folder / Subfolder`, with host,
    /// port, user and a `_ProfileDir_` key under `~`.
    #[test]
    fn mobaxterm_ssh_sessions_import_with_their_folders() {
        let parsed = parse(&mobaxterm_ini()).expect("MobaXterm");
        assert_eq!(parsed.format, "MobaXterm");
        assert_eq!(
            parsed.hosts,
            [
                Host {
                    user: Some("alice".into()),
                    ..host("top", "top.example.com", None)
                },
                Host {
                    name: "web1".into(),
                    host: "web1.example.com".into(),
                    port: Some(2222),
                    user: Some("deploy".into()),
                    identity_file: Some("~\\.ssh\\id_rsa".into()),
                    group: Some("Prod / Web".into()),
                },
            ]
        );
        let mut skipped = parsed.skipped.clone();
        skipped.sort();
        assert_eq!(skipped, [Skip::NotSsh, Skip::JumpHost]);
    }

    /// AC1 end to end: the imported profiles read back from the config.
    #[test]
    fn mobaxterm_hosts_read_back_as_ssh_profiles() {
        let parsed = parse(&mobaxterm_ini()).expect("MobaXterm");
        let config = config::parse(TEMPLATE).config;
        let text = plan(&parsed.hosts, &config, &[])
            .apply(TEMPLATE)
            .expect("applies");
        let loaded = config::parse(&text);
        let web = loaded
            .config
            .profiles
            .iter()
            .find(|p| p.id == "web1")
            .expect("web1 imported");
        let ProfileKind::Ssh(ssh) = &web.kind else {
            panic!("not an ssh profile: {web:?}");
        };
        assert_eq!(ssh.host, "web1.example.com");
        assert_eq!(ssh.user.as_deref(), Some("deploy"));
        assert_eq!(ssh.port.map(|p| p.get()), Some(2222));
        assert_eq!(ssh.group.as_deref(), Some("Prod / Web"));
        assert!(ssh.identity_file.is_some());
        assert!(
            text.contains(r#"identity_file: "~\\.ssh\\id_rsa""#),
            "key not under ~:\n{text}"
        );
    }

    /// AC2: SSH profiles with their groups (by ID, or by name in older
    /// configs); other types skipped and counted.
    #[test]
    fn tabby_ssh_profiles_import_with_their_groups() {
        let parsed = parse(TABBY).expect("Tabby");
        assert_eq!(parsed.format, "Tabby");
        assert_eq!(
            parsed.hosts,
            [
                Host {
                    name: "DB primary".into(),
                    host: "db.example.com".into(),
                    port: Some(2200),
                    user: Some("postgres".into()),
                    identity_file: Some("/home/someone/.ssh/id_db".into()),
                    group: Some("Databases".into()),
                },
                Host {
                    group: Some("Legacy Name".into()),
                    ..host("Default user", "plain.example.com", Some("root"))
                },
            ]
        );
        assert_eq!(parsed.skipped, [Skip::NotSsh, Skip::NotSsh]);
        let plan = plan(&parsed.hosts, &config::parse(TEMPLATE).config, &[]);
        let report = report(&parsed, &plan, Path::new("config.yaml"), false);
        assert!(report.contains("2 sessions: not SSH"), "{report}");
    }

    /// AC2: a serial profile is skipped like a local one. Tabby writes a
    /// serial port as a device path, which must not fail the whole file.
    #[test]
    fn tabby_serial_profiles_are_skipped_and_counted() {
        let parsed = parse(
            "profiles:
  - type: serial
    name: Console
    options:
      port: /dev/ttyUSB0
      baudrate: 115200
  - type: ssh
    name: Box
    options: { host: box.example.com }
",
        )
        .expect("a Tabby config with a serial profile");
        assert_eq!(parsed.skipped, [Skip::NotSsh]);
        assert_eq!(parsed.hosts.len(), 1);
    }

    /// AC3: a dry run reports the same hosts a real run imports; applying
    /// returns new text and the plan doesn't depend on whether it's written.
    #[test]
    fn a_dry_run_reports_what_a_real_run_imports() {
        let parsed = parse(TABBY).expect("Tabby");
        let config = config::parse(TEMPLATE).config;
        let plan = plan(&parsed.hosts, &config, &[]);
        let original = TEMPLATE.to_owned();
        plan.apply(&original).expect("applies");
        assert_eq!(original, TEMPLATE);
        let path = Path::new("config.yaml");
        let dry = report(&parsed, &plan, path, true);
        let real = report(&parsed, &plan, path, false);
        assert!(dry.starts_with("Would import 2 hosts"), "{dry}");
        assert!(real.starts_with("Imported 2 hosts"), "{real}");
        let body = |text: &str| text.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(body(&dry), body(&real));
    }

    /// AC4: a host matching an existing profile is a duplicate.
    #[test]
    fn a_host_already_a_profile_is_a_duplicate() {
        let text = "profiles:\n  db:\n    type: ssh\n    host: DB.example.com\n    user: postgres\n    port: 2200\n";
        let config = config::parse(text).config;
        let parsed = parse(TABBY).expect("Tabby");
        let plan = plan(&parsed.hosts, &config, &[]);
        assert_eq!(plan.duplicates.len(), 1);
        assert_eq!(plan.duplicates[0].0.host, "db.example.com");
        assert_eq!(plan.duplicates[0].1, "profile `db`");
        assert_eq!(plan.imported.len(), 1);
    }

    /// AC4: a host named by a `~/.ssh/config` alias is a duplicate.
    #[test]
    fn a_host_that_is_an_ssh_config_alias_is_a_duplicate() {
        let config = config::parse(TEMPLATE).config;
        let hosts = [
            host("Bastion", "bastion", None),
            host("Other", "other", None),
        ];
        let plan = plan(&hosts, &config, &["bastion".to_owned()]);
        assert_eq!(plan.duplicates.len(), 1);
        assert_eq!(plan.duplicates[0].0.host, "bastion");
        assert_eq!(plan.imported.len(), 1);
        assert_eq!(plan.imported[0].1.host, "other");
    }

    /// AC4: the same destination twice in one file imports once; the same
    /// name with another destination gets a unique ID.
    #[test]
    fn a_repeat_within_the_file_is_a_duplicate() {
        let config = config::parse(TEMPLATE).config;
        let hosts = [
            host("Web", "web.example.com", Some("u")),
            host("Web again", "WEB.example.com", Some("u")),
            host("Web", "web.example.com", Some("other")),
        ];
        let plan = plan(&hosts, &config, &[]);
        assert_eq!(plan.duplicates.len(), 1);
        assert_eq!(plan.duplicates[0].1, "earlier in the file");
        let ids: Vec<&str> = plan.imported.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["web", "web-2"]);
    }

    /// AC4: running the same import twice adds nothing the second time.
    #[test]
    fn importing_twice_adds_nothing_the_second_time() {
        for file in [mobaxterm_ini(), TABBY.to_owned()] {
            let parsed = parse(&file).expect("parses");
            let first = plan(&parsed.hosts, &config::parse(TEMPLATE).config, &[]);
            assert!(!first.imported.is_empty());
            let text = first.apply(TEMPLATE).expect("applies");
            let second = plan(&parsed.hosts, &config::parse(&text).config, &[]);
            assert!(second.imported.is_empty(), "{:?}", second.imported);
            assert_eq!(second.duplicates.len(), parsed.hosts.len());
            assert_eq!(second.apply(&text).as_deref(), Ok(text.as_str()));
        }
    }

    /// AC5: no saved password, credential or vault content reaches the
    /// config or the report.
    #[test]
    fn no_secret_reaches_the_config_or_the_report() {
        for file in [mobaxterm_ini(), TABBY.to_owned()] {
            let parsed = parse(&file).expect("parses");
            let plan = plan(&parsed.hosts, &config::parse(TEMPLATE).config, &[]);
            let text = plan.apply(TEMPLATE).expect("applies");
            let report = report(&parsed, &plan, Path::new("config.yaml"), false);
            for out in [&text, &report, &format!("{:?}", parsed.hosts)] {
                assert!(!out.contains("SECRET"), "a secret leaked:\n{out}");
                assert!(!out.contains("password:"), "a password key:\n{out}");
            }
        }
    }

    /// AC6: appending changes no existing line, in the template and in a
    /// config with profiles followed by other keys, and the result parses
    /// with every host and no new notice.
    #[test]
    fn applying_keeps_every_existing_line() {
        let existing = "\
# my config
default_profile: work
profiles:
  work:
    type: local
    cwd: ~/work   # where I work
  box:
    type: ssh
    host: box.example.com
font:
  size: 14
";
        let parsed = parse(&mobaxterm_ini()).expect("MobaXterm");
        for before in [TEMPLATE, existing] {
            let loaded = config::parse(before);
            let plan = plan(&parsed.hosts, &loaded.config, &[]);
            let after = plan.apply(before).expect("applies");
            assert!(keeps_every_line(before, &after), "changed a line:\n{after}");
            let reloaded = config::parse(&after);
            assert_eq!(reloaded.notices, loaded.notices);
            assert_eq!(
                reloaded.config.profiles.len(),
                loaded.config.profiles.len() + parsed.hosts.len()
            );
        }
    }

    /// AC7: files that are neither format aren't parsed.
    #[test]
    fn an_unrecognized_file_is_not_parsed() {
        let cases = [
            "",
            "hello world\n",
            "[General]\nkey=value\n",
            "profiles: []\n",
            "profiles:\n  - name: no type\n",
            "profiles:\n  - type: ssh\n    options: [unclosed\n",
            // A Tern config: `profiles` is a map, not a list.
            "profiles:\n  box:\n    type: ssh\n    host: box\n",
        ];
        for text in cases {
            assert!(parse(text).is_none(), "parsed {text:?}");
        }
    }

    /// A unique path in the temp dir, removed on drop.
    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str, bytes: &[u8]) -> Self {
            let path =
                std::env::temp_dir().join(format!("tern-import-{}-{name}", std::process::id()));
            std::fs::write(&path, bytes).expect("temp file");
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// AC7: over 10 MB is refused; exactly 10 MB isn't.
    #[test]
    fn a_file_over_10_mb_is_refused() {
        let limit = MAX_BYTES as usize;
        let over = TempFile::new("over", &vec![b'a'; limit + 1]);
        assert!(read(&over.0).expect_err("refused").contains("10 MB"));
        let at = TempFile::new("at", &vec![b'a'; limit]);
        assert_eq!(read(&at.0).map(|text| text.len()), Ok(limit));
    }

    /// AC7: a missing file is an error naming it.
    #[test]
    fn a_missing_file_is_an_error() {
        let path = std::env::temp_dir().join("tern-import-does-not-exist.ini");
        let error = read(&path).expect_err("missing");
        assert!(error.contains("tern-import-does-not-exist.ini"), "{error}");
    }

    /// MobaXterm writes CP1252, sometimes with a BOM; both read as text.
    #[test]
    fn cp1252_and_a_bom_are_read() {
        let cp1252 = TempFile::new("cp1252", b"[Bookmarks]\r\nCaf\xe9 \x80=x\r\n");
        assert_eq!(
            read(&cp1252.0).as_deref(),
            Ok("[Bookmarks]\r\nCafé €=x\r\n")
        );
        let bom = TempFile::new("bom", "\u{feff}[Bookmarks]\n".as_bytes());
        assert_eq!(read(&bom.0).as_deref(), Ok("[Bookmarks]\n"));
    }

    /// Spec 0010, Security: names reach the terminal and the config
    /// without control characters.
    #[test]
    fn control_characters_are_stripped() {
        let ini = format!(
            "[Bookmarks]\nSubRep=Ops\x07\\Red\x1b[31m\nevil\x1b]0;pwned\x07name={}\n",
            moba("0", "host.example.com", "22", "ro\x1bot", "", "")
        );
        let parsed = parse(&ini).expect("MobaXterm");
        let found = &parsed.hosts[0];
        assert_eq!(found.name, "evil]0;pwnedname");
        assert_eq!(found.user.as_deref(), Some("root"));
        assert_eq!(found.group.as_deref(), Some("Ops / Red[31m"));
        let plan = plan(&parsed.hosts, &config::parse(TEMPLATE).config, &[]);
        let report = report(&parsed, &plan, Path::new("config.yaml"), false);
        assert!(
            !report.contains('\x1b') && !report.contains('\x07'),
            "{report:?}"
        );
    }
}
