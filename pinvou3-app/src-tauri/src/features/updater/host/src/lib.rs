//! Windows first-install host. Data models are not protected capabilities.
//! Shipping app integration remains disabled until actual named-profile acceptance.
mod bootstrap;
mod error;
mod handoff;
mod identity;
mod ipc;
mod json;
mod monotonic;
mod platform;
mod profile;
mod registration;
mod restart;
mod trusted_time;

pub use error::Error;
pub use restart::{CommittedRestart, PreparedRestart};
/// Prepare one actual same-user restart association before app cleanup.
pub fn prepare_restart() -> std::result::Result<PreparedRestart, Error> {
    platform::prepare_restart()
}
/// Describe the embedded PUBLIC build configuration. Successful return is
/// not native identity, installed readiness, approval or execution permission.
pub fn compiled_profile_diagnostic() -> std::result::Result<Vec<u8>, Error> {
    profile::Seed::embedded()?.diagnostic()
}

#[cfg(test)]
mod compiled_profile_tests {
    #[test]
    fn public_diagnostic_contains_only_compiled_input_facts() {
        match super::compiled_profile_diagnostic() {
            Err(super::Error::ProfileDisabled) => {}
            Ok(bytes) => {
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                let object = value.as_object().unwrap();
                let keys = [
                    "hostProfileVersion",
                    "enabled",
                    "profileKind",
                    "profileId",
                    "product",
                    "component",
                    "targetKey",
                    "buildId",
                    "layoutId",
                    "seedSha256",
                    "originalRoot",
                ];
                assert_eq!(object.len(), keys.len());
                assert!(keys.iter().all(|key| object.contains_key(*key)));
                let seed = crate::profile::Seed::embedded().unwrap();
                assert_eq!(value["profileKind"], seed.profile_kind);
                assert_eq!(value["targetKey"], "windows-x86_64-nsis-perMachine");
                assert_eq!(value["originalRoot"].as_object().unwrap().len(), 2);
            }
            Err(other) => panic!("invalid configured public build input: {other}"),
        }
    }
}

#[cfg(test)]
mod build_input;

#[cfg(test)]
mod build_toolchain;

mod entry;
pub use entry::{helper_main, launcher_main};

mod inventory;

mod package_resource;

mod download_stage;
pub use download_stage::{
    DownloadedPackageSource, NativeRole, ProtectedByteCapability, StagingLimits,
    stage_retained_download,
};

mod execution_object;
pub use execution_object::{
    ExecutionObjectGuard, ObjectBinding, ObjectGeneration, ObjectReference, VerifiedExecutionObject,
};

mod installed_images;
pub use installed_images::{InstalledImageObservation, inspect_installed_images};
