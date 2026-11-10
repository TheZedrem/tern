//! WinSCP's saved sites (spec 0015): SFTP and SCP sites become hosts, a
//! site `Folder/Sub/name` named `name` in group `Folder / Sub`. Workspace
//! entries copy sites the user also has and are skipped. WinSCP.ini writes
//! its values `%`-escaped (`C:%5Ckeys`), so values are decoded too; the
//! registry's raw values hold no `%XX` in practice.

use super::sessions::{Session, unescape};
use super::{Host, Parsed, Skip};

pub fn parse(sessions: &[Session]) -> Parsed {
    let mut parsed = Parsed {
        format: "WinSCP",
        asks_user: true,
        ..Parsed::default()
    };
    for session in sessions.iter().filter(|s| !s.is_defaults()) {
        parsed.push(host(session));
    }
    parsed
}

fn host(session: &Session) -> Result<Host, Skip> {
    if session.get("IsWorkspace") == "1" {
        return Err(Skip::Workspace);
    }
    // `FSProtocol`: 0 SCP, 1 SFTP (the default), 2 SFTP only; 5 FTP,
    // 6 WebDAV, 7 S3.
    if !matches!(session.number("FSProtocol"), Ok(None | Some(0..=2))) {
        return Err(Skip::NotSsh);
    }
    if session.get("Tunnel") == "1" {
        return Err(Skip::JumpHost);
    }
    if !matches!(session.number("ProxyMethod"), Ok(None | Some(0))) {
        return Err(Skip::Proxy);
    }
    let port = session
        .number("PortNumber")
        .ok()
        .and_then(|port| port.map(u16::try_from).transpose().ok())
        .ok_or(Skip::BadPort)?;
    let mut path: Vec<String> = session.name.split('/').map(unescape).collect();
    let name = path.pop().unwrap_or_default();
    let group = path
        .iter()
        .map(|folder| folder.trim())
        .filter(|folder| !folder.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
    let value = |key| unescape(session.get(key));
    Host::new(
        &name,
        &value("HostName"),
        port,
        &value("UserName"),
        &value("PublicKeyFile"),
        Some(&group),
    )
    .map(Host::without_ppk)
}
