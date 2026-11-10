//! PuTTY's and WinSCP's saved sessions as named sets of values (spec 0015,
//! Model), whatever they were read from: the registry, a `.reg` export,
//! `WinSCP.ini` or PuTTY's `~/.putty/sessions` directory. The parsers only
//! see this list, so a click on a card and a CLI import of `reg export`
//! output go through the same code.
//!
//! Only the values in [`KEEP`] are held; every other value, `Password`
//! and WinSCP's master-password data among them, is passed over unparsed.

use super::MAX_BYTES;
use std::io::Read;
use std::path::Path;

/// The values the parsers read. Nothing else is kept.
const KEEP: [&str; 10] = [
    "HostName",
    "PortNumber",
    "UserName",
    "PublicKeyFile",
    "Protocol",
    "ProxyMethod",
    "ProxyType",
    "FSProtocol",
    "Tunnel",
    "IsWorkspace",
];

/// Where PuTTY keeps its sessions, under `HKEY_CURRENT_USER`.
pub const PUTTY_KEY: &str = r"Software\SimonTatham\PuTTY\Sessions";
/// Where WinSCP keeps its sites, under `HKEY_CURRENT_USER`.
pub const WINSCP_KEY: &str = r"Software\Martin Prikryl\WinSCP 2\Sessions";
/// Both apps' template session, stored under this escaped name. Not a host.
pub const DEFAULTS: &str = "Default%20Settings";

/// One saved session: its name as stored (still `%`-escaped), and the
/// [`KEEP`] values it has.
#[derive(Debug, Default)]
pub struct Session {
    pub name: String,
    values: Vec<(&'static str, String)>,
}

impl Session {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            values: Vec::new(),
        }
    }

    /// The names of the values worth querying, for a reader that can ask
    /// for them one by one (the registry).
    #[cfg(windows)]
    pub fn wanted() -> &'static [&'static str] {
        &KEEP
    }

    /// Keeps `value` if `key` is one the parsers read; drops it otherwise.
    pub fn set(&mut self, key: &str, value: String) {
        if let Some(kept) = KEEP.iter().find(|kept| kept.eq_ignore_ascii_case(key)) {
            self.values.push((kept, value));
        }
    }

    /// The value of `key`, `""` if it isn't set.
    pub fn get(&self, key: &str) -> &str {
        self.values
            .iter()
            .find(|(kept, _)| *kept == key)
            .map_or("", |(_, value)| value.as_str())
    }

    /// The value of `key` as a number; `None` if it isn't set, `Err` if it
    /// isn't one.
    pub fn number(&self, key: &str) -> Result<Option<u32>, ()> {
        match self.get(key).trim() {
            "" => Ok(None),
            text => text.parse().map(Some).map_err(|_| ()),
        }
    }

    /// The template session, which is no host.
    pub fn is_defaults(&self) -> bool {
        self.name == DEFAULTS
    }
}

/// `%XX`-escaped text, as both apps write session names (and WinSCP.ini
/// its values), decoded. A `%` not followed by two hex digits stays.
pub fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The sessions under `HKEY_CURRENT_USER\<under>` in a `.reg` export;
/// `None` if `text` isn't one (no `Windows Registry Editor` or `REGEDIT4`
/// header).
pub fn reg(text: &str, under: &str) -> Option<Vec<Session>> {
    let header = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    if !(header.starts_with("Windows Registry Editor") || header.starts_with("REGEDIT4")) {
        return None;
    }
    let marker = format!("\\{}\\", under.to_lowercase());
    let mut sessions = Vec::new();
    let mut current: Option<Session> = None;
    for line in text.lines().map(str::trim) {
        if let Some(key) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sessions.extend(current.take());
            let lower = key.to_lowercase();
            // Byte offsets of the lowercase text match `key`'s for the
            // ASCII marker; a non-ASCII key above it is no session anyway.
            current = lower
                .find(&marker)
                .filter(|_| lower.len() == key.len())
                .map(|at| &key[at + marker.len()..])
                .filter(|name| !name.is_empty() && !name.contains('\\'))
                .map(Session::new);
            continue;
        }
        let (Some(session), Some(rest)) = (current.as_mut(), line.strip_prefix('"')) else {
            continue;
        };
        let Some((name, raw)) = rest.split_once("\"=") else {
            continue;
        };
        if let Some(value) = reg_value(raw) {
            session.set(name, value);
        }
    }
    sessions.extend(current);
    Some(sessions)
}

