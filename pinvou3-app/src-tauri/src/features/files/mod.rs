pub mod attachment_upload;
pub mod file_ingest;
pub(crate) mod file_watcher;
mod platform;

/// Hard read bound for artifact TEXT files, shared by the viewer lane
/// (`app::commands::artifacts::read_artifact_text`) and the review lane
/// (`features::review` summon feed): a text artifact is read into a String
/// before serialization/IPC, so tens-of-MB reads are a memory spike + UI
/// freeze. Lives here (features tier) because commands may depend on
/// features, never the reverse (architecture guard: rust_feature_depends_on_app).
pub(crate) const MAX_ARTIFACT_TEXT_BYTES: u64 = 50 * 1024 * 1024;
