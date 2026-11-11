//! Port forwards (spec 0017): local, remote and SOCKS forwards to an SSH
//! pane's host, each its own `ssh -N` ([`process`]), held in memory by the
//! app's [`registry`] until deleted or Tern quits, and added in the SSH
//! sidebar's Port forwards [`view`]. This module is the plain data: what a
//! forward is, how the add row's text becomes one, and what ssh is given.

pub mod process;
pub mod registry;
pub mod view;

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

/// A forward's ID, unique for the app run.
pub type Id = u64;

/// Why a host can't have a forward: it listens where another one does
/// (flow 2).
pub const CLASH: &str = "This host already has a forward on that port";

/// What the palette's `pf` shortcut needs typed after it (flow 10).
const SHORTCUT_HELP: &str = "Type a forward like 13305:13305, -R 9000:3000 or -D 1080";

/// The forward a palette query asks for in ssh's syntax (spec 0017, flow
/// 10): `pf 13305:13305`, `pf 5432:db:5432`, `pf -R 9000:3000`, `pf -D
/// 1080`. `None` unless it starts with `pf `. It always listens on
/// `127.0.0.1` [load-bearing]: a bind address is refused, since only the
/// sidebar's checkbox opens a port to the network (TM-fwd-02).
pub fn shortcut(query: &str) -> Option<Result<Forward, &'static str>> {
    let rest = query.trim_start().strip_prefix("pf ")?;
    let mut words = rest.split_whitespace();
    let (kind, spec) = match (words.next(), words.next(), words.next()) {
        (Some("-L"), Some(spec), None) | (Some(spec), None, None) if !spec.starts_with('-') => {
            (Kind::Local, spec)
        }
        (Some("-R"), Some(spec), None) => (Kind::Remote, spec),
        (Some("-D"), Some(spec), None) => (Kind::Socks, spec),
        _ => return Some(Err(SHORTCUT_HELP)),
    };
    let (listen, to) = match kind {
        Kind::Socks => (spec, ""),
        _ => spec.split_once(':').unwrap_or((spec, "")),
    };
    // A first part that isn't a port is a bind address: `0.0.0.0:80:web:80`.
    if listen.contains(['.', '[', '*'])
        || listen == "localhost"
        || kind == Kind::Socks && spec.contains(':')
    {
        return Some(Err(
            "Forwards from here listen on 127.0.0.1; to let other machines connect, use the sidebar",
        ));
    }
    Some(Forward::parse(kind, listen, to, false))
}

/// What the add row's kind buttons choose between.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Local,
    Remote,
    Socks,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Local, Kind::Remote, Kind::Socks];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Local => "Local",
            Kind::Remote => "Remote",
            Kind::Socks => "SOCKS",
        }
    }
}

/// Where a local or remote forward leads: a host checked by [`parse_host`]
/// and a port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Destination {
    /// An IPv4 address, an IPv6 address in brackets, or a host name.
    pub host: String,
    pub port: u16,
}

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

/// Which way a forward goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Way {
    /// `-L`: a port on this machine leads to `Destination` from the host.
    Local(Destination),
    /// `-R`: a port on the host leads to `Destination` from this machine.
    Remote(Destination),
    /// `-D`: a SOCKS proxy on this machine into the host's network.
    Dynamic,
}

/// One forward, as the add row checked it (spec 0017, Model). Only typed
/// fields reach ssh (TM-fwd-05).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forward {
    pub way: Way,
    pub listen: u16,
    /// "Let other machines connect": listen on `0.0.0.0` instead of `127.0.0.1`
    /// (TM-fwd-01, -02). Off unless the user checks it.
    pub network: bool,
}

