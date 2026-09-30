pub mod app_events;
pub(crate) mod capabilities;
pub mod connector_lock;
pub mod connector_skills;
pub mod connector_state;
pub mod credential_store;
#[cfg(target_os = "macos")]
pub(crate) mod cursor;
pub(crate) mod download;
pub(crate) mod encoding;
pub(crate) mod filesystem;
pub(crate) mod hashing;
pub(crate) mod notifications;
pub(crate) mod os;
pub mod path_policy;
pub mod paths;
pub mod prefs;
pub(crate) mod process;
// Targeted re-export: the module itself stays crate-private, but the CLI's
// Windows kill tree needs the hardened resolution of `external_command`
// across the crate boundary (`pinvoy3_lib::platform::external_command`).
// Item-level `pub` inside a `pub(crate)` module is capped at crate
// visibility, so without this re-export the external consumer cannot name
// the path.
pub use process::external_command;
// Same crate-boundary shape as `external_command` above: the headless CLI
// verifies downloaded artifacts (connector CLI binaries, staged voice
// models) with the app's own sha256 rather than a drifting copy.
pub use hashing::sha256_file;
pub(crate) mod startup;
pub(crate) mod strings;
pub mod super_permission;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod ui_cache;
pub(crate) mod window_startup;
