//! `tern --import-hosts <file> [--dry-run]` (spec 0010): the SSH hosts
//! of a MobaXterm, Tabby, PuTTY or WinSCP file (spec 0015) added to
//! `config.yaml` as `type: ssh` profiles, once, with a report on stdout.
//! Runs instead of the app: no window, no PTY, no session. Passwords and
//! vaults are never read into a host; the parsers have no field for them.

mod mobaxterm;
mod putty;
#[cfg(windows)]
mod registry;
mod sessions;
mod tabby;
mod winscp;

use crate::config::{self, Config, ProfileKind};
use crate::settings::change::{ProfileForm, new_id};
use crate::settings::yaml;
use crate::ssh;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

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
    /// A PuTTY-format key the session named, which OpenSSH can't read: left
    /// out of the profile, named in the report (spec 0015, Model).
    pub ppk: Option<String>,
}

/// Why a session of the file isn't imported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Skip {
    NotSsh,
    JumpHost,
    Proxy,
    BadHost,
    BadPort,
    Workspace,
}

impl fmt::Display for Skip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Skip::NotSsh => "not SSH",
            Skip::JumpHost => "behind a jump host",
            Skip::Proxy => "behind a proxy",
            Skip::BadHost => "no usable host",
            Skip::BadPort => "an invalid port",
            Skip::Workspace => "a WinSCP workspace",
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
    /// Whether a host without a user is asked for one: PuTTY and WinSCP
    /// often save none (spec 0015, Model).
    pub asks_user: bool,
}

impl Parsed {
    fn push(&mut self, session: Result<Host, Skip>) {
        match session {
            Ok(host) => self.hosts.push(host),
            Err(skip) => self.skipped.push(skip),
        }
    }

    /// The hosts asked for a user: none of them saved one.
    fn asking(&self) -> impl Iterator<Item = &Host> {
        self.hosts
            .iter()
            .filter(|host| self.asks_user && host.user.is_none())
    }
}

/// Users entered for hosts that saved none (spec 0015, Model): one for
/// all, and one per host by [`Host::site`]. A host's own non-blank entry
/// wins, else the one for all; with both blank it keeps no user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Users {
    pub all: String,
    pub each: BTreeMap<String, String>,
}

impl Users {
    /// `parsed`'s hosts with the entered users set where they ask for one.
    fn hosts(&self, parsed: &Parsed) -> Vec<Host> {
        let mut hosts = parsed.hosts.clone();
        for host in hosts.iter_mut() {
            if !(parsed.asks_user && host.user.is_none()) {
                continue;
            }
            let own = self.each.get(&host.site()).map(|user| clean(user));
            let user = own
                .filter(|user| !user.is_empty())
                .unwrap_or_else(|| clean(&self.all));
            host.user = Some(user).filter(|user| !user.is_empty());
        }
        hosts
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
            ppk: None,
        })
    }

    /// The host with a `.ppk` key moved out of `identity_file`: OpenSSH
    /// can't read one, and the file is never opened (spec 0015, Model).
    fn without_ppk(mut self) -> Self {
        if self
            .identity_file
            .as_deref()
            .is_some_and(|key| key.to_lowercase().ends_with(".ppk"))
        {
            self.ppk = self.identity_file.take();
        }
        self
    }

    /// What tells the hosts of one file apart: the group and the name, as
    /// the app showed the site.
    pub fn site(&self) -> String {
        match &self.group {
            Some(group) => format!("{group} / {}", self.name),
            None => self.name.clone(),
        }
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

/// An app whose hosts the Import tab offers, with where it keeps them
/// (spec 0014, Model).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    MobaXterm,
    Tabby,
    PuTTY,
    WinSCP,
}

impl Source {
    pub const ALL: [Source; 4] = [
        Source::MobaXterm,
        Source::Tabby,
        Source::PuTTY,
        Source::WinSCP,
    ];