impl Forward {
    /// The forward the add row describes, or why Add stays disabled (spec
    /// 0017, flow 2).
    pub fn parse(kind: Kind, listen: &str, to: &str, network: bool) -> Result<Self, &'static str> {
        let listen = parse_port(listen).ok_or("The listen port must be 1–65535")?;
        let to = || -> Result<Destination, &'static str> {
            let to = to.trim();
            if to.is_empty() {
                return Err("Say where it leads: a port, or host:port");
            }
            parse_destination(to)
        };
        let way = match kind {
            Kind::Local => Way::Local(to()?),
            Kind::Remote => Way::Remote(to()?),
            Kind::Socks => Way::Dynamic,
        };
        Ok(Self {
            way,
            listen,
            network,
        })
    }

    /// Whether the listening port is on the host rather than this machine.
    pub fn listens_remotely(&self) -> bool {
        matches!(self.way, Way::Remote(_))
    }

    /// Whether two forwards would listen on the same port on the same side,
    /// which flow 2 doesn't allow for one host.
    pub fn clashes(&self, other: &Forward) -> bool {
        self.listen == other.listen && self.listens_remotely() == other.listens_remotely()
    }

    /// ssh's option for it: `-L`, `-R` or `-D`.
    pub fn flag(&self) -> &'static str {
        match self.way {
            Way::Local(_) => "-L",
            Way::Remote(_) => "-R",
            Way::Dynamic => "-D",
        }
    }

    /// The value after [`Forward::flag`], with the bind address always
    /// written out, so `GatewayPorts` in `~/.ssh/config` can't widen it
    /// (TM-fwd-01): `127.0.0.1:5432:db.internal:5432`.
    pub fn arg(&self) -> String {
        let bind = if self.network { "0.0.0.0" } else { "127.0.0.1" };
        match &self.way {
            Way::Local(to) | Way::Remote(to) => format!("{bind}:{}:{to}", self.listen),
            Way::Dynamic => format!("{bind}:{}", self.listen),
        }
    }

    /// How the sidebar and `forwards.stop` name it: `5432 → db.internal:5432`,
    /// `remote 9000 → localhost:3000`, `SOCKS 1080`, with `(network)` after
    /// the port of one that allows it.
    pub fn summary(&self) -> String {
        let network = if self.network { " (network)" } else { "" };
        match &self.way {
            Way::Local(to) => format!("{}{network} → {to}", self.listen),
            Way::Remote(to) => format!("remote {}{network} → {to}", self.listen),
            Way::Dynamic => format!("SOCKS {}{network}", self.listen),
        }
    }
}

/// A port of 1–65535 written in digits only.
fn parse_port(text: &str) -> Option<u16> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|port| *port != 0)
}

/// `host:port`, where an IPv6 host is in brackets: `[::1]:5432`. A port
/// alone is `localhost` on the side the forward leads from.
fn parse_destination(text: &str) -> Result<Destination, &'static str> {
    if let Some(port) = parse_port(text) {
        return Ok(Destination {
            host: "localhost".to_owned(),
            port,
        });
    }
    let (host, port) = text
        .rsplit_once(':')
        .ok_or("Say where it leads: a port, or host:port")?;
    let port = parse_port(port).ok_or("The target port must be 1–65535")?;
    let host = parse_host(host).ok_or(
        "The target host may only have letters, digits, '.', '-' and '_', \
         or be an IP address (IPv6 in brackets)",
    )?;
    Ok(Destination { host, port })
}

