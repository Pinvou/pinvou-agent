//! Actual SCM service image/token/registration and startup incarnation owner.
//! A deserialized registration is only an expectation, never this constructor.
use super::{
    image::{SealedImage, verify_bytes},
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
        let manifest = store.manifest(&seed, &original)?;
        if manifest.helper != original.helper
            || manifest.launcher != original.launcher
            || manifest.app != original.app
        {
            return Err(Error::StateCorrupt);
        }
        let targets = layout.open_targets(sid)?;
        targets.validate_known_contents(&seed, &manifest, true)?;
        for entry in &manifest.app_files {
            let mut file = targets.app.relative_file(&entry.relative_name)?;
            verify_bytes(&mut file, entry.size, &entry.sha256)?;
        }
        let helpers = targets
            .code
            .child("helpers", false, false, Some(sid))?
            .child(&seed.build_id, false, false, Some(sid))?;
        let helper = SealedImage::open(
            &helpers,
            "pinvou-upgrade-helper.exe",
            &original.helper,
            &seed,
        )?;
        let launcher = SealedImage::open(
            &targets.code,
            "pinvou-upgrade-launcher.exe",
            &original.launcher,
            &seed,
        )?;
        let app = SealedImage::open(&targets.app, "pinvou3-tauri.exe", &original.app, &seed)?;
        seal_host_query_access(sid)?;
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
        let startup = current
            .startup_attestation
            .as_ref()
            .ok_or(Error::SetupIncomplete)?;
        if startup.process_id != actual.selector.process_id
            || startup.creation_file_time != actual.selector.creation_file_time
            || startup.host_epoch != current.host_epoch
            || startup.helper_image_sha256 != own.helper.sha256
            || current.installation_scope_id != own.original.installation_scope_id
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
