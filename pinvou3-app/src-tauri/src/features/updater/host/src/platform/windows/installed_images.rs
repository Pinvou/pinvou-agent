//! Read-only native diagnostic. No service dispatch, startup commit, mutex,
//! registration write, ACL repair, elevation or installation operation occurs.
use super::{
    image::verify_bytes_with_poll,
    installed::{HeldAppMember, RegisteredImages, check_startup_record, seal_registered_images},
    layout::Layout,
    process::RetainedProcess,
    registry::Store,
    scm,
};
use crate::{
    download_stage::poll,
    error::{Error, Result},
    installed_images::{
        ImageDiagnostic, InstalledImageDiagnostic, ObservationStamp, require_observed_app_version,
        require_unchanged,
    },
    profile::Seed,
    registration::{HostRegistration, Phase},
};

pub(crate) struct InstalledImages {
    seed: Seed,
    layout: Layout,
    original: ObservationStamp,
    images: RegisteredImages,
    members: Vec<HeldAppMember>,
    service: RetainedProcess,
    diagnostic: InstalledImageDiagnostic,
    verified: bool,
}
fn stamp(record: &HostRegistration, service: &RetainedProcess) -> Result<ObservationStamp> {
    if record.phase != Phase::Ready {
        return Err(Error::SetupIncomplete);
    }
    Ok(ObservationStamp {
        registration_digest: record.digest()?,
        scope: record.installation_scope_id.clone(),
        revision: record.revision,
        host_epoch: record.host_epoch,
        service: service.selector.clone(),
    })
}

fn running(seed: &Seed, layout: &Layout, record: &HostRegistration) -> Result<RetainedProcess> {
    let sid = record
        .service_identity
        .service_sid
        .as_deref()
        .ok_or(Error::ServiceIdentityRefused)?;
    if scm::resolve_service_sid(seed)? != sid {
        return Err(Error::ServiceIdentityRefused);
    }
    let actual = scm::live_process(seed, layout)?;
    actual.verify_host_query_security()?;
    if actual.exited()? || actual.token.user.session_id != 0 || actual.token.network {
        return Err(Error::ServiceIdentityRefused);
    }
    check_startup_record(record, &actual.selector, &record.helper.sha256)?;
    Ok(actual)
}