/// A host that can't be anything but a host to ssh: an IPv4 address, an
/// IPv6 address in brackets, or a name of letters, digits, `.`, `-` and
/// `_` that doesn't start with `-` (TM-fwd-05).
fn parse_host(host: &str) -> Option<String> {
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return inner.parse::<Ipv6Addr>().ok().map(|ip| format!("[{ip}]"));
    }
    if host.parse::<Ipv4Addr>().is_ok() {
        return Some(host.to_owned());
    }
    let name = !host.is_empty()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    name.then(|| host.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(listen: &str, to: &str) -> Result<Forward, &'static str> {
        Forward::parse(Kind::Local, listen, to, false)
    }

    /// Flow 2: a port is 1–65535 in digits; anything else keeps Add
    /// disabled. Surrounding blanks from a paste are harmless, since the
    /// port becomes a number before it reaches ssh.
    #[test]
    fn a_listen_port_is_1_to_65535_in_digits() {
        assert_eq!(local("1", "db:1").map(|f| f.listen), Ok(1));
        assert_eq!(local("65535", "db:1").map(|f| f.listen), Ok(65535));
        assert_eq!(local(" 80 ", "db:1").map(|f| f.listen), Ok(80));
        for bad in [
            "0", "65536", "70000", "", "  ", "12a", "-1", "+80", "8 0", "0x50", "٣",
        ] {
            assert!(local(bad, "db:1").is_err(), "listen port {bad:?} was taken");
        }
    }

    /// Flow 2: the `to` port follows the same rule as the listen port.
    #[test]
    fn a_target_port_is_1_to_65535_in_digits() {
        assert!(local("5432", "db:65535").is_ok());
        for bad in [
            "db:0",
            "db:70000",
            "db:",
            "db:12a",
            "db",
            "db:5432 -v",
            "0",
            "70000",
        ] {
            assert!(local("5432", bad).is_err(), "`to` {bad:?} was taken");
        }
    }

    /// Flow 2: Local and Remote lead somewhere, so they need a `to`;
    /// SOCKS leads anywhere and ignores the field.
    #[test]
    fn local_and_remote_need_a_to_and_socks_does_not() {
        assert!(Forward::parse(Kind::Local, "5432", "", false).is_err());
        assert!(Forward::parse(Kind::Remote, "9000", "  ", false).is_err());
        let socks = Forward::parse(Kind::Socks, "1080", "", false).expect("SOCKS without `to`");
        assert_eq!(socks.way, Way::Dynamic);
    }

    /// Flow 2: a `to` host is a name, an IPv4 address or an IPv6 address
    /// in brackets.
    #[test]
    fn a_to_host_is_a_name_an_ipv4_or_a_bracketed_ipv6_address() {
        let to = |text: &str| match local("5432", text).map(|f| f.way) {
            Ok(Way::Local(to)) => Some(to.to_string()),
            _ => None,
        };
        assert_eq!(to("db.internal:5432").as_deref(), Some("db.internal:5432"));
        // A port alone leads to that port on the far side's own localhost.
        assert_eq!(to("13305").as_deref(), Some("localhost:13305"));
        assert_eq!(to("db-1_a:5432").as_deref(), Some("db-1_a:5432"));
        assert_eq!(to("localhost:3000").as_deref(), Some("localhost:3000"));
        assert_eq!(to("10.0.0.7:5432").as_deref(), Some("10.0.0.7:5432"));
        assert_eq!(to("[::1]:5432").as_deref(), Some("[::1]:5432"));
        assert_eq!(to("[fd00::7]:22").as_deref(), Some("[fd00::7]:22"));
    }

    /// TM-fwd-05: nothing typed into `to` can become an ssh option or
    /// turn one forward into another. Each of these keeps Add disabled.
    #[test]
    fn hostile_to_hosts_keep_add_disabled() {
        let hostile = [
            "-oProxyCommand=x",
            "-oProxyCommand=x:22",
            "-oProxyCommand=sh -c id:22",
            "-db:22",
            "a b:22",
            "a,b:22",
            "db:22,other:23",
            "db\t:22",
            "db\n-oProxyCommand=x:22",
            "user@db:22",
            "db/x:22",
            "$(id):22",
            "`id`:22",
            "db;id:22",
            "::1:22",
            "fd00::7:22",
            "[db.internal]:22",
            "[::1:22",
            "[]:22",
            ":22",
        ];
        for kind in [Kind::Local, Kind::Remote] {
            for to in hostile {
                assert!(
                    Forward::parse(kind, "5432", to, false).is_err(),
                    "{kind:?} took `to` {to:?}"
                );
            }
        }
    }

    /// AC1 and TM-fwd-01: the bind address is always written out, so
    /// `GatewayPorts yes` in `~/.ssh/config` can't widen a forward.
    #[test]
    fn the_forward_value_always_names_loopback_as_its_bind_address() {
        let arg = |kind, listen, to| {
            let forward = Forward::parse(kind, listen, to, false).expect("a valid forward");
            (forward.flag(), forward.arg())
        };
        assert_eq!(
            arg(Kind::Local, "5432", "db.internal:5432"),
            ("-L", "127.0.0.1:5432:db.internal:5432".to_owned())
        );
        assert_eq!(
            arg(Kind::Remote, "9000", "localhost:3000"),
            ("-R", "127.0.0.1:9000:localhost:3000".to_owned())
        );
        assert_eq!(
            arg(Kind::Socks, "1080", ""),
            ("-D", "127.0.0.1:1080".to_owned())
        );
        assert_eq!(
            arg(Kind::Local, "8080", "[::1]:80"),
            ("-L", "127.0.0.1:8080:[::1]:80".to_owned())
        );
    }

    /// Flow 3 and TM-fwd-02: only "Let other machines connect" binds `0.0.0.0`.
    #[test]
    fn allow_from_network_binds_every_ipv4_interface() {
        let network = |kind, to| {
            Forward::parse(kind, "5432", to, true)
                .expect("a valid forward")
                .arg()
        };
        assert_eq!(network(Kind::Local, "db:5432"), "0.0.0.0:5432:db:5432");
        assert_eq!(network(Kind::Remote, "db:5432"), "0.0.0.0:5432:db:5432");
        assert_eq!(network(Kind::Socks, ""), "0.0.0.0:5432");
    }

    /// Module surface and flow 3: how the sidebar and `forwards.stop`
    /// name a forward, with "(network)" on one that allows it.
    #[test]
    fn summaries_name_the_kind_the_ports_and_network_access() {
        let summary = |kind, listen, to, network| {
            Forward::parse(kind, listen, to, network)
                .expect("a valid forward")
                .summary()
        };
        assert_eq!(
            summary(Kind::Local, "5432", "db.internal:5432", false),
            "5432 → db.internal:5432"
        );
        assert_eq!(
            summary(Kind::Remote, "9000", "localhost:3000", false),
            "remote 9000 → localhost:3000"
        );
        assert_eq!(summary(Kind::Socks, "1080", "", false), "SOCKS 1080");
        assert_eq!(
            summary(Kind::Local, "5432", "db.internal:5432", true),
            "5432 (network) → db.internal:5432"
        );
        assert_eq!(
            summary(Kind::Remote, "9000", "localhost:3000", true),
            "remote 9000 (network) → localhost:3000"
        );
        assert_eq!(
            summary(Kind::Socks, "1080", "", true),
            "SOCKS 1080 (network)"
        );
    }

    /// Flow 2: one host can't have two forwards listening on the same port
    /// on the same side. A remote forward listens on the server, so it
    /// doesn't clash with a local or SOCKS one on this machine.
    #[test]
    fn forwards_clash_on_the_same_port_on_the_same_side_only() {
        let forward =
            |kind, listen| Forward::parse(kind, listen, "db:1", false).expect("a valid forward");
        let local = forward(Kind::Local, "5432");
        assert!(local.clashes(&forward(Kind::Local, "5432")));
        assert!(local.clashes(&forward(Kind::Socks, "5432")));
        assert!(forward(Kind::Socks, "5432").clashes(&local));
        assert!(forward(Kind::Remote, "5432").clashes(&forward(Kind::Remote, "5432")));
        assert!(!local.clashes(&forward(Kind::Remote, "5432")));
        assert!(!forward(Kind::Remote, "5432").clashes(&local));
        assert!(!local.clashes(&forward(Kind::Local, "5433")));
        // Network access doesn't change the port.
        let network = Forward::parse(Kind::Local, "5432", "db:1", true).expect("a valid forward");
        assert!(local.clashes(&network));
    }

    fn pf(query: &str) -> Forward {
        match shortcut(query) {
            Some(Ok(forward)) => forward,
            other => panic!("`{query}` gave {other:?}"),
        }
    }

    /// Flow 10 and AC11: `pf 13305:13305` is `-L` by default, listens on
    /// this machine's loopback and leads to the host's own localhost.
    #[test]
    fn a_pf_query_with_two_ports_is_a_local_forward_on_loopback() {
        let forward = pf("pf 13305:13305");
        assert_eq!(
            forward,
            Forward {
                way: Way::Local(Destination {
                    host: "localhost".into(),
                    port: 13305
                }),
                listen: 13305,
                network: false,
            }
        );
        assert_eq!(
            (forward.flag(), forward.arg()),
            ("-L", "127.0.0.1:13305:localhost:13305".to_owned())
        );
        assert_eq!(forward.summary(), "13305 → localhost:13305");
    }

    /// Flow 10: ssh's own forward syntax, with `-L` optional, `-R` remote
    /// and `-D` SOCKS; blanks around the words are a typing slip, not a
    /// different forward.
    #[test]
    fn a_pf_query_reads_ssh_forward_syntax() {
        let db = pf("pf 5432:db.internal:5432");
        assert_eq!(db.summary(), "5432 → db.internal:5432");
        assert_eq!(pf("pf -L 5432:db.internal:5432"), db);
        assert_eq!(pf("  pf   -L   5432:db.internal:5432  "), db);

        let remote = pf("pf -R 9000:3000");
        assert_eq!(remote.summary(), "remote 9000 → localhost:3000");
        assert_eq!(remote.arg(), "127.0.0.1:9000:localhost:3000");
        assert_eq!(pf("pf  -R\t9000:3000 "), remote);

        let socks = pf("pf -D 1080");
        assert_eq!((socks.way.clone(), socks.listen), (Way::Dynamic, 1080));
        assert_eq!(socks.arg(), "127.0.0.1:1080");
    }

    /// Flow 10 and TM-fwd-02 [load-bearing]: a bind address never opens a
    /// port to the network from the palette; the entry sends the user to
    /// the sidebar's "Let other machines connect" instead.
    #[test]
    fn a_pf_query_with_a_bind_address_points_to_the_sidebar() {
        for query in [
            "pf 0.0.0.0:80:web:80",
            "pf -L 0.0.0.0:80:web:80",
            "pf localhost:80:web:80",
            "pf 127.0.0.1:80:web:80",
            "pf *:80:web:80",
            "pf [::]:80:x:80",
            "pf -D 0.0.0.0:1080",
            "pf -D localhost:1080",
            "pf -R 0.0.0.0:9000:3000",
        ] {
            match shortcut(query) {
                Some(Err(reason)) => assert!(
                    reason.contains("sidebar"),
                    "`{query}` doesn't point to the sidebar: {reason}"
                ),
                other => panic!("`{query}` gave {other:?}"),
            }
        }
    }

    /// Flow 10 and TM-fwd-02: whatever is typed, no forward from the
    /// palette listens on the network.
    #[test]
    fn no_pf_query_makes_a_network_forward() {
        for query in [
            "pf 13305:13305",
            "pf -R 9000:3000",
            "pf -D 1080",
            "pf 0.0.0.0:80:web:80",
            "pf 80:0.0.0.0:80",
            "pf -R 9000:0.0.0.0:3000",
            "pf -D 0.0.0.0:1080",
            "pf GatewayPorts:80:web:80",
            "pf 1080 -g",
            "pf -g 1080",
        ] {
            if let Some(Ok(forward)) = shortcut(query) {
                assert!(!forward.network, "`{query}` listens on the network");
                assert!(forward.arg().starts_with("127.0.0.1:"), "`{query}`");
            }
        }
    }

    /// Flow 10 reusing flow 2: what the sidebar's Add refuses, the palette
    /// refuses too, and nothing typed becomes an extra ssh word or option
    /// (TM-fwd-05).
    #[test]
    fn a_pf_query_flow_2_refuses_is_an_error() {
        for query in [
            "pf ",
            "pf    ",
            "pf 70000:1",
            "pf 0:1",
            "pf 1:70000",
            "pf 13305",
            "pf -L 13305",
            "pf -R 9000",
            "pf -D",
            "pf -D 0",
            "pf -L",
            "pf 1:-oProxyCommand=x:22",
            "pf -R 1:-oProxyCommand=x:22",
            "pf -oProxyCommand=x",
            "pf -L -oProxyCommand=x:22",
            "pf 1:2 3",
            "pf 1:2 -oProxyCommand=x",
            "pf -L 1:2 3",
            "pf -D 1080 1081",
            "pf -X 1:2",
            "pf -l 1:2",
            "pf 1:a b:22",
            "pf 1:$(id):22",
        ] {
            assert!(
                matches!(shortcut(query), Some(Err(_))),
                "`{query}` gave {:?}",
                shortcut(query)
            );
        }
    }

    /// Flow 10: only a query starting with `pf ` is a forward; anything
    /// else ranks the palette's commands as usual.
    #[test]
    fn a_query_without_pf_and_a_space_is_no_shortcut() {
        for query in [
            "",
            "p",
            "pf",
            "pf13305:1",
            "pfx 1:2",
            "split",
            "port forwards",
        ] {
            assert_eq!(shortcut(query), None, "`{query}`");
        }
    }
}
