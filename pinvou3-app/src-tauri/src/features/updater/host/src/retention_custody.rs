//! Fixed fresh-install custody DATA, not an absence proof or cleanup authority.
//! Only the native original-intent transaction can establish this genesis.
//! No producer activation, nonzero object insertion or generic owner callback
//! is implemented here. Ordinary downloaded Package bytes are outside its scope.
use crate::{
    error::{Error, Result},
    json,
    trusted_time::{Binding, CounterEvidence, Incarnation},
};
use serde::{Deserialize, Serialize};

pub(crate) const PURPOSE: &str = "zero-retention-custody.v1";
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProducerState {
    NeverEnabled,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Genesis {
    protocol_version: u8,
    purpose: String,
    binding: Binding,
    inventory_revision: u64,
    consume_query_secret: ProducerState,
    local_backup: ProducerState,
    backup_deadline_objects: ProducerState,
}
impl Genesis {
    pub(crate) fn new(binding: Binding) -> Result<Self> {
        binding.validate()?;
        Ok(Self {
            protocol_version: 1,
            purpose: PURPOSE.into(),
            binding,
            inventory_revision: 1,
            consume_query_secret: ProducerState::NeverEnabled,
            local_backup: ProducerState::NeverEnabled,
            backup_deadline_objects: ProducerState::NeverEnabled,
        })
    }
    pub(crate) fn bytes(&self) -> Result<Vec<u8>> {
        json::encode(self, false)
    }
    pub(crate) fn parse(raw: &[u8], binding: &Binding) -> Result<Self> {
        let parsed: Self = json::decode(raw, false)?;
        if parsed != Self::new(binding.clone())? || parsed.bytes()? != raw {
            return Err(Error::StateCorrupt);
        }
        Ok(parsed)
    }
}
/// A native publisher must authenticate the referenced original completion in
/// its transaction. Constructing or parsing this reference conveys no rights.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ZeroRetentionReference {
    pub(crate) completion_revision: u64,
    pub(crate) completion_sha256: String,
}
impl ZeroRetentionReference {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.completion_revision < 2
            || self.completion_revision > json::SAFE_MAX
            || !json::digest(&self.completion_sha256)
        {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Completion {
    pub(crate) protocol_version: u8,
    pub(crate) binding: Binding,
    pub(crate) inventory_revision: u64,
    pub(crate) inventory_sha256: String,
    pub(crate) anchor_time_revision: u64,
    pub(crate) anchor_time_sha256: String,
    pub(crate) completed_time_revision: u64,
    pub(crate) policy_sha256: String,
    pub(crate) incarnation: Incarnation,
    pub(crate) initial_counter: CounterEvidence,
    pub(crate) final_counter: CounterEvidence,
    pub(crate) earliest: u64,
    pub(crate) latest: u64,
}
impl Completion {
    pub(crate) fn validate(&self, binding: &Binding, inventory: &[u8]) -> Result<()> {
        Genesis::parse(inventory, binding)?;
        if self.protocol_version != 1
            || self.binding != *binding
            || self.inventory_revision != 1
            || self.inventory_sha256 != json::sha256(inventory)
            || self.anchor_time_revision == 0
            || self.anchor_time_revision >= json::SAFE_MAX
            || self.completed_time_revision != self.anchor_time_revision + 1
            || !json::digest(&self.anchor_time_sha256)
            || !json::digest(&self.policy_sha256)
            || self.earliest > self.latest
            || self.latest > json::SAFE_MAX
        {
            return Err(Error::StateCorrupt);
        }
        self.incarnation.validate()?;
        self.final_counter
            .data()?
            .elapsed_since(self.initial_counter.data()?)
            .map_err(|_| Error::StateCorrupt)?;
        Ok(())
    }
    pub(crate) fn bytes(&self) -> Result<Vec<u8>> {
        json::encode(self, false)
    }
    pub(crate) fn reference(&self) -> Result<ZeroRetentionReference> {
        Ok(ZeroRetentionReference {
            completion_revision: self.completed_time_revision,
            completion_sha256: json::sha256(&self.bytes()?),
        })
    }
    pub(crate) fn parse(raw: &[u8], binding: &Binding, inventory: &[u8]) -> Result<Self> {
        let parsed: Self = json::decode(raw, false)?;
        parsed.validate(binding, inventory)?;
        if parsed.bytes()? != raw {
            return Err(Error::StateCorrupt);
        }
        Ok(parsed)
    }
}

/// The already-specified recovery scan ceiling, not a download/network
/// deadline or a configurable freshness policy. Actual native callers supply
/// the original response receive bracket and final same-incarnation sample.
pub(crate) fn check_first_scan(
    received: &CounterEvidence,
    final_counter: &CounterEvidence,
) -> Result<()> {
    let elapsed = final_counter.data()?.elapsed_since(received.data()?)?;
    if elapsed.suspend_inclusive().ceil_millis()? > 10 * 60 * 1000 {
        return Err(Error::SourceUnavailable);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> Binding {
        Binding {
            installation_scope_id: "scope".into(),
            product: "pinvou".into(),
            component: "agent".into(),
            target_key: "windows-x86_64-nsis-perMachine".into(),
            profile_kind: "lab".into(),
            profile_id: "profile".into(),
            genesis_seed_sha256: "1".repeat(64),
            genesis_manifest_sha256: "2".repeat(64),
            original_install_operation_id: "original".into(),
        }
    }
    #[test]
    fn genesis_is_closed_scope_bound_and_cannot_claim_an_enabled_producer() {
        let b = binding();
        let raw = Genesis::new(b.clone()).unwrap().bytes().unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        value["consumeQuerySecret"] = serde_json::json!("enabled");
        assert!(Genesis::parse(&json::encode(&value, false).unwrap(), &b).is_err());
        value = serde_json::from_slice(&raw).unwrap();
        value["inventoryRevision"] = serde_json::json!(2);
        assert!(Genesis::parse(&json::encode(&value, false).unwrap(), &b).is_err());
        let mut other = b.clone();
        other.installation_scope_id = "other".into();
        assert!(Genesis::parse(&raw, &other).is_err());
        assert!(Genesis::parse(br#"{"protocolVersion":1,"protocolVersion":1}"#, &b).is_err());
        assert!(Genesis::parse(b"{}", &b).is_err());
        let value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert!(Genesis::parse(&serde_json::to_vec_pretty(&value).unwrap(), &b).is_err());
        assert!(Genesis::parse(&raw, &b).is_ok()); // DATA only, no native proof.
    }
    #[test]
    fn completion_cannot_move_scope_inventory_parent_or_incarnation_silently() {
        let b = binding();
        let raw = Genesis::new(b.clone()).unwrap().bytes().unwrap();
        let counter = |n| {
            CounterEvidence::from_data(
                crate::monotonic::CounterObservation::from_bracket(n, n / 2, n / 2 + 1, n + 1)
                    .unwrap(),
            )
        };
        let c = Completion {
            protocol_version: 1,
            binding: b.clone(),
            inventory_revision: 1,
            inventory_sha256: json::sha256(&raw),
            anchor_time_revision: 3,
            anchor_time_sha256: "3".repeat(64),
            completed_time_revision: 4,
            policy_sha256: "4".repeat(64),
            incarnation: Incarnation {
                host_epoch: 1,
                instance_nonce: "5".repeat(64),
                process: crate::identity::ProcessSelector {
                    process_id: 1,
                    creation_file_time: crate::identity::FileTime::from_native(0, 1),
                },
            },
            initial_counter: counter(100),
            final_counter: counter(200),
            earliest: 10,
            latest: 12,
        };
        c.validate(&b, &raw).unwrap();
        for field in [
            "inventorySha256",
            "anchorTimeRevision",
            "completedTimeRevision",
            "binding",
            "finalCounter",
            "latest",
        ] {
            let mut v = serde_json::to_value(&c).unwrap();
            v[field] = match field {
                "inventorySha256" => serde_json::json!("9".repeat(64)),
                "anchorTimeRevision" => serde_json::json!(4),
                "completedTimeRevision" => serde_json::json!(5),
                "binding" => serde_json::json!({}),
                "finalCounter" => serde_json::to_value(counter(50)).unwrap(),
                _ => serde_json::json!(9),
            };
            assert!(Completion::parse(&json::encode(&v, false).unwrap(), &b, &raw).is_err());
        }
        let mut other = b;
        other.original_install_operation_id = "another".into();
        assert!(Completion::parse(&c.bytes().unwrap(), &other, &raw).is_err());
    }
    #[test]
    fn scan_bound_includes_suspension_lock_wait_and_fractional_millisecond() {
        let sample = |inclusive, awake| {
            CounterEvidence::from_data(
                crate::monotonic::CounterObservation::from_bracket(
                    inclusive, awake, awake, inclusive,
                )
                .unwrap(),
            )
        };
        let start = sample(10_000, 10_000);
        check_first_scan(&start, &sample(10_000 + 600_000 * 10_000, 20_000)).unwrap();
        assert!(check_first_scan(&start, &sample(10_001 + 600_000 * 10_000, 20_000)).is_err());
        assert!(check_first_scan(&start, &sample(9_999, 9_999)).is_err());
    }
}
