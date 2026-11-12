//! `plugins:` (spec 0016, Model): what the user granted each plugin, by its
//! ID. A plugin that isn't named here is found and listed, and never
//! started.

use crate::plugins::{Right, Subscription};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One plugin's entry. A key or a right Tern doesn't know makes the whole
/// entry fall back, so the plugin stays disabled rather than run with
/// something other than what was written.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PluginSettings {
    pub enabled: bool,
    /// The command line the enable confirmation showed. A manifest whose
    /// `run` differs isn't started (TM-plug-12).
    pub run: Vec<String>,
    pub rights: Vec<Right>,
    pub events: Vec<Subscription>,
    /// The plugin's own settings, sent with every event. Plain text: no
    /// place for secrets (TM-plug-18).
    pub settings: BTreeMap<String, Setting>,
}

/// A plugin setting's value: `bool`, `number`, or `text` and `choice`.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Setting {
    Bool(bool),
    Number(f64),
    Text(String),
}
