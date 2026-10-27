//! Native Windows adapters own every unsafe ABI and handle lifetime.
mod handle;
mod process;
mod security;
use crate::error::Result;
pub(crate) fn native_supported() -> Result<()> {
    Ok(())
}

mod download_stage;
mod image;
mod package_stage;
pub(crate) use download_stage::{ProtectedDownloadBytes, stage_download_bytes};
pub(crate) fn install_local() -> Result<u32> {
    package_stage::install_local()
}
mod layout;
mod pipe;
mod registry;
mod scm;
mod source;

mod controller;

mod monotonic;
/// Raw native counter DATA only. This does not authenticate a service context.
pub(crate) fn read_monotonic_counters() -> Result<crate::monotonic::CounterObservation> {
    monotonic::read_counters()
}

mod installed;
mod installed_images;
mod live_time;
mod original_root;
mod time_owner;
mod time_service;
mod time_transport;
pub(crate) use installed_images::{InstalledImages, inspect_images};

mod app_writer;
mod client;
pub(crate) use app_writer::{AppWriterAdmission, admit_current_app_writer};
pub(crate) use client::{inspect_trusted_time, request_trusted_time_refresh};

mod launch;

mod binding;
mod restart;
pub(crate) fn prepare_restart() -> crate::error::Result<crate::restart::PreparedRestart> {
    restart::begin()
}

mod keeper_spawn;
mod peer;
mod writer_coordinator;
mod writer_job;
mod writer_keeper;
mod writer_service;
mod writer_status;
pub(crate) use writer_status::inspect_writer_journal;

mod bootstrap;
mod dispatch;
mod launcher;
mod os_image;
mod preflight;
mod references;
mod server;
mod service;

mod desktop;

mod role_process;
pub(crate) fn harden_role_process() -> Result<()> {
    role_process::harden()
}
pub(crate) fn execute_helper(mode: crate::entry::HelperMode) -> Result<()> {
    crate::profile::Seed::embedded()?;
    match mode {
        crate::entry::HelperMode::Service => service::dispatch(),
        crate::entry::HelperMode::WriterCustody => writer_keeper::run(),
        crate::entry::HelperMode::PrepareInstall => controller::prepare_install(),
        crate::entry::HelperMode::PrepareLocalInstall => controller::prepare_local_install(),
        // No real metadata/consume handoff producer is installed in this batch.
        // Never fall through to local administrative bootstrap on missing proof.
        crate::entry::HelperMode::PrepareMetadataUpgrade => {
            Err(crate::Error::InitialTrustUnavailable)
        }
        crate::entry::HelperMode::VerifyFiles => controller::verify_files(),
        crate::entry::HelperMode::StartService => controller::start_service(),
        crate::entry::HelperMode::PrepareRemove => controller::prepare_remove(),
        crate::entry::HelperMode::RemoveTarget => controller::remove_target(),
        crate::entry::HelperMode::LaunchDesktop => controller::launch_desktop(),
        crate::entry::HelperMode::Profile
        | crate::entry::HelperMode::InstallLocal
        | crate::entry::HelperMode::InspectInstalledImages => Err(crate::Error::IpcInvalid),
    }
}
pub(crate) fn execute_launcher(id: Option<&str>) -> Result<()> {
    launcher::run(id)
}

mod acceptance;
