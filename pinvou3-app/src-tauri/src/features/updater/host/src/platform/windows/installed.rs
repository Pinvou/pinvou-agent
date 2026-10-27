//! Actual SCM service image/token/registration and startup incarnation owner.
//! A deserialized registration is only an expectation, never this constructor.
use super::{
    image::SealedImage,
    layout::{Layout, Targets, ordinal_equal},
    pipe::nonce,
    process::{RetainedProcess, seal_host_query_access},
    registry::{BootstrapGuard, Store},
    scm,
};
use crate::{
    error::{Error, Result},
    profile::{InstallationManifest, Seed},
    registration::{HostRegistration, Phase, StartupAttestation},
};
use pinvou_upgrade_custody::Identity;
use std::fs::File;

/// A real member and its complete held parent chain. Dropping this value keeps
/// the startup constructor's original snapshot behavior; a diagnostic retains it.
pub(super) struct HeldAppMember {
    file: File,
    identity: Identity,
    expected: crate::profile::AppFile,
    parent: Option<super::layout::ProtectedDirectory>,
    leaf: String,
}
impl HeldAppMember {
    pub(super) fn recheck(
        &mut self,
        root: &super::layout::ProtectedDirectory,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<()> {
        crate::download_stage::poll(cancelled)?;
        let parent = self.parent.as_ref().unwrap_or(root);
        parent.recheck()?;
        if super::image::verify_bytes_with_poll(
            &mut self.file,
            self.expected.size,
            &self.expected.sha256,
            cancelled,
        )? != self.identity
        {
            return Err(Error::ImageRefused);
        }
        let mut reopened = parent.file(&self.leaf)?;
        if super::image::verify_bytes_with_poll(
            &mut reopened,
            self.expected.size,
            &self.expected.sha256,
            cancelled,
        )? != self.identity
        {
            return Err(Error::ImageRefused);
        }
        parent.recheck()
    }
}

pub(super) struct RegisteredImages {
    pub(super) targets: Targets,
    pub(super) manifest: InstallationManifest,
    pub(super) helper_root: super::layout::ProtectedDirectory,
    pub(super) helper: SealedImage,
    pub(super) launcher: SealedImage,
    pub(super) app: SealedImage,
}

/// Read-only file sealing shared by SCM startup and image diagnostics. This
/// never creates a ScmDispatch, commits startup or writes a security descriptor.
pub(super) fn seal_registered_images(
    seed: &Seed,
    layout: &Layout,
    original: &HostRegistration,
    cancelled: &mut dyn FnMut() -> bool,
    mut retain: impl FnMut(HeldAppMember),
) -> Result<RegisteredImages> {
    crate::download_stage::poll(cancelled)?;
    let manifest = Store::new(seed).manifest(seed, original)?;
    if manifest.helper != original.helper
        || manifest.launcher != original.launcher
        || manifest.app != original.app
    {
        return Err(Error::StateCorrupt);
    }
    let sid = original
        .service_identity
        .service_sid
        .as_deref()
        .ok_or(Error::ServiceIdentityRefused)?;
    let targets = layout.open_targets_with_poll(sid, cancelled)?;
    targets.validate_known_contents_with_poll(seed, &manifest, true, cancelled)?;
    for expected in &manifest.app_files {
        crate::download_stage::poll(cancelled)?;
        let (parent, leaf) = targets
            .app
            .existing_relative_parent(&expected.relative_name)?;
        let mut file = parent.as_ref().unwrap_or(&targets.app).file(&leaf)?;
        let identity = super::image::verify_bytes_with_poll(
            &mut file,
            expected.size,
            &expected.sha256,
            cancelled,
        )?;
        retain(HeldAppMember {
            file,
            identity,
            expected: expected.clone(),
            parent,
            leaf,
        });
    }
    let helper_root = targets
        .code
        .child_with_poll("helpers", false, false, Some(sid), cancelled)?
        .child_with_poll(&seed.build_id, false, false, Some(sid), cancelled)?;
    let helper = SealedImage::open_with_poll(
        &helper_root,
        "pinvou-upgrade-helper.exe",
        &original.helper,
        seed,
        cancelled,
    )?;
    let launcher = SealedImage::open_with_poll(
        &targets.code,
        "pinvou-upgrade-launcher.exe",
        &original.launcher,
        seed,
        cancelled,
    )?;
    let app = SealedImage::open_with_poll(
        &targets.app,
        "pinvou3-tauri.exe",
        &original.app,
        seed,
        cancelled,
    )?;
    crate::download_stage::poll(cancelled)?;
    Ok(RegisteredImages {
        targets,
        manifest,
        helper_root,
        helper,
        launcher,
        app,
    })
}

pub(super) fn check_startup_record(
    current: &HostRegistration,
    actual: &crate::identity::ProcessSelector,
    helper_sha256: &str,
) -> Result<()> {
    let startup = current
        .startup_attestation
        .as_ref()
        .ok_or(Error::SetupIncomplete)?;
    if startup.process_id != actual.process_id
        || startup.creation_file_time != actual.creation_file_time
        || startup.host_epoch != current.host_epoch
        || startup.helper_image_sha256 != helper_sha256
        || startup.manifest_sha256 != current.genesis_manifest_sha256
    {
        return Err(Error::ServiceIdentityRefused);
    }
    Ok(())
}

pub(super) struct InstalledServiceContext {
    pub(super) seed: Seed,
    pub(super) layout: Layout,
    pub(super) targets: Targets,
    pub(super) manifest: InstallationManifest,
    pub(super) helper: SealedImage,
    pub(super) launcher: SealedImage,
    pub(super) app: SealedImage,
    pub(super) process: RetainedProcess,
    pub(super) original: HostRegistration,
}
impl InstalledServiceContext {
    /// Called only inside the successful SDK SCM dispatcher/handler binding.
    /// SCM RUNNING/PID authentication is completed before any pipe is created.
    pub(super) fn from_scm_dispatch(_dispatch: &super::service::ScmDispatch) -> Result<Self> {
        let seed = Seed::embedded()?;
        let layout = Layout::discover(&seed)?;
        let store = Store::new(&seed);
        let original = store.read(&seed)?.ok_or(Error::SetupIncomplete)?;
        if !matches!(original.phase, Phase::ServiceConfigured | Phase::Ready) {
            return Err(Error::SetupIncomplete);
        }
        let sid = original
            .service_identity
            .service_sid
            .as_deref()
            .ok_or(Error::ServiceIdentityRefused)?;
        if scm::resolve_service_sid(&seed)? != sid {
            return Err(Error::ServiceIdentityRefused);
        }
        let current =
            RetainedProcess::open(RetainedProcess::current()?.selector.process_id, Some(sid))?;
        if current.token.user.sid != "S-1-5-18"
            || current.token.user.session_id != 0
            || !current.token.service_sid_enabled
            || current.token.app_container
            || current.token.network
            || !ordinal_equal(
                current.image.to_str().ok_or(Error::ImageRefused)?,
                layout
                    .helper_path(&seed)
                    .to_str()
                    .ok_or(Error::ImageRefused)?,
            )?
        {
            return Err(Error::ServiceIdentityRefused);
        }
        // Disable all non-baseline privileges before opening the application
        // inventory. The CLI creation owner may later enable its approved pair
        // only on a short-lived thread token, never on this shared process token.
        seal_host_query_access(sid)?;
        // Preserve startup's original snapshot lifetime: each inventory member
        // is verified then dropped. Diagnostics supply a retaining sink instead.
        let RegisteredImages {
            targets,
            manifest,
            helper,
            launcher,
            app,
            helper_root: _helpers,
        } = seal_registered_images(&seed, &layout, &original, &mut || false, drop)?;
        Ok(Self {
            seed,
            layout,
            targets,
            manifest,
            helper,
            launcher,
            app,
            process: current,
            original,
        })
    }
    pub(super) fn commit_startup(self) -> Result<StartedServiceContext> {
        // The dispatcher has reported RUNNING, making SCM PID valid. The pipe
        // is not listening yet. This observation precedes the short CAS lock.
        let observed = scm::live_process(&self.seed, &self.layout)?;
        if observed.selector != self.process.selector || observed.exited()? {
            return Err(Error::ServiceIdentityRefused);
        }
        let _guard = BootstrapGuard::acquire(&self.seed, false)?;
        let store = Store::new(&self.seed);
        let current = store.read(&self.seed)?.ok_or(Error::StateCorrupt)?;
        if current.digest()? != self.original.digest()? || self.process.exited()? {
            return Err(Error::ScopeConflict);
        }
        let mut next = current.next(current.phase, "startup-incarnation")?;
        next.host_epoch = next
            .host_epoch
            .checked_add(1)
            .filter(|n| *n <= crate::json::SAFE_MAX)
            .ok_or(Error::StateCorrupt)?;
        next.startup_attestation = Some(StartupAttestation {
            process_id: self.process.selector.process_id,
            creation_file_time: self.process.selector.creation_file_time,
            instance_nonce: nonce()?,
            helper_image_sha256: self.helper.sha256.clone(),
            manifest_sha256: next.genesis_manifest_sha256.clone(),
            host_epoch: next.host_epoch,
            observed_registration_revision: current.revision,
        });
        store.commit(&self.seed, Some(&current), &next, None)?;
        drop(_guard);
        Ok(StartedServiceContext {
            context: self,
            startup: next,
        })
    }
}
pub(super) struct StartedServiceContext {
    pub(super) context: InstalledServiceContext,
    pub(super) startup: HostRegistration,
}
impl StartedServiceContext {
    pub(super) fn authenticate_running(&self, current: &HostRegistration) -> Result<()> {
        let own = &self.context;
        let actual = scm::live_process(&own.seed, &own.layout)?;
        if actual.selector != own.process.selector || actual.exited()? {
            return Err(Error::ServiceIdentityRefused);
        }
        check_startup_record(current, &actual.selector, &own.helper.sha256)?;
        let startup = current
            .startup_attestation
            .as_ref()
            .ok_or(Error::SetupIncomplete)?;
        if current.installation_scope_id != own.original.installation_scope_id
            || current.genesis_manifest_sha256 != own.original.genesis_manifest_sha256
        {
            return Err(Error::ServiceIdentityRefused);
        }
        let original = self
            .startup
            .startup_attestation
            .as_ref()
            .ok_or(Error::StateCorrupt)?;
        if current.host_epoch != self.startup.host_epoch
            || startup.instance_nonce != original.instance_nonce
        {
            return Err(Error::ServiceIdentityRefused);
        }
        own.targets.code.recheck()?;
        own.targets.app.recheck()?;
        own.targets.state.recheck()?;
        Ok(())
    }
}
