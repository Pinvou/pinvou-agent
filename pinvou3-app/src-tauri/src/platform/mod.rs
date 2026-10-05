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
// across the crate boundary (`pinvou3_lib::platform::external_command`).
// Item-level `pub` inside a `pub(crate)` module is capped at crate
// visibility, so without this re-export the external consumer cannot name
// the path.
pub use process::external_command;
// Same crate-boundary shape as `external_command` above: the headless CLI
// verifies downloaded artifacts (connector CLI binaries, staged voice
// models) with the app's own sha256 rather than a drifting copy.
pub use hashing::sha256_file;
// And the npm mirror registry the `connectors ensure-cli tmeet` retry chain
// falls back to (the GUI's own install_tmeet_cli constant).
pub use download::NPM_MIRROR_REGISTRY;
// The headless CLI's archive downloads run on the same slow links the GUI's
// budget was sized for ("slow links need ~150 KB/s to finish"); a drifting
// local 600 s constant timed installs out where the GUI succeeds.
pub use download::ARTIFACT_DOWNLOAD_TOTAL_TIMEOUT;
// The projects rebind lane folds stored paths through the OS-guaranteed
// identity equivalence before comparing: on Windows, a case/separator
// spelling drift between metadata and binding must not re-admit a healthy
// session for a whole-record rewrite.
pub use os::filesystem_path_identity_key;
pub use os::path_identity_is_same_or_nested;
// Round-38: the headless CLI's `projects rebind` nesting rejection runs the
// same folded keys through the same component-boundary predicate the GUI's
// command layer uses — a raw `Path::starts_with` copy had drifted (Windows
// case-only spellings slipped past the nested arm).
// Same crate-boundary shape as the re-exports above: the headless CLI's
// file-persisting lanes (`artifacts write`, `feedback submit`) were the last
// holders of a drifting local stage-then-rename copy; they now consume the
// app's hardened writer (Windows backup/rollback state machine included)
// instead of maintaining their own.
pub use filesystem::{atomic_write, atomic_write_private};
pub(crate) mod startup;
pub(crate) mod strings;
pub mod super_permission;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod ui_cache;
pub(crate) mod window_startup;
