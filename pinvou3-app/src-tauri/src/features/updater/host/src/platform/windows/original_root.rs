//! Original Root custody from the real protected registration, not a GET/DTO.
//! This source is immutable genesis, never accepted-current Root or clock rights.
use super::{installed::StartedServiceContext, registry::Store};
use crate::{
    error::{Error, Result},
    identity::ProcessSelector,
};

#[derive(PartialEq, Eq)]
struct OriginalRootBinding {
    installation_scope_id: String,
    product: String,
    component: String,
    target_key: String,
    profile_kind: String,
    profile_id: String,
    genesis_seed_sha256: String,
    genesis_manifest_sha256: String,
    host_epoch: u64,
    instance_nonce: String,
    process: ProcessSelector,
}

/// Native-custody source minted only by the post-COMMIT SCM context below.
/// No constructor, Clone, Debug, Serialize or parsed-data-to-source entrance.
/// Original bytes are public trust material; this wrapper adds actual custody
/// and incarnation association, not UTC, latest/current metadata or permissions.
pub(super) struct ProtectedOriginalRoot {
    binding: OriginalRootBinding,
    metadata_record: Vec<u8>,
    root: Vec<u8>,
}
impl ProtectedOriginalRoot {
    /// Return captured ORIGINAL registry bytes only after actual source recheck.
    /// The byte slice remains DATA; a later rights decision must recheck its own
    /// live context. No immutable Root read can promise future service liveness.
    pub(super) fn root_bytes<'a>(&'a self, host: &StartedServiceContext) -> Result<&'a [u8]> {
        self.recheck(host)?;
        Ok(&self.root)
    }
    pub(super) fn recheck(&self, host: &StartedServiceContext) -> Result<()> {
        let current = host.original_root_source()?;
        if self.binding != current.binding {
            return Err(Error::ServiceIdentityRefused);
        }
        if self.metadata_record != current.metadata_record || self.root != current.root {
            return Err(Error::StateCorrupt);
        }
        // No Head digest/revision, phase, last transition or other mutable status
        // is retained in the source binding. Legal status updates do not make
        // the original Root permanently stale within this same incarnation.
        Ok(())
    }
}

impl StartedServiceContext {
    /// Only this actually committed and authenticated SCM owner can mint the
    /// source. Missing/legacy/partial bootstrap state never initializes itself.
    pub(super) fn original_root_source(&self) -> Result<ProtectedOriginalRoot> {
        let own = &self.context;
        let store = Store::new(&own.seed);
        let before = store
            .read(&own.seed)?
            .ok_or(Error::InitialTrustUnavailable)?;
        self.authenticate_running(&before)?;
        let original = store.original_root(&own.seed, &before)?;
        let after = store
            .read(&own.seed)?
            .ok_or(Error::InitialTrustUnavailable)?;
        self.authenticate_running(&after)?;
        if before.digest()? != after.digest()? {
            // Capture was not one consistent expected snapshot. The caller can
            // retry capture/recheck against the actual new Head; no state heals.
            return Err(Error::ScopeConflict);
        }
        let startup = after
            .startup_attestation
            .as_ref()
            .ok_or(Error::SetupIncomplete)?;
        let binding = OriginalRootBinding {
            installation_scope_id: after.installation_scope_id.clone(),
            product: after.product.clone(),
            component: after.component.clone(),
            target_key: after.target_key.clone(),
            profile_kind: after.profile_kind.clone(),
            profile_id: after.profile_id.clone(),
            genesis_seed_sha256: after.genesis_seed_sha256.clone(),
            genesis_manifest_sha256: after.genesis_manifest_sha256.clone(),
            host_epoch: after.host_epoch,
            instance_nonce: startup.instance_nonce.clone(),
            process: own.process.selector.clone(),
        };
        Ok(ProtectedOriginalRoot {
            binding,
            metadata_record: original.metadata_record,
            root: original.root,
        })
    }
}
