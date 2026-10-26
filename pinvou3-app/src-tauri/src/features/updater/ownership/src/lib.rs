//! Immutable first-install genesis data, not a preparation/freeze capability.
//! Native protected creation, atomic commit and readback belong to the adapter.
mod binding;
pub mod live;
pub use binding::BootstrapBinding;

use pinvou_upgrade_codec::{Document, InputKind};
use serde::Serialize;

pub const BOOTSTRAP_NAMESPACE: &str = "preparation-owner-bootstrap-v1";
pub const RECORD_MAXIMUM: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidBinding,
    InvalidRecord,
    ExistingStateMismatch,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidBinding => "PREPARATION_BOOTSTRAP_BINDING_INVALID",
            Self::InvalidRecord => "PREPARATION_BOOTSTRAP_RECORD_INVALID",
            Self::ExistingStateMismatch => "PREPARATION_BOOTSTRAP_EXISTING_STATE_MISMATCH",
        })
    }
}
impl std::error::Error for Error {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Record<'a> {
    ownership_bootstrap_version: u8,
    binding: &'a BootstrapBinding,
    owner_epoch: u64,
    freeze_epoch: u64,
    preparation: (),
    freeze: (),
    execution: (),
    reconciliation: (),
}

/// Expected immutable genesis bytes only. No system lock, writer permission,
/// live owner, freeze, trusted clock or recovery authority is constructed here.
pub struct InitialOwnership {
    record: Vec<u8>,
}
impl InitialOwnership {
    pub fn new(binding: &BootstrapBinding) -> Result<Self, Error> {
        binding.validate()?;
        // Epoch zero means none has ever been allocated. A future acquisition
        // must advance it under the real owner; host_epoch is unrelated.
        let value = Record {
            ownership_bootstrap_version: 1,
            binding,
            owner_epoch: 0,
            freeze_epoch: 0,
            preparation: (),
            freeze: (),
            execution: (),
            reconciliation: (),
        };
        let raw = serde_json::to_vec(&value).map_err(|_| Error::InvalidRecord)?;
        let record = Document::parse(&raw, InputKind::Metadata)
            .and_then(|doc| doc.canonical_bytes())
            .map_err(|_| Error::InvalidRecord)?;
        if record.len() > RECORD_MAXIMUM {
            return Err(Error::InvalidRecord);
        }
        Ok(Self { record })
    }
    /// Recognize only this exact canonical genesis, never an advanced active
    /// owner record. Such state is neither adopted nor reset by this module.
    pub fn parse(record: &[u8], binding: &BootstrapBinding) -> Result<Self, Error> {
        let expected = Self::new(binding)?;
        expected.check_existing(record)?;
        Ok(expected)
    }
    pub fn record_bytes(&self) -> &[u8] {
        &self.record
    }
    pub fn check_existing(&self, record: &[u8]) -> Result<(), Error> {
        if record.is_empty() || record.len() > RECORD_MAXIMUM {
            return Err(Error::InvalidRecord);
        }
        // Exact equality also requires the complete closed schema, original
        // binding, canonical wire, epoch zero and all four absent occupancies.
        if record != self.record.as_slice() {
            return Err(Error::ExistingStateMismatch);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn binding(scope: &str) -> BootstrapBinding {
        BootstrapBinding::new(
            scope,
            "pinvou",
            "agent",
            "windows-x86_64-nsis-perMachine",
            "production",
            "stable-installation",
            &"2".repeat(64),
        )
        .unwrap()
    }
    #[test]
    fn genesis_has_no_live_owner_or_occupancy_and_roundtrips_exactly() {
        let binding = binding(&"1".repeat(64));
        let genesis = InitialOwnership::new(&binding).unwrap();
        let record: Value = serde_json::from_slice(genesis.record_bytes()).unwrap();
        assert_eq!(record["ownerEpoch"], 0);
        assert_eq!(record["freezeEpoch"], 0);
        for key in ["preparation", "freeze", "execution", "reconciliation"] {
            assert_eq!(record[key], Value::Null);
        }
        assert_eq!(record["binding"]["installationScopeId"], "1".repeat(64));
        let readback = InitialOwnership::parse(genesis.record_bytes(), &binding).unwrap();
        assert_eq!(readback.record_bytes(), genesis.record_bytes());
        assert!(genesis.check_existing(readback.record_bytes()).is_ok());
    }
    #[test]
    fn existing_epochs_occupancy_or_different_scope_are_never_reset() {
        let original = InitialOwnership::new(&binding(&"1".repeat(64))).unwrap();
        let before = original.record_bytes().to_vec();
        for (key, value) in [
            ("ownerEpoch", json!(1)),
            ("freezeEpoch", json!(1)),
            ("preparation", json!({"ownerEpoch": 1})),
            ("freeze", json!({})),
            ("execution", json!({})),
            ("reconciliation", json!({})),
            ("extra", json!(true)),
        ] {
            let mut changed: Value = serde_json::from_slice(&before).unwrap();
            changed[key] = value;
            assert!(
                original
                    .check_existing(&serde_json::to_vec(&changed).unwrap())
                    .is_err()
            );
        }
        let other = binding(&"3".repeat(64));
        assert!(InitialOwnership::parse(&before, &other).is_err());
        assert_eq!(original.record_bytes(), before);
    }
    #[test]
    fn partial_noncanonical_duplicate_and_oversize_records_refuse() {
        let binding = binding(&"1".repeat(64));
        let original = InitialOwnership::new(&binding).unwrap();
        let value: Value = serde_json::from_slice(original.record_bytes()).unwrap();
        for bad in [
            Vec::new(),
            b"{}".to_vec(),
            vec![b' '; RECORD_MAXIMUM + 1],
            serde_json::to_vec_pretty(&value).unwrap(),
            br#"{"ownerEpoch":0,"ownerEpoch":0}"#.to_vec(),
        ] {
            assert!(InitialOwnership::parse(&bad, &binding).is_err());
        }
    }
}
