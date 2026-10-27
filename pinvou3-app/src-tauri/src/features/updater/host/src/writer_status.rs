//! Closed journal observations, never live-process census or release authority.
use crate::{Error, error::Result, json};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriterStatusObservation {
    /// Counts only the protected journal, including retained uncertain effects.
    /// Zero does not prove all application writers or native Jobs have stopped.
    JournalOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriterJournalCounts {
    pub reserved: u32,
    pub admitted: u32,
    pub draining: u32,
    pub pending_release: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriterJournalStatus {
    pub protocol_version: u8,
    pub observation: WriterStatusObservation,
    pub installation_scope_id: String,
    pub host_epoch: u64,
    pub journal_revision: u64,
    pub counts: WriterJournalCounts,
}

impl WriterJournalStatus {
    pub(crate) fn validate(&self) -> Result<()> {
        let total = [
            self.counts.reserved,
            self.counts.admitted,
            self.counts.draining,
            self.counts.pending_release,
        ]
        .into_iter()
        .try_fold(0u32, u32::checked_add);
        if self.protocol_version != 1
            || !json::digest(&self.installation_scope_id)
            || self.host_epoch == 0
            || self.host_epoch > json::SAFE_MAX
            || self.journal_revision == 0
            || self.journal_revision > json::SAFE_MAX
            || total
                .is_none_or(|n| n as u64 > pinvou_upgrade_ownership::live::MAX_PARTICIPANTS as u64)
        {
            return Err(Error::IpcInvalid);
        }
        Ok(())
    }

    pub(crate) fn validate_for(&self, scope: &str, epoch: u64) -> Result<()> {
        self.validate()?;
        if self.installation_scope_id != scope || self.host_epoch != epoch {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }

    /// Serialize diagnostic DATA only. Public fields are not native evidence.
    pub fn diagnostic_json(&self) -> Result<Vec<u8>> {
        self.validate()?;
        json::encode(self, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> WriterJournalStatus {
        WriterJournalStatus {
            protocol_version: 1,
            observation: WriterStatusObservation::JournalOnly,
            installation_scope_id: "a".repeat(64),
            host_epoch: 2,
            journal_revision: 7,
            counts: WriterJournalCounts {
                reserved: 1,
                admitted: 2,
                draining: 3,
                pending_release: 4,
            },
        }
    }

    #[test]
    fn journal_status_has_only_closed_nonsecret_observation_fields() {
        let original = data();
        let raw = original.diagnostic_json().unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 6);
        assert_eq!(value["observation"], "journal_only");
        assert_eq!(value["counts"].as_object().unwrap().len(), 4);
        let decoded: WriterJournalStatus = json::decode(&raw, true).unwrap();
        assert_eq!(decoded, original);
        value["ready"] = serde_json::Value::Bool(true);
        assert!(
            json::decode::<WriterJournalStatus>(&json::encode(&value, true).unwrap(), true)
                .is_err()
        );
        value.as_object_mut().unwrap().remove("ready");
        value["counts"]["processId"] = serde_json::json!(123);
        assert!(
            json::decode::<WriterJournalStatus>(&json::encode(&value, true).unwrap(), true)
                .is_err()
        );
    }

    #[test]
    fn current_binding_and_combined_capacity_are_required_even_for_zero_counts() {
        let mut value = data();
        value.validate_for(&"a".repeat(64), 2).unwrap();
        assert!(value.validate_for(&"b".repeat(64), 2).is_err());
        assert!(value.validate_for(&"a".repeat(64), 3).is_err());
        value.counts.reserved = 119;
        assert!(value.validate().is_ok()); // Combined boundary is exactly 128.
        value.counts.reserved = 120;
        assert!(value.validate().is_err());
        value.counts.reserved = u32::MAX;
        assert!(value.validate().is_err());
        value = data();
        value.journal_revision = 0;
        assert!(value.validate().is_err());
        value = data();
        value.host_epoch = json::SAFE_MAX + 1;
        assert!(value.validate().is_err());
    }
}
