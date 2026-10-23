//! Kernel-derived pipe/process/token/registered-image peer roles only.
//! Source peer is a read-only probe role, NOT a SourceInstallerContext writer.
use super::{
    image::SealedImage,
    installed::StartedServiceContext,
    layout::ordinal_equal,
    pipe::Pipe,
    process::{RetainedProcess, TokenFacts},
};
use crate::{
    error::{Error, Result},
    handoff::{PeerKey, Role},
    registration::HostRegistration,
};
pub(super) struct Peer {
    pub(super) process: std::sync::Arc<RetainedProcess>,
    pub(super) role: Role,
    source: Option<SourcePeer>,
}
struct SourcePeer {
    parent: RetainedProcess,
    _helper: SealedImage,
    _installer: SealedImage,
}
fn equal_path(left: &std::path::Path, right: &std::path::Path) -> Result<bool> {
    ordinal_equal(
        left.to_str().ok_or(Error::PeerRefused)?,
        right.to_str().ok_or(Error::PeerRefused)?,
    )
}
fn same_token(left: &TokenFacts, right: &TokenFacts) -> bool {
    left.user == right.user
        && left.integrity == right.integrity
        && left.elevated == right.elevated
        && left.administrator == right.administrator
        && left.app_container == right.app_container
        && left.network == right.network
        && left.interactive == right.interactive
}
fn acceptable(token: &TokenFacts) -> bool {
    token.interactive
        && token.user.session_id != 0
        && token.user.sid != "S-1-5-18"
        && !token.app_container
        && !token.network
        && token.integrity >= 0x2000
}
impl Peer {
    /// Must follow the bounded initial read on this same actual pipe instance.
    pub(super) fn authenticate(
        pipe: &Pipe,
        host: &StartedServiceContext,
        record: &HostRegistration,
    ) -> Result<Self> {
        let id = pipe.peer_process_id()?;
        let session = pipe.peer_session_id()?;
        let identification = pipe.identify_client()?; // RAII reverted before file/registry IO.
        let process = RetainedProcess::open(id, None)?;
        if process.token.user.session_id != session
            || !same_token(&identification, &process.token)
            || process.token.app_container
            || process.token.network
            || process.token.integrity < 0x2000
        {
            return Err(Error::PeerRefused);
        }
        let context = &host.context;
        let (role, source) = if equal_path(&process.image, &context.layout.app_path())? {
            (Role::App, None)
        } else if equal_path(&process.image, &context.layout.launcher_path())? {
            (Role::Launcher, None)
        } else if equal_path(
            &process.image,
            &context.layout.source.join("pinvou-upgrade-helper.exe"),
        )? {
            if !process.token.elevated
                || !process.token.administrator
                || process.token.integrity < 0x3000
            {
                return Err(Error::PeerRefused);
            }
            let parent = process.parent()?;
            let original = &record.installer_identity;
            if parent.token.user.sid != original.token_user_sid
                || parent.token.user.logon != original.logon_authentication_id
                || parent.token.user.session_id != original.session_id
                || parent.selector.process_id != original.process_id
                || parent.selector.creation_file_time != original.creation_file_time
                || parent.selector.creation_file_time.ticks()
                    >= process.selector.creation_file_time.ticks()
                || parent.token.user != process.token.user
                || !parent.token.elevated
                || !parent.token.administrator
                || parent.token.integrity < 0x3000
                || parent.token.app_container
                || parent.token.network
                || parent.exited()?
                || !equal_path(&parent.image, &context.layout.source.join("installer.exe"))?
            {
                return Err(Error::PeerRefused);
            }
            let root = context.layout.source_root()?;
            let helper = SealedImage::open(
                &root,
                "pinvou-upgrade-helper.exe",
                &record.helper,
                &context.seed,
            )?;
            let installer = SealedImage::open_registered_installer(&root, record, &context.seed)?;
            if installer.sha256 != original.source_nsis_sha256
                || installer.identity != original.source_nsis_file_identity
            {
                return Err(Error::PeerRefused);
            }
            (
                Role::SourceInstaller,
                Some(SourcePeer {
                    parent,
                    _helper: helper,
                    _installer: installer,
                }),
            )
        } else {
            return Err(Error::PeerRefused);
        };
        if role != Role::SourceInstaller && !acceptable(&process.token) {
            return Err(Error::PeerRefused);
        }
        let value = Self {
            process: std::sync::Arc::new(process),
            role,
            source,
        };
        value.recheck(pipe)?;
        Ok(value)
    }
    pub(super) fn recheck(&self, pipe: &Pipe) -> Result<()> {
        if self.process.exited()?
            || pipe.peer_process_id()? != self.process.selector.process_id
            || pipe.peer_session_id()? != self.process.token.user.session_id
        {
            return Err(Error::PeerRefused);
        }
        let current = self.process.fresh_token(None)?;
        if !same_token(&current, &self.process.token)
            || (self.role != Role::SourceInstaller && !acceptable(&current))
        {
            return Err(Error::PeerRefused);
        }
        if let Some(source) = &self.source {
            if source.parent.exited()? {
                return Err(Error::PeerRefused);
            }
        }
        Ok(())
    }
    pub(super) fn key(&self) -> PeerKey {
        PeerKey {
            process: self.process.selector.clone(),
            user_sid: self.process.token.user.sid.clone(),
            logon: self.process.token.user.logon,
            session_id: self.process.token.user.session_id,
        }
    }
}