impl InstalledImages {
    pub(crate) fn diagnostic(&self) -> Result<&InstalledImageDiagnostic> {
        if !self.verified {
            return Err(Error::ImageRefused);
        }
        Ok(&self.diagnostic)
    }
    pub(crate) fn recheck(&mut self, cancelled: &mut dyn FnMut() -> bool) -> Result<()> {
        // A failed refresh cannot leave a currently usable diagnostic projection.
        self.verified = false;
        poll(cancelled)?;
        let store = Store::new(&self.seed);
        let before = store.read(&self.seed)?.ok_or(Error::SetupIncomplete)?;
        let live = running(&self.seed, &self.layout, &before)?;
        if self.service.exited()? {
            return Err(Error::ServiceIdentityRefused);
        }
        require_unchanged(&self.original, &stamp(&before, &live)?)?;
        // Re-read the original root/shards against the protected registry head;
        // no missing/corrupt material is filled from the compiled configuration.
        let current_manifest = store.manifest(&self.seed, &before)?;
        self.images.targets.validate_known_contents_with_poll(
            &self.seed,
            &current_manifest,
            true,
            cancelled,
        )?;
        if self.members.len() != current_manifest.app_files.len() {
            return Err(Error::StateCorrupt);
        }
        for member in &mut self.members {
            member.recheck(&self.images.targets.app, cancelled)?;
        }
        // Parse the held application image before the complete byte recheck.
        let observed =
            crate::package_resource::observe_version(&mut self.images.app.file, cancelled)?;
        require_observed_app_version(
            &observed.canonical_version,
            &self.images.manifest.app.version,
            &self.seed.app_version,
        )?;
        for (name, root, image) in [
            (
                "pinvou-upgrade-helper.exe",
                &self.images.helper_root,
                &mut self.images.helper,
            ),
            (
                "pinvou-upgrade-launcher.exe",
                &self.images.targets.code,
                &mut self.images.launcher,
            ),
            (
                "pinvou3-tauri.exe",
                &self.images.targets.app,
                &mut self.images.app,
            ),
        ] {
            poll(cancelled)?;
            root.recheck()?;
            if verify_bytes_with_poll(&mut image.file, image.size, &image.sha256, cancelled)?
                != image.identity
            {
                return Err(Error::ImageRefused);
            }
            let mut reopened = root.file(name)?;
            if verify_bytes_with_poll(&mut reopened, image.size, &image.sha256, cancelled)?
                != image.identity
            {
                return Err(Error::ImageRefused);
            }
        }
        // All inventory handles/parents are STILL held here. Close the namespace
        // again after every actual-byte scan, then bracket with the same native
        // registration and service incarnation. No preparation/freeze lock.
        self.images.targets.validate_known_contents_with_poll(
            &self.seed,
            &current_manifest,
            true,
            cancelled,
        )?;
        self.images.helper_root.recheck()?;
        let after = store.read(&self.seed)?.ok_or(Error::SetupIncomplete)?;
        let live_after = running(&self.seed, &self.layout, &after)?;
        require_unchanged(&self.original, &stamp(&after, &live_after)?)?;
        if self.service.exited()? || live.exited()? {
            return Err(Error::ServiceIdentityRefused);
        }
        poll(cancelled)?;
        self.diagnostic.observed_application_version = observed.canonical_version;
        self.verified = true;
        Ok(())
    }
}

pub(crate) fn inspect_images(cancelled: &mut dyn FnMut() -> bool) -> Result<InstalledImages> {
    poll(cancelled)?;
    let current = RetainedProcess::current()?;
    if !current.token.elevated
        || !current.token.administrator
        || current.token.app_container
        || current.token.network
    {
        return Err(Error::SecurityRefused);
    }
    let seed = Seed::embedded()?;
    let layout = Layout::discover(&seed)?;
    let record = Store::new(&seed)
        .read(&seed)?
        .ok_or(Error::SetupIncomplete)?;
    if record.phase != Phase::Ready {
        return Err(Error::SetupIncomplete);
    }
    let service = running(&seed, &layout, &record)?;
    let original = stamp(&record, &service)?;
    let mut members = Vec::new();
    let images = seal_registered_images(&seed, &layout, &record, cancelled, |file| {
        members.push(file)
    })?;
    let image_fact = |reference: &crate::profile::ImageReference| ImageDiagnostic {
        size: reference.size,
        sha256: reference.sha256.clone(),
        manifest_declared_version: reference.version.clone(),
    };
    let total = images
        .manifest
        .app_files
        .iter()
        .try_fold(0u64, |sum, file| {
            sum.checked_add(file.size).ok_or(Error::StateCorrupt)
        })?;
    let diagnostic = InstalledImageDiagnostic {
        observation_version: 1,
        observation_kind: "installed-images-physical-diagnostic",
        registration_revision: record.revision,
        host_epoch: record.host_epoch,
        manifest_sha256: record.genesis_manifest_sha256.clone(),
        application: image_fact(&images.manifest.app),
        observed_application_version: String::new(),
        helper: image_fact(&images.manifest.helper),
        launcher: image_fact(&images.manifest.launcher),
        checked_inventory_files: members.len(),
        checked_inventory_bytes: total,
        complete_inventory_checked: true,
        json_retains_leases: false,
        source_facts_produced: false,
        current_qualification: false,
        installation_permission: false,
    };
    let mut result = InstalledImages {
        seed,
        layout,
        original,
        images,
        members,
        service,
        diagnostic,
        verified: false,
    };
    result.recheck(cancelled)?;
    Ok(result)
}
