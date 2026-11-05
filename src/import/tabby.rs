//! Tabby's `config.yaml` (spec 0010): its `profiles:` of `type: ssh`, each
//! in the group its `group` names, by ID in `groups:` or, in older
//! configs, by name. Only the keys below are read: passwords and the
//! `vault:` are skipped by the YAML reader, never held.

use super::{Host, Parsed, Skip};
use serde::Deserialize;

#[derive(Deserialize)]
struct File {
    profiles: Vec<Profile>,
    #[serde(default)]
    groups: Vec<Group>,
}

#[derive(Deserialize)]
struct Group {
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct Profile {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    options: Options,
}

/// Tabby leaves out what's at its default, so a missing `user` is `root`
/// and a missing `port` 22, as Tabby connects.
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Options {
    host: String,
    port: Option<Port>,
    user: Option<String>,
    private_keys: Vec<String>,
    jump_host: Option<String>,
    proxy_command: Option<String>,
    socks_proxy_host: Option<String>,
    http_proxy_host: Option<String>,
}

/// A `port`: a number for SSH, a device like `/dev/ttyUSB0` or `COM3` for
/// a serial profile, which must not fail the whole file.
#[derive(Deserialize)]
#[serde(untagged)]
enum Port {
    Number(u16),
    Other(serde::de::IgnoredAny),
}

/// The hosts of a Tabby config; `None` if `text` isn't one: YAML with a
/// `profiles:` list whose entries have a `type:`.
pub fn parse(text: &str) -> Option<Parsed> {
    let file: File = serde_saphyr::from_str(text).ok()?;
    if file.profiles.is_empty() {
        return None;
    }
    let mut parsed = Parsed {
        format: "Tabby",
        ..Parsed::default()
    };
    for profile in &file.profiles {
        parsed.push(host(profile, &file.groups));
    }
    Some(parsed)
}

fn host(profile: &Profile, groups: &[Group]) -> Result<Host, Skip> {
    if profile.kind != "ssh" {
        return Err(Skip::NotSsh);
    }
    let options = &profile.options;
    let set = |value: &Option<String>| value.as_deref().is_some_and(|v| !v.trim().is_empty());
    if set(&options.jump_host) {
        return Err(Skip::JumpHost);
    }
    if [
        &options.proxy_command,
        &options.socks_proxy_host,
        &options.http_proxy_host,
    ]
    .into_iter()
    .any(set)
    {
        return Err(Skip::Proxy);
    }
    let group = profile.group.as_deref().map(|group| {
        groups
            .iter()
            .find(|found| found.id == group)
            .map_or(group, |found| found.name.as_str())
    });
    let port = match options.port {
        None => None,
        Some(Port::Number(port)) => Some(port),
        Some(Port::Other(_)) => return Err(Skip::BadPort),
    };
    // Keys in Tabby's vault (no `file://`) stay there.
    let key = options
        .private_keys
        .iter()
        .find_map(|key| key.strip_prefix("file://"))
        .unwrap_or_default();
    Host::new(
        &profile.name,
        &options.host,
        port,
        options.user.as_deref().unwrap_or("root"),
        key,
        group,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Jump hosts and proxies are skipped with their reason, and a key kept
    /// in the vault (no `file://`) isn't imported as a path.
    #[test]
    fn jump_hosts_proxies_and_vault_keys_are_left_out() {
        let parsed = parse(
            "profiles:
  - type: ssh
    name: Behind jump
    options: { host: a.example.com, jumpHost: bastion-id }
  - type: ssh
    name: Behind socks
    options: { host: b.example.com, socksProxyHost: 127.0.0.1 }
  - type: ssh
    name: Vault key
    options: { host: c.example.com, user: me, privateKeys: ['vault://key-id'] }
",
        )
        .expect("Tabby");
        assert_eq!(parsed.skipped, [Skip::JumpHost, Skip::Proxy]);
        assert_eq!(parsed.hosts.len(), 1);
        assert_eq!(parsed.hosts[0].identity_file, None);
    }
}
