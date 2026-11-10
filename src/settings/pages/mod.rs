//! The settings screen's tabs, one file each: its rows, its render and
//! what its controls do. `SettingsView` hosts them (spec 0009).

mod appearance;
mod general;
mod import;
mod keyboard;
mod profiles;
mod terminal;

pub use general::update_check_name;
pub(super) use import::ImportCard;
pub(super) use profiles::Form;
