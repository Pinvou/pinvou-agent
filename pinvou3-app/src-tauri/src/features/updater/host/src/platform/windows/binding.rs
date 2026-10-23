//! Hash a registered image and actual service incarnation; never an image lease.
use super::image::SealedImage;
use crate::{
    error::{Error, Result},
    identity::ProcessSelector,
    json,
    registration::PublicRegistration,
};
use serde::Serialize;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Binding<'a> {
    installation_scope_id: &'a str,
    seed_sha256: &'a str,
    manifest_sha256: &'a str,
    host_epoch: u64,
    host_instance_nonce: &'a str,
    service_process: &'a ProcessSelector,
    app: &'a crate::profile::ImageReference,
    app_file_identity: pinvou_upgrade_custody::Identity,
}
pub(super) fn image(
    record: &PublicRegistration,
    app: &SealedImage,
    server: &ProcessSelector,
    instance: &str,
) -> Result<String> {
    if !json::digest(instance)
        || record.host_epoch == 0
        || app.sha256 != record.app.sha256
        || app.size != record.app.size
    {
        return Err(Error::ImageRefused);
    }
    let identity = pinvou_upgrade_custody::identity(&app.file).map_err(|_| Error::ImageRefused)?;
    if identity != app.identity {
        return Err(Error::ImageRefused);
    }
    Ok(json::sha256(&json::encode(
        &Binding {
            installation_scope_id: &record.installation_scope_id,
            seed_sha256: &record.genesis_seed_sha256,
            manifest_sha256: &record.genesis_manifest_sha256,
            host_epoch: record.host_epoch,
            host_instance_nonce: instance,
            service_process: server,
            app: &record.app,
            app_file_identity: identity,
        },
        false,
    )?))
}