/// A `.reg` value as text: a quoted string unescaped, a `dword:` as its
/// decimal number. Other types (binary, multi-string) aren't read.
fn reg_value(raw: &str) -> Option<String> {
    if let Some(dword) = raw.strip_prefix("dword:") {
        return u32::from_str_radix(dword.trim(), 16)
            .ok()
            .map(|n| n.to_string());
    }
    let quoted = raw.strip_prefix('"')?.strip_suffix('"')?;
    let mut value = String::with_capacity(quoted.len());
    let mut chars = quoted.chars();
    while let Some(c) = chars.next() {
        value.push(match c {
            '\\' => chars.next().unwrap_or('\\'),
            c => c,
        });
    }
    Some(value)
}

/// The sites of a `WinSCP.ini`: its `[Sessions\<name>]` sections. `None`
/// if it has none.
pub fn ini(text: &str) -> Option<Vec<Session>> {
    let mut sessions = Vec::new();
    let mut current: Option<Session> = None;
    let mut found = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            sessions.extend(current.take());
            current = line
                .strip_prefix("[Sessions\\")
                .and_then(|l| l.strip_suffix(']'))
                .map(Session::new);
            found |= current.is_some();
        } else if let (Some(session), Some((key, value))) = (current.as_mut(), line.split_once('='))
        {
            session.set(key.trim(), value.to_owned());
        }
    }
    sessions.extend(current);
    found.then_some(sessions)
}

/// The sessions of PuTTY's `~/.putty/sessions` directory: a file per
/// session, named by its escaped name, of `Key=value` lines. Fails over
/// [`MAX_BYTES`] in all.
pub fn dir(path: &Path) -> Result<Vec<Session>, String> {
    let fail = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut sessions = Vec::new();
    let mut total = 0;
    for entry in std::fs::read_dir(path).map_err(fail)? {
        let entry = entry.map_err(fail)?;
        if !entry.file_type().map_err(fail)?.is_file() {
            continue;
        }
        let mut bytes = Vec::new();
        std::fs::File::open(entry.path())
            .and_then(|file| file.take(MAX_BYTES + 1 - total).read_to_end(&mut bytes))
            .map_err(fail)?;
        total += bytes.len() as u64;
        if total > MAX_BYTES {
            return Err(format!("{} holds over 10 MB", path.display()));
        }
        let mut session = Session::new(&entry.file_name().to_string_lossy());
        for line in String::from_utf8_lossy(&bytes).lines() {
            if let Some((key, value)) = line.split_once('=') {
                session.set(key, value.to_owned());
            }
        }
        sessions.push(session);
    }
    sessions.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Session names come back as written: spaces, `%` and non-ASCII
    /// (UTF-8 bytes) decoded, a stray `%` left alone.
    #[test]
    fn escaped_names_are_decoded() {
        assert_eq!(unescape("my%20box"), "my box");
        assert_eq!(unescape("100%25"), "100%");
        assert_eq!(unescape("M%C3%BCnchen"), "München");
        assert_eq!(unescape("50%"), "50%");
        assert_eq!(unescape("%zz"), "%zz");
    }

    /// Only the values the parsers read are held: a password in the file
    /// never reaches a session.
    #[test]
    fn values_outside_keep_are_dropped() {
        let sessions = reg(
            "Windows Registry Editor Version 5.00\r\n\r\n\
             [HKEY_CURRENT_USER\\Software\\Martin Prikryl\\WinSCP 2\\Sessions\\a]\r\n\
             \"HostName\"=\"a.example.com\"\r\n\
             \"Password\"=\"A35C-SECRET\"\r\n",
            WINSCP_KEY,
        )
        .expect("a .reg");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].get("HostName"), "a.example.com");
        assert!(!format!("{sessions:?}").contains("SECRET"));
    }
}