    /// The name the app goes by, which is also what [`Parsed::format`]
    /// says for its files.
    pub fn title(self) -> &'static str {
        match self {
            Source::MobaXterm => "MobaXterm",
            Source::Tabby => "Tabby",
            Source::PuTTY => "PuTTY",
            Source::WinSCP => "WinSCP",
        }
    }

    /// Where the installed app keeps its sessions in a file: under the
    /// user's config directory, or PuTTY's `~/.putty/sessions` on Linux.
    /// `None` where it doesn't run, or keeps them in the registry only.
    pub fn default_path(self) -> Option<PathBuf> {
        let dir = dirs::config_dir()?;
        match self {
            Source::MobaXterm if cfg!(windows) => Some(dir.join("MobaXterm").join("MobaXterm.ini")),
            Source::Tabby => Some(dir.join("tabby").join("config.yaml")),
            Source::PuTTY if cfg!(target_os = "linux") => {
                Some(dirs::home_dir()?.join(".putty").join("sessions"))
            }
            // Where WinSCP writes when told to keep its sites in a file.
            Source::WinSCP if cfg!(windows) => Some(dir.join("WinSCP.ini")),
            Source::MobaXterm | Source::PuTTY | Source::WinSCP => None,
        }
    }

    /// Where the installed app's sessions are, if anywhere: on Windows
    /// PuTTY's and WinSCP's registry key when it has sessions (WinSCP's
    /// `WinSCP.ini` otherwise), else [`Self::default_path`] if it exists.
    /// Reads the registry and the disk: never on the UI thread.
    pub fn find(self) -> Option<Place> {
        #[cfg(windows)]
        if let Some(key) = self.registry_key() {
            // A key that can't be read is shown as the error it is.
            let empty = registry::sessions(key)
                .is_ok_and(|sessions| sessions.iter().all(sessions::Session::is_defaults));
            if !empty {
                return Some(Place::Registry(self));
            }
        }
        self.default_path()
            .filter(|path| path.exists())
            .map(Place::File)
    }

    #[cfg(windows)]
    fn registry_key(self) -> Option<&'static str> {
        match self {
            Source::PuTTY => Some(sessions::PUTTY_KEY),
            Source::WinSCP => Some(sessions::WINSCP_KEY),
            Source::MobaXterm | Source::Tabby => None,
        }
    }

    /// What the card says when [`Self::find`] finds nothing.
    pub fn missing(self) -> String {
        match (self, self.default_path()) {
            (Source::PuTTY, _) if cfg!(windows) => "No PuTTY sessions found.".into(),
            (Source::WinSCP, _) if cfg!(windows) => "No WinSCP sites found.".into(),
            (_, Some(path)) => format!("Not found at {}", path.display()),
            (Source::MobaXterm, None) => {
                "MobaXterm runs on Windows: choose an exported .mxtsessions file.".into()
            }
            (Source::WinSCP, None) => {
                "WinSCP runs on Windows: choose a .reg export or WinSCP.ini.".into()
            }
            (Source::PuTTY, None) => "Choose a .reg export of PuTTY's sessions.".into(),
            (Source::Tabby, None) => "This system has no config directory.".into(),
        }
    }
}

/// What an import reads: a file (or PuTTY's sessions directory), or on
/// Windows an app's registry key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    File(PathBuf),
    #[cfg(windows)]
    Registry(Source),
}

impl fmt::Display for Place {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Place::File(path) => write!(f, "{}", path.display()),
            #[cfg(windows)]
            Place::Registry(source) => {
                let key = source.registry_key().unwrap_or_default();
                write!(f, "HKEY_CURRENT_USER\\{key}")
            }
        }
    }
}

/// What an import does, in counts, as the Import tab shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The format the file turned out to be, whatever card it came from.
    pub format: &'static str,
    pub new: usize,
    /// The distinct groups of the new hosts.
    pub groups: usize,
    pub duplicates: usize,
    pub skipped: usize,
    /// New hosts whose `.ppk` key was left out.
    pub without_key: usize,
    /// The hosts asked for a user, as read (spec 0015, flow 5).
    pub asking: Vec<Host>,
}

impl Outcome {
    fn of(parsed: &Parsed, plan: &Plan) -> Self {
        let mut groups: Vec<&str> = plan
            .imported
            .iter()
            .filter_map(|(_, host)| host.group.as_deref())
            .collect();
        groups.sort_unstable();
        groups.dedup();
        Self {
            format: parsed.format,
            new: plan.imported.len(),
            groups: groups.len(),
            duplicates: plan.duplicates.len(),
            skipped: parsed.skipped.len(),
            without_key: plan
                .imported
                .iter()
                .filter(|(_, host)| host.ppk.is_some())
                .count(),
            asking: parsed.asking().cloned().collect(),
        }
    }
}

