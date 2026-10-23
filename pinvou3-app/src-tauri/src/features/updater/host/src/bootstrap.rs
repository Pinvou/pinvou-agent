//! First-install composition of two independent owners' immutable genesis.
//! These records initialize storage, not online qualification or preparation.
use crate::{
    error::{Error, Result},
    profile::Seed,
};
use pinvou_upgrade_metadata_owner::{BootstrapAnchor, BootstrapBinding as MetadataBinding};
use pinvou_upgrade_ownership::{BootstrapBinding as OwnershipBinding, InitialOwnership};

pub(crate) fn namespaces() -> Vec<String> {
    [
        "bootstrap-v1",
        pinvou_upgrade_metadata_owner::BOOTSTRAP_NAMESPACE,
        pinvou_upgrade_ownership::BOOTSTRAP_NAMESPACE,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

pub(crate) fn initialized(names: &[String]) -> bool {
    names == namespaces()
}

pub(crate) fn valid_namespaces(names: &[String], profile_kind: &str) -> bool {
    // Old lab registrations remain readable, without claiming owner creation.
    // Native commits cannot upgrade this list or manufacture absent genesis.
    matches!(profile_kind, "lab" | "production")
        && (initialized(names) || profile_kind == "lab" && names == ["bootstrap-v1"])
}

pub(crate) struct OwnerBootstrap {
    pub(crate) metadata: BootstrapAnchor,
    pub(crate) ownership: InitialOwnership,
}
impl OwnerBootstrap {
    pub(crate) fn new(seed: &Seed, scope: &str) -> Result<Self> {
        let seed_hash = seed.seed_sha256();
        let metadata_binding = MetadataBinding::new(
            scope,
            &seed.product,
            &seed.component,
            &seed.target_key,
            &seed.profile_kind,
            &seed.profile_id,
            &seed_hash,
        )
        .map_err(|_| Error::StateCorrupt)?;
        let ownership_binding = OwnershipBinding::new(
            scope,
            &seed.product,
            &seed.component,
            &seed.target_key,
            &seed.profile_kind,
            &seed.profile_id,
            &seed_hash,
        )
        .map_err(|_| Error::StateCorrupt)?;
        let metadata = BootstrapAnchor::new(
            &metadata_binding,
            seed.original_root_bytes(),
            seed.original_root.size,
            &seed.original_root.sha256,
        )
        .map_err(|_| Error::InitialTrustUnavailable)?;
        let ownership =
            InitialOwnership::new(&ownership_binding).map_err(|_| Error::StateCorrupt)?;
        Ok(Self {
            metadata,
            ownership,
        })
    }
    pub(crate) fn check_existing(
        &self,
        metadata: &[u8],
        original_root: &[u8],
        ownership: &[u8],
    ) -> Result<()> {
        self.metadata
            .check_existing(metadata, original_root)
            .map_err(|_| Error::StateCorrupt)?;
        self.ownership
            .check_existing(ownership)
            .map_err(|_| Error::StateCorrupt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_namespace_sets_are_complete_ordered_and_legacy_is_distinct() {
        assert!(valid_namespaces(&["bootstrap-v1".into()], "lab"));
        assert!(!valid_namespaces(&["bootstrap-v1".into()], "production"));
        assert!(!initialized(&["bootstrap-v1".into()]));
        let complete = namespaces();
        assert!(initialized(&complete));
        assert!(valid_namespaces(&complete, "production"));
        assert!(!valid_namespaces(&complete, "unknown"));
        assert!(!valid_namespaces(&complete[..2], "lab"));
        let mut reversed = complete.clone();
        reversed.reverse();
        assert!(!valid_namespaces(&reversed, "lab"));
        let mut extra = complete;
        extra.push("unknown-owner".into());
        assert!(!valid_namespaces(&extra, "lab"));
    }
    #[test]
    fn compiled_anchor_genesis_is_bound_to_the_original_scope() {
        let seed = match Seed::embedded() {
            Ok(seed) => seed,
            Err(Error::ProfileDisabled) => return,
            Err(error) => panic!("invalid compiled public Seed: {error}"),
        };
        let original = OwnerBootstrap::new(&seed, &"a".repeat(64)).unwrap();
        original
            .check_existing(
                original.metadata.record_bytes(),
                original.metadata.root_bytes(),
                original.ownership.record_bytes(),
            )
            .unwrap();
        let another = OwnerBootstrap::new(&seed, &"b".repeat(64)).unwrap();
        assert!(
            original
                .check_existing(
                    another.metadata.record_bytes(),
                    another.metadata.root_bytes(),
                    another.ownership.record_bytes()
                )
                .is_err()
        );
    }
}
