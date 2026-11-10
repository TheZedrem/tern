//! PuTTY's saved sessions (spec 0015): SSH sessions become hosts, without
//! a group (PuTTY has no folders). `Default Settings` is a template, not a
//! host: it's passed over and not counted.

use super::sessions::{Session, unescape};
use super::{Host, Parsed, Skip};

pub fn parse(sessions: &[Session]) -> Parsed {
    let mut parsed = Parsed {
        format: "PuTTY",
        asks_user: true,
        ..Parsed::default()
    };
    for session in sessions.iter().filter(|s| !s.is_defaults()) {
        parsed.push(host(session));
    }
    parsed
}

fn host(session: &Session) -> Result<Host, Skip> {
    // No `Protocol` is SSH, PuTTY's default.
    if !matches!(session.get("Protocol"), "" | "ssh") {
        return Err(Skip::NotSsh);
    }
    proxy(session)?;
    let port = session
        .number("PortNumber")
        .ok()
        .and_then(|port| port.map(u16::try_from).transpose().ok())
        .ok_or(Skip::BadPort)?;
    // A `user@host` in the host name wins over `UserName`, as in PuTTY.
    let (user, host) = match session.get("HostName").rsplit_once('@') {
        Some((user, host)) => (user, host),
        None => (session.get("UserName"), session.get("HostName")),
    };
    Host::new(
        &unescape(&session.name),
        host,
        port,
        user,
        session.get("PublicKeyFile"),
        None,
    )
    .map(Host::without_ppk)
}

/// PuTTY's proxy types (`ProxyMethod`): 0 none, 1–5 SOCKS, HTTP, telnet or
/// a local command, 6–8 through another SSH host. Before 0.77 the setting
/// was `ProxyType`, where anything but 0 is a proxy.
fn proxy(session: &Session) -> Result<(), Skip> {
    match session.number("ProxyMethod") {
        Ok(Some(0)) => Ok(()),
        Ok(Some(6..=8)) => Err(Skip::JumpHost),
        Ok(Some(_)) | Err(()) => Err(Skip::Proxy),
        Ok(None) => match session.number("ProxyType") {
            Ok(None | Some(0)) => Ok(()),
            _ => Err(Skip::Proxy),
        },
    }
}