/// Runs the import of `file` and prints its report. Returns the exit code.
/// With `may_ask` and stdin a terminal, hosts without a user are asked
/// for one first (spec 0015, flow 6); a caller whose shell doesn't wait
/// for it (a GUI-subsystem `tern.exe`) passes `false`.
pub fn run(file: &Path, dry_run: bool, may_ask: bool) -> i32 {
    let imported = load(&Place::File(file.to_owned())).and_then(|parsed| {
        let users = if may_ask && std::io::stdin().is_terminal() {
            ask(&parsed)
        } else {
            Users::default()
        };
        let (plan, path) = execute(&parsed, &users, dry_run)?;
        Ok(report(&parsed, &plan, &path, dry_run))
    });
    match imported {
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

/// Asks on stderr for the users of the hosts that have none: one for all,
/// or, left blank, one per host. A blank answer or the end of input leaves
/// them without.
fn ask(parsed: &Parsed) -> Users {
    let asking: Vec<&Host> = parsed.asking().collect();
    let mut users = Users::default();
    if asking.is_empty() {
        return users;
    }
    let mut lines = std::io::stdin().lock().lines();
    let mut prompt = |text: String| {
        eprint!("{text}");
        let _ = std::io::stderr().flush();
        lines.next()?.ok().map(|line| line.trim().to_owned())
    };
    let hosts = if asking.len() == 1 {
        "host has"
    } else {
        "hosts have"
    };
    let all = format!(
        "{} {hosts} no user. User for all (blank: ask per host): ",
        asking.len()
    );
    match prompt(all) {
        Some(all) if !all.is_empty() => users.all = all,
        Some(_) => {
            for host in asking {
                let Some(user) = prompt(format!("  {} ({}): ", host.site(), host.host)) else {
                    break;
                };
                users.each.insert(host.site(), user);
            }
        }
        None => {}
    }
    users
}

/// Imports what `place` holds (or, with `dry_run`, plans it against the
/// config as it is), with `users` for the hosts that have none, and says
/// what that does, in counts: the settings screen's Import tab (spec
/// 0014). The same code as the CLI's, so both write the same.
pub fn outcome(place: &Place, users: &Users, dry_run: bool) -> Result<Outcome, String> {
    let parsed = load(place)?;
    let (plan, _) = execute(&parsed, users, dry_run)?;
    Ok(Outcome::of(&parsed, &plan))
}

/// Reads and parses what `place` holds, told apart by content.
pub fn load(place: &Place) -> Result<Parsed, String> {
    match place {
        Place::File(path) if path.is_dir() => Ok(putty::parse(&sessions::dir(path)?)),
        Place::File(path) => parse(&read(path)?).ok_or_else(|| {
            format!(
                "{} is not a MobaXterm, Tabby, PuTTY or WinSCP file",
                path.display()
            )
        }),
        #[cfg(windows)]
        Place::Registry(source) => {
            let key = source.registry_key().unwrap_or_default();
            let found = registry::sessions(key)?;
            Ok(match source {
                Source::WinSCP => winscp::parse(&found),
                _ => putty::parse(&found),
            })
        }
    }
}

/// Plans `parsed`'s hosts, with `users` set, against `config.yaml` and,
/// unless `dry_run`, writes them there. Returns the plan and the config's
/// path.
fn execute(parsed: &Parsed, users: &Users, dry_run: bool) -> Result<(Plan, PathBuf), String> {
    let hosts = users.hosts(parsed);
    let path = config::default_path().ok_or("this system has no config directory")?;
    let aliases = ssh::config_path()
        .map(|path| ssh::config::aliases(&path).0)
        .unwrap_or_default();
    let plan_for = |text: &str| plan(&hosts, &config::parse(text).config, &aliases);
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
    Ok((plan, path))
}

/// Reads `file`, refusing one over [`MAX_BYTES`]. `reg export` writes
/// UTF-16 with a BOM; MobaXterm writes CP1252, so other text that isn't
/// UTF-8 is read as that.
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
    if let Some(utf16) = bytes.strip_prefix(&[0xff, 0xfe]) {
        let units: Vec<u16> = utf16
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_le_bytes(pair))
            .collect();
        return Ok(String::from_utf16_lossy(&units));
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

/// The hosts of `text`, told apart by content: a `.reg` export is PuTTY's
/// or WinSCP's by the key its sessions are under (`None` with neither),
/// `[Sessions\…]` sections are WinSCP.ini, `[Bookmarks…]` sections
/// MobaXterm, `profiles:` entries with a `type:` Tabby. `None` for
/// anything else.
pub fn parse(text: &str) -> Option<Parsed> {
    if let Some(found) = sessions::reg(text, sessions::PUTTY_KEY) {
        if !found.is_empty() {
            return Some(putty::parse(&found));
        }
        return sessions::reg(text, sessions::WINSCP_KEY)
            .filter(|found| !found.is_empty())
            .map(|found| winscp::parse(&found));
    }
    if let Some(found) = sessions::ini(text) {
        Some(winscp::parse(&found))
    } else if mobaxterm::detect(text) {
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
    let ppk: Vec<_> = plan
        .imported
        .iter()
        .filter_map(|(_, host)| Some((host, host.ppk.as_deref()?)))
        .collect();
    if !ppk.is_empty() {
        out.push_str(
            "Without key (PuTTY format, which ssh can't read; convert it, \
             then set the key on the profile):\n",
        );
        for (host, key) in ppk {
            let pem = match key.len().checked_sub(4) {
                Some(stem) if key.is_char_boundary(stem) => format!("{}.pem", &key[..stem]),
                _ => format!("{key}.pem"),
            };
            out.push_str(&line(
                host,
                format!("  puttygen \"{key}\" -O private-openssh -o \"{pem}\""),
            ));
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
            ppk: None,
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
                    ppk: None,
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
                    ppk: None,
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

    /// What the Import tab would show for `text` planned into `config`.
    fn outcome_of(text: &str, config: &str) -> Outcome {
        let parsed = parse(text).expect("parses");
        let plan = plan(&parsed.hosts, &config::parse(config).config, &[]);
        Outcome::of(&parsed, &plan)
    }

    /// Spec 0014 AC1, flow 3: the card's count of a MobaXterm file with
    /// folders. A host outside any folder is new but adds no group; the
    /// RDP session and the one behind a jump host are skipped.
    #[test]
    fn the_tab_counts_new_hosts_groups_and_skips_of_a_mobaxterm_file() {
        assert_eq!(
            outcome_of(&mobaxterm_ini(), TEMPLATE),
            Outcome {
                format: "MobaXterm",
                new: 2,
                groups: 1,
                duplicates: 0,
                skipped: 2,
                without_key: 0,
                asking: Vec::new(),
            }
        );
    }

    /// Spec 0014 flow 3: groups are counted once each, and only for hosts
    /// that are new; a duplicate's group doesn't count.
    #[test]
    fn groups_are_counted_once_and_only_for_new_hosts() {
        let in_group = |name: &str, host_name: &str, group: &str| Host {
            group: Some(group.to_owned()),
            ..host(name, host_name, None)
        };
        let hosts = [
            in_group("a", "a.example.com", "Prod"),
            in_group("b", "b.example.com", "Prod"),
            in_group("c", "c.example.com", "Staging"),
            in_group("old", "old.example.com", "Legacy"),
            host("loose", "loose.example.com", None),
        ];
        let existing = "profiles:\n  old:\n    type: ssh\n    host: old.example.com\n";
        let plan = plan(&hosts, &config::parse(existing).config, &[]);
        let parsed = Parsed {
            format: "MobaXterm",
            hosts: hosts.to_vec(),
            skipped: Vec::new(),
            asks_user: false,
        };
        let outcome = Outcome::of(&parsed, &plan);
        assert_eq!((outcome.new, outcome.groups), (4, 2));
        assert_eq!(outcome.duplicates, 1);
    }

    /// Spec 0014 AC2: the Tabby card counts the SSH profiles and its local
    /// and serial profiles as skipped.
    #[test]
    fn the_tab_counts_tabby_local_and_serial_profiles_as_skipped() {
        let text = "profiles:
  - type: local
    name: Zsh
    options: { command: zsh }
  - type: serial
    name: Console
    options: { port: /dev/ttyUSB0, baudrate: 115200 }
  - type: ssh
    name: Box
    options: { host: box.example.com }
";
        let outcome = outcome_of(text, TEMPLATE);
        assert_eq!(outcome.format, Source::Tabby.title());
        assert_eq!((outcome.new, outcome.skipped), (1, 2));
    }

    /// Spec 0014 AC3, flow 4: after an import, the card plans the same
    /// file against the written config and finds nothing new, and a second
    /// import leaves the text byte for byte as it was.
    #[test]
    fn after_an_import_the_card_finds_nothing_new() {
        for file in [mobaxterm_ini(), TABBY.to_owned()] {
            let parsed = parse(&file).expect("parses");
            let first = plan(&parsed.hosts, &config::parse(TEMPLATE).config, &[]);
            let written = first.apply(TEMPLATE).expect("applies");
            let again = outcome_of(&file, &written);
            assert_eq!(again.new, 0);
            assert_eq!(again.duplicates, parsed.hosts.len());
            assert_eq!(again.skipped, parsed.skipped.len());
            let second = plan(&parsed.hosts, &config::parse(&written).config, &[]);
            assert_eq!(second.apply(&written).as_deref(), Ok(written.as_str()));
        }
    }

    /// Spec 0014 AC6, Model: the outcome names the format the file is, so
    /// a Tabby config chosen on the MobaXterm card is told apart.
    #[test]
    fn a_tabby_file_says_tabby_whatever_card_it_came_from() {
        assert_eq!(outcome_of(TABBY, TEMPLATE).format, Source::Tabby.title());
        assert_eq!(
            outcome_of(&mobaxterm_ini(), TEMPLATE).format,
            Source::MobaXterm.title()
        );
    }

    /// Spec 0014 AC5: an exported `.mxtsessions` file, in MobaXterm's
    /// CP1252, reads and counts like the INI it came from.
    #[test]
    fn an_mxtsessions_export_imports_like_the_ini() {
        let ini = mobaxterm_ini().replace("Prod\\Web", "Café");
        let cp1252: Vec<u8> = ini
            .chars()
            .map(|c| u8::try_from(u32::from(c)).expect("Latin-1"))
            .collect();
        let export = TempFile::new("export.mxtsessions", &cp1252);
        let text = read(&export.0).expect("reads");
        let hosts = |text: &str| parse(text).expect("MobaXterm").hosts;
        assert_eq!(hosts(&text), hosts(&ini));
        assert!(
            hosts(&text)
                .iter()
                .any(|h| h.group.as_deref() == Some("Café"))
        );
        assert_eq!(outcome_of(&text, TEMPLATE), outcome_of(&ini, TEMPLATE));
    }

    /// Spec 0014 flow 6: a config that already has a notice still gets a
    /// count and an import; only a write that adds a notice is refused.
    #[test]
    fn a_config_with_notices_still_counts_and_imports() {
        let existing = format!("default_profile: nowhere\n{TEMPLATE}");
        assert!(!config::parse(&existing).notices.is_empty(), "no notice");
        let parsed = parse(TABBY).expect("Tabby");
        let plan = plan(&parsed.hosts, &config::parse(&existing).config, &[]);
        assert_eq!(Outcome::of(&parsed, &plan).new, 2);
        assert!(plan.apply(&existing).is_ok());
    }

    /// Spec 0014 AC8: the tab's counts and the CLI's report describe the
    /// same plan: the same number imported and skipped, the same format.
    #[test]
    fn the_tab_and_the_cli_report_the_same_import() {
        for file in [mobaxterm_ini(), TABBY.to_owned()] {
            let parsed = parse(&file).expect("parses");
            let plan = plan(&parsed.hosts, &config::parse(TEMPLATE).config, &[]);
            let outcome = Outcome::of(&parsed, &plan);
            let report = report(&parsed, &plan, Path::new("config.yaml"), false);
            let head = format!("Imported {} hosts from {}", outcome.new, outcome.format);
            assert!(report.starts_with(&head), "{report}");
            let skipped: usize = report
                .lines()
                .filter_map(|line| line.trim().split_once(" session"))
                .filter_map(|(n, _)| n.parse::<usize>().ok())
                .sum();
            assert_eq!(skipped, outcome.skipped, "{report}");
        }
    }

    /// Spec 0014 Model: both apps are looked for under the user's config
    /// directory; MobaXterm only where it runs.
    #[test]
    fn each_source_is_looked_for_at_its_default_place() {
        let dir = dirs::config_dir().expect("a config dir");
        assert_eq!(
            Source::Tabby.default_path(),
            Some(dir.join("tabby").join("config.yaml"))
        );
        let moba = Source::MobaXterm.default_path();
        if cfg!(windows) {
            assert_eq!(moba, Some(dir.join("MobaXterm").join("MobaXterm.ini")));
        } else {
            assert_eq!(moba, None);
        }
    }

    // Spec 0015: PuTTY and WinSCP.

    /// Where PuTTY keeps its sessions, as `reg export` names the key.
    const PUTTY: &str = r"Software\SimonTatham\PuTTY\Sessions";
    /// Where WinSCP keeps its sites, as `reg export` names the key.
    const WINSCP: &str = r"Software\Martin Prikryl\WinSCP 2\Sessions";

    /// A `.reg` export of `key` as `reg export` writes it: the header, the
    /// parent key, then one key per session with its value lines, CRLF.
    fn reg_export(key: &str, sessions: &[(&str, &str)]) -> String {
        let mut out = format!(
            "Windows Registry Editor Version 5.00\r\n\r\n[HKEY_CURRENT_USER\\{key}]\r\n\r\n"
        );
        for (name, values) in sessions {
            out.push_str(&format!("[HKEY_CURRENT_USER\\{key}\\{name}]\r\n"));
            for line in values.lines().map(str::trim).filter(|l| !l.is_empty()) {
                out.push_str(line);
                out.push_str("\r\n");
            }
            out.push_str("\r\n");
        }
        out
    }

    /// `text` as `reg export` writes it to disk: UTF-16LE with a BOM.
    fn utf16(text: &str) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        bytes
    }

    /// A PuTTY export with the template, two SSH sessions (one with a
    /// `user@host`, one with an escaped name and a key path with
    /// backslashes) and a telnet session.
    fn putty_reg() -> String {
        reg_export(
            PUTTY,
            &[
                (
                    "Default%20Settings",
                    r#""HostName"="template.example.com"
                       "PortNumber"=dword:00000016
                       "Protocol"="ssh""#,
                ),
                (
                    "my%20box",
                    r#""HostName"="box.example.com"
                       "PortNumber"=dword:00000016
                       "UserName"="deploy"
                       "Protocol"="ssh"
                       "PublicKeyFile"="C:\\Users\\me\\.ssh\\id_box"
                       "ProxyMethod"=dword:00000000"#,
                ),
                (
                    "router",
                    r#""HostName"="router.example.com"
                       "PortNumber"=dword:00000017
                       "Protocol"="telnet""#,
                ),
                (
                    "web",
                    r#""HostName"="admin@web.example.com"
                       "PortNumber"=dword:000008ae
                       "Protocol"="ssh""#,
                ),
            ],
        )
    }

    /// The hosts [`putty_reg`] holds.
    fn putty_hosts() -> Vec<Host> {
        vec![
            Host {
                identity_file: Some(r"C:\Users\me\.ssh\id_box".into()),
                ..host("my box", "box.example.com", Some("deploy"))
            },
            Host {
                port: Some(2222),
                ..host("web", "web.example.com", Some("admin"))
            },
        ]
    }

    /// A unique directory in the temp dir, removed with its files on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("tern-import-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Spec 0015 AC2, Model: `reg export` output (UTF-16LE with a BOM,
    /// CRLF, `dword:` ports, `\\` escapes) loads as PuTTY with the same
    /// hosts and skips as its UTF-8 text.
    #[test]
    fn a_utf16_reg_export_loads_like_its_utf8_text() {
        let text = putty_reg();
        let wide = TempFile::new("putty-utf16.reg", &utf16(&text));
        let narrow = TempFile::new("putty-utf8.reg", text.as_bytes());
        for file in [&wide, &narrow] {
            let parsed = load(&Place::File(file.0.clone())).expect("a PuTTY export");
            assert_eq!(parsed.format, "PuTTY");
            assert_eq!(parsed.hosts, putty_hosts());
            assert_eq!(parsed.skipped, [Skip::NotSsh]);
        }
    }

    /// Spec 0015 AC1, AC6: the template session is neither a host nor a
    /// skip, so the card counts only the real sessions.
    #[test]
    fn putty_default_settings_is_never_counted() {
        let outcome = outcome_of(&putty_reg(), TEMPLATE);
        assert_eq!((outcome.new, outcome.skipped), (2, 1));
        let parsed = parse(&putty_reg()).expect("PuTTY");
        assert!(
            parsed
                .hosts
                .iter()
                .all(|h| h.host != "template.example.com"),
            "{:?}",
            parsed.hosts
        );
    }

    /// Spec 0015 AC6, Model: a PuTTY session behind an SSH proxy is behind
    /// a jump host; any other proxy, including the pre-0.77 `ProxyType`,
    /// is a proxy; telnet is not SSH and no `Protocol` is SSH.
    #[test]
    fn putty_sessions_are_skipped_by_protocol_and_proxy() {
        let text = reg_export(
            PUTTY,
            &[
                (
                    "jump",
                    "\"HostName\"=\"a\"\r\n\"ProxyMethod\"=dword:00000006",
                ),
                (
                    "socks",
                    "\"HostName\"=\"b\"\r\n\"ProxyMethod\"=dword:00000002",
                ),
                ("old", "\"HostName\"=\"c\"\r\n\"ProxyType\"=dword:00000001"),
                ("tel", "\"HostName\"=\"d\"\r\n\"Protocol\"=\"telnet\""),
                ("plain", "\"HostName\"=\"e.example.com\""),
            ],
        );
        let parsed = parse(&text).expect("PuTTY");
        assert_eq!(
            parsed.skipped,
            [Skip::JumpHost, Skip::Proxy, Skip::Proxy, Skip::NotSsh]
        );
        assert_eq!(parsed.hosts, [host("plain", "e.example.com", None)]);
        let plan = plan(&parsed.hosts, &config::parse(TEMPLATE).config, &[]);
        let report = report(&parsed, &plan, Path::new("config.yaml"), false);
        assert!(report.contains("1 session: behind a jump host"), "{report}");
    }

    /// WinSCP sites `Prod/Web/app1` (SFTP, with a saved password) and
    /// `ftp-box` (FTP) as a `.reg` export.
    fn winscp_reg() -> String {
        reg_export(
            WINSCP,
            &[
                (
                    "Prod/Web/app1",
                    r#""HostName"="app1.example.com"
                       "PortNumber"=dword:00000016
                       "UserName"="deploy"
                       "Password"="A35C4D5B-SECRET-PASSWORD"
                       "PublicKeyFile"="C:%5Ckeys%5Capp1"
                       "FSProtocol"=dword:00000002"#,
                ),
                (
                    "ftp-box",
                    r#""HostName"="ftp.example.com"
                       "FSProtocol"=dword:00000005"#,
                ),
            ],
        )
    }

    /// The same sites as a `WinSCP.ini`, values `%`-escaped.
    const WINSCP_INI: &str = "\
[Configuration\\Interface]\r
Theme=Dark\r
[Sessions\\Prod/Web/app1]\r
HostName=app1.example.com\r
PortNumber=22\r
UserName=deploy\r
Password=A35C4D5B-SECRET-PASSWORD\r
PublicKeyFile=C:%5Ckeys%5Capp1\r
FSProtocol=2\r
[Sessions\\ftp-box]\r
HostName=ftp.example.com\r
FSProtocol=5\r
";

    /// Spec 0015 AC3: from a `.reg` export and from `WinSCP.ini`, `app1`
    /// lands in group `Prod / Web` and `ftp-box` is skipped as not SSH.
    #[test]
    fn winscp_sites_import_with_folders_from_reg_and_ini() {
        let expected = Host {
            identity_file: Some(r"C:\keys\app1".into()),
            group: Some("Prod / Web".into()),
            ..host("app1", "app1.example.com", Some("deploy"))
        };
        let reg = TempFile::new("winscp.reg", &utf16(&winscp_reg()));
        let ini = TempFile::new("WinSCP.ini", WINSCP_INI.as_bytes());
        for file in [&reg, &ini] {
            let parsed = load(&Place::File(file.0.clone())).expect("a WinSCP file");
            assert_eq!(parsed.format, "WinSCP");
            assert_eq!(parsed.hosts, std::slice::from_ref(&expected));
            assert_eq!(parsed.skipped, [Skip::NotSsh]);
        }
    }

    /// Spec 0015 Model: escaped folder and site names are decoded, and
    /// SCP sites import like SFTP ones.
    #[test]
    fn winscp_escaped_folders_and_names_are_decoded() {
        let parsed =
            parse("[Sessions\\My%20Folder/my%20site]\nHostName=a.example.com\nFSProtocol=0\n")
                .expect("WinSCP.ini");
        assert_eq!(parsed.hosts.len(), 1);
        assert_eq!(parsed.hosts[0].name, "my site");
        assert_eq!(parsed.hosts[0].group.as_deref(), Some("My Folder"));
    }

    /// Spec 0015 AC6, Model: a WinSCP tunnel is a jump host, another proxy
    /// a proxy, a workspace entry a workspace; its template is no host.
    #[test]
    fn winscp_sites_are_skipped_by_tunnel_proxy_and_workspace() {
        let parsed = parse(
            "[Sessions\\Default%20Settings]\nHostName=template.example.com\n\
             [Sessions\\tunneled]\nHostName=a.example.com\nTunnel=1\n\
             [Sessions\\proxied]\nHostName=b.example.com\nProxyMethod=2\n\
             [Sessions\\Workspace/0000]\nHostName=c.example.com\nIsWorkspace=1\n",
        )
        .expect("WinSCP.ini");
        assert!(parsed.hosts.is_empty(), "{:?}", parsed.hosts);
        assert_eq!(
            parsed.skipped,
            [Skip::JumpHost, Skip::Proxy, Skip::Workspace]
        );
    }

    /// A PuTTY export with a session that names a `.ppk` key.
    fn putty_ppk_reg() -> String {
        reg_export(
            PUTTY,
            &[(
                "keyed",
                r#""HostName"="keyed.example.com"
                   "UserName"="ops"
                   "PublicKeyFile"="C:\\keys\\keyed.PPK""#,
            )],
        )
    }

    /// Spec 0015 AC4: a WinSCP password reaches neither the parsed hosts,
    /// the config text nor the report; a `.ppk` key is left out of the
    /// profile, counted, and named in the report with the `puttygen` hint.
    #[test]
    fn no_password_and_no_ppk_key_reach_the_config() {
        for file in [winscp_reg(), WINSCP_INI.to_owned(), putty_ppk_reg()] {
            let parsed = parse(&file).expect("parses");
            let plan = plan(&parsed.hosts, &config::parse(TEMPLATE).config, &[]);
            let text = plan.apply(TEMPLATE).expect("applies");
            let report = report(&parsed, &plan, Path::new("config.yaml"), false);
            for out in [&text, &report, &format!("{parsed:?}")] {
                assert!(!out.contains("SECRET"), "a password leaked:\n{out}");
                assert!(!out.contains("password:"), "a password key:\n{out}");
                assert!(!out.contains(".PPK\n"), "a .ppk key path:\n{out}");
            }
        }

        let parsed = parse(&putty_ppk_reg()).expect("PuTTY");
        assert_eq!(parsed.hosts[0].identity_file, None);
        let plan = plan(&parsed.hosts, &config::parse(TEMPLATE).config, &[]);
        let text = plan.apply(TEMPLATE).expect("applies");
        assert!(!text.contains("identity_file"), "{text}");
        assert_eq!(Outcome::of(&parsed, &plan).without_key, 1);
        let report = report(&parsed, &plan, Path::new("config.yaml"), false);
        assert!(
            report.contains(
                r#"keyed  ops@keyed.example.com  puttygen "C:\keys\keyed.PPK" -O private-openssh -o "C:\keys\keyed.pem""#
            ),
            "{report}"
        );
    }

    /// Spec 0015 AC5, flow 7: a host saved in both PuTTY and WinSCP is
    /// imported by the first source; the second counts it as a duplicate.
    #[test]
    fn a_host_in_putty_and_winscp_imports_once() {
        let putty = putty_reg();
        let winscp = "[Sessions\\Boxes/box]\nHostName=BOX.example.com\nUserName=deploy\n";
        let first = parse(&putty).expect("PuTTY");
        let written = plan(&first.hosts, &config::parse(TEMPLATE).config, &[])
            .apply(TEMPLATE)
            .expect("applies");
        let second = outcome_of(winscp, &written);
        assert_eq!((second.new, second.duplicates), (0, 1));
    }

    /// Three user-less PuTTY sessions `a`, `b`, `c` and one that saved
    /// `carol`.
    fn userless_putty() -> Parsed {
        parse(&reg_export(
            PUTTY,
            &[
                ("a", "\"HostName\"=\"a.example.com\""),
                ("b", "\"HostName\"=\"b.example.com\""),
                ("c", "\"HostName\"=\"c.example.com\""),
                (
                    "d",
                    "\"HostName\"=\"d.example.com\"\r\n\"UserName\"=\"carol\"",
                ),
            ],
        ))
        .expect("PuTTY")
    }

    fn users_of(hosts: &[Host]) -> Vec<Option<&str>> {
        hosts.iter().map(|h| h.user.as_deref()).collect()
    }

    /// Spec 0015 AC7: `alice` for all and `bob` for the second gives
    /// alice, bob, alice; the host that saved `carol` keeps her and is
    /// never asked.
    #[test]
    fn a_host_s_own_user_wins_over_the_one_for_all() {
        let parsed = userless_putty();
        let users = Users {
            all: "alice".into(),
            each: BTreeMap::from([("b".to_owned(), "bob".to_owned())]),
        };
        assert_eq!(
            users_of(&users.hosts(&parsed)),
            [Some("alice"), Some("bob"), Some("alice"), Some("carol")]
        );
        let plan = plan(&parsed.hosts, &config::parse(TEMPLATE).config, &[]);
        let asking: Vec<_> = Outcome::of(&parsed, &plan)
            .asking
            .iter()
            .map(Host::site)
            .collect();
        assert_eq!(asking, ["a", "b", "c"]);
    }

    /// Spec 0015 AC7: with every entry blank (or only whitespace), the
    /// hosts are imported without `user:`.
    #[test]
    fn blank_users_import_without_a_user() {
        let parsed = userless_putty();
        let blank = Users {
            all: "  ".into(),
            each: BTreeMap::from([("b".to_owned(), String::new())]),
        };
        for users in [Users::default(), blank] {
            let hosts = users.hosts(&parsed);
            assert_eq!(users_of(&hosts), [None, None, None, Some("carol")]);
            let text = plan(&hosts, &config::parse(TEMPLATE).config, &[])
                .apply(TEMPLATE)
                .expect("applies");
            assert_eq!(text.matches("user:").count(), 1, "{text}");
        }
    }

    /// Spec 0015 Model: an entered user is cleaned like a read one.
    #[test]
    fn an_entered_user_is_cleaned() {
        let users = Users {
            all: " al\x1bice \n".into(),
            each: BTreeMap::new(),
        };
        assert_eq!(
            users.hosts(&userless_putty())[0].user.as_deref(),
            Some("alice")
        );
    }

    /// Spec 0015 Model: per-host users go by the full site name, so two
    /// WinSCP sites of the same name in different folders are told apart.
    #[test]
    fn per_host_users_go_by_group_and_name() {
        let parsed = parse(
            "[Sessions\\A/box]\nHostName=a.example.com\n[Sessions\\B/box]\nHostName=b.example.com\n",
        )
        .expect("WinSCP.ini");
        let users = Users {
            all: String::new(),
            each: BTreeMap::from([("B / box".to_owned(), "bob".to_owned())]),
        };
        assert_eq!(users_of(&users.hosts(&parsed)), [None, Some("bob")]);
    }

    /// Spec 0015 Model: MobaXterm and Tabby hosts are never asked for a
    /// user, and a user for all doesn't touch them.
    #[test]
    fn mobaxterm_and_tabby_hosts_are_never_asked() {
        let moba = format!(
            "[Bookmarks]\nSubRep=\nImgNum=42\nbare={}\n",
            moba("0", "bare.example.com", "22", "", "", "")
        );
        let users = Users {
            all: "alice".into(),
            each: BTreeMap::new(),
        };
        for file in [moba, TABBY.to_owned()] {
            let parsed = parse(&file).expect("parses");
            assert!(outcome_of(&file, TEMPLATE).asking.is_empty());
            assert_eq!(users.hosts(&parsed), parsed.hosts);
        }
    }

    /// Spec 0015 Model: users are set before the plan, so a user-less host
    /// that duplicates a user-less profile is new once given a user.
    #[test]
    fn users_are_set_before_duplicates_are_judged() {
        let existing = "profiles:\n  a:\n    type: ssh\n    host: a.example.com\n";
        let config = config::parse(existing).config;
        let parsed = parse(&reg_export(
            PUTTY,
            &[("a", "\"HostName\"=\"a.example.com\"")],
        ))
        .expect("PuTTY");
        let without = plan(&Users::default().hosts(&parsed), &config, &[]);
        assert_eq!((without.imported.len(), without.duplicates.len()), (0, 1));
        let users = Users {
            all: "alice".into(),
            each: BTreeMap::new(),
        };
        let with = plan(&users.hosts(&parsed), &config, &[]);
        assert_eq!((with.imported.len(), with.duplicates.len()), (1, 0));
    }

    /// Spec 0015 AC8, Model: a `~/.putty/sessions` directory (a file per
    /// session, named by its escaped name, of `Key=value` lines) imports
    /// its SSH sessions; the template and a telnet session don't.
    #[test]
    fn a_putty_sessions_directory_imports_its_ssh_sessions() {
        let dir = TempDir::new("putty-sessions");
        let files = [
            ("Default%20Settings", "HostName=template.example.com\n"),
            (
                "my%20box",
                "HostName=box.example.com\nPortNumber=2222\nUserName=deploy\nProtocol=ssh\n",
            ),
            ("router", "HostName=router.example.com\nProtocol=telnet\n"),
        ];
        for (name, text) in files {
            std::fs::write(dir.0.join(name), text).expect("session file");
        }
        let parsed = load(&Place::File(dir.0.clone())).expect("a sessions directory");
        assert_eq!(parsed.format, "PuTTY");
        assert_eq!(
            parsed.hosts,
            [Host {
                port: Some(2222),
                ..host("my box", "box.example.com", Some("deploy"))
            }]
        );
        assert_eq!(parsed.skipped, [Skip::NotSsh]);
    }

    /// Spec 0015 flow 1: a `.reg` export of neither app's sessions is not a
    /// known format, and loading it is an error naming the file.
    #[test]
    fn a_reg_export_of_another_key_is_not_a_known_format() {
        let text = reg_export(
            r"Software\Other\App\Sessions",
            &[("x", "\"HostName\"=\"x\"")],
        );
        assert!(parse(&text).is_none());
        let file = TempFile::new("other.reg", &utf16(&text));
        let error = load(&Place::File(file.0.clone())).expect_err("unknown");
        assert!(error.contains("other.reg"), "{error}");
    }

    /// Spec 0015 Module surface, flows 2–3: PuTTY's default place on Linux
    /// is `~/.putty/sessions`, WinSCP's on Windows `WinSCP.ini` in the
    /// roaming config dir; with nothing found, the cards say so per
    /// platform.
    #[test]
    fn putty_and_winscp_are_looked_for_per_platform() {
        let putty = Source::PuTTY.default_path();
        let winscp = Source::WinSCP.default_path();
        if cfg!(windows) {
            assert_eq!(putty, None);
            let dir = dirs::config_dir().expect("a config dir");
            assert_eq!(winscp, Some(dir.join("WinSCP.ini")));
            assert_eq!(Source::PuTTY.missing(), "No PuTTY sessions found.");
            assert_eq!(Source::WinSCP.missing(), "No WinSCP sites found.");
        } else {
            assert_eq!(winscp, None);
            assert!(Source::WinSCP.missing().contains("WinSCP runs on Windows"));
        }
        if cfg!(target_os = "linux") {
            let home = dirs::home_dir().expect("a home dir");
            assert_eq!(putty, Some(home.join(".putty").join("sessions")));
        }
    }
}
