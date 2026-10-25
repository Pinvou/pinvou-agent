//! Protected time-owner DATA and retained cryptographic evidence. No parsed
//! record grants UTC continuity, cleanup completion or execution permission.
use crate::{
    error::{Error, Result},
    identity::ProcessSelector,
    json,
    monotonic::CounterObservation,
    registration::HostRegistration,
};
use pinvou_upgrade_codec::{Document, InputKind, decode_canonical_envelope};
use pinvou_upgrade_verifier::{
    SuppliedTimeObservation, TimeVerificationInputs, verify_time_response,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Binding {
    pub(crate) installation_scope_id: String,
    pub(crate) product: String,
    pub(crate) component: String,
    pub(crate) target_key: String,
    pub(crate) profile_kind: String,
    pub(crate) profile_id: String,
    pub(crate) genesis_seed_sha256: String,
    pub(crate) genesis_manifest_sha256: String,
    pub(crate) original_install_operation_id: String,
}
impl Binding {
    pub(crate) fn from_host(host: &HostRegistration) -> Self {
        Self {
            installation_scope_id: host.installation_scope_id.clone(),
            product: host.product.clone(),
            component: host.component.clone(),
            target_key: host.target_key.clone(),
            profile_kind: host.profile_kind.clone(),
            profile_id: host.profile_id.clone(),
            genesis_seed_sha256: host.genesis_seed_sha256.clone(),
            genesis_manifest_sha256: host.genesis_manifest_sha256.clone(),
            original_install_operation_id: host.original_install_operation_id.clone(),
        }
    }
    fn validate(&self) -> Result<()> {
        if ![
            &self.product,
            &self.component,
            &self.target_key,
            &self.profile_kind,
            &self.profile_id,
        ]
        .iter()
        .all(|s| json::identifier(s))
            || self.installation_scope_id.is_empty()
            || self.installation_scope_id.len() > 128
            || self.original_install_operation_id.is_empty()
            || self.original_install_operation_id.len() > 128
            || !json::digest(&self.genesis_seed_sha256)
            || !json::digest(&self.genesis_manifest_sha256)
        {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Incarnation {
    pub(crate) host_epoch: u64,
    pub(crate) instance_nonce: String,
    pub(crate) process: ProcessSelector,
}
impl Incarnation {
    pub(crate) fn from_host(host: &HostRegistration) -> Result<Self> {
        let a = host
            .startup_attestation
            .as_ref()
            .ok_or(Error::SetupIncomplete)?;
        let value = Self {
            host_epoch: host.host_epoch,
            instance_nonce: a.instance_nonce.clone(),
            process: ProcessSelector {
                process_id: a.process_id,
                creation_file_time: a.creation_file_time,
            },
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<()> {
        if self.host_epoch == 0
            || self.host_epoch > json::SAFE_MAX
            || !json::digest(&self.instance_nonce)
        {
            return Err(Error::StateCorrupt);
        }
        self.process.validate().map_err(|_| Error::StateCorrupt)
    }
}
/// Fixed protected registry Policy DATA, explicitly configured by an administrator.
/// No defaults or ordinary environment/file source. Changes cannot reset state.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Policy {
    pub(crate) protocol_version: u8,
    pub(crate) binding: Binding,
    pub(crate) max_round_trip_ms: u64,
    pub(crate) max_uncertainty_ms: u64,
    pub(crate) denied_key_ids: Vec<String>,
}
impl Policy {
    pub(crate) fn parse(bytes: &[u8], binding: &Binding) -> Result<Self> {
        let p: Self = json::decode(bytes, false)?;
        if p.protocol_version != 1
            || p.binding != *binding
            || p.max_round_trip_ms > json::SAFE_MAX
            || p.max_uncertainty_ms > json::SAFE_MAX
            || json::encode(&p, false)? != bytes
        {
            return Err(Error::StateCorrupt);
        }
        p.binding.validate()?;
        check_denied(&p.denied_key_ids)?;
        Ok(p)
    }
}
fn check_denied(ids: &[String]) -> Result<()> {
    if ids.len() > 4096
        || ids.iter().any(|s| !json::digest(s))
        || ids.windows(2).any(|a| a[0] >= a[1])
    {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}

/// Decimal strings preserve all native u64 ticks; safe-JSON numbers are used
/// only after checked enclosing millisecond conversion in the existing module.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CounterEvidence {
    interrupt_before: String,
    awake_before: String,
    awake_after: String,
    interrupt_after: String,
}
impl CounterEvidence {
    pub(crate) fn from_data(c: CounterObservation) -> Self {
        Self {
            interrupt_before: c.suspend_inclusive().earliest_100ns().to_string(),
            interrupt_after: c.suspend_inclusive().latest_100ns().to_string(),
            awake_before: c.awake().earliest_100ns().to_string(),
            awake_after: c.awake().latest_100ns().to_string(),
        }
    }
    fn data(&self) -> Result<CounterObservation> {
        fn ticks(s: &str) -> Result<u64> {
            if s.is_empty()
                || s.len() > 20
                || s.len() > 1 && s.starts_with('0')
                || !s.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(Error::StateCorrupt);
            }
            s.parse().map_err(|_| Error::StateCorrupt)
        }
        CounterObservation::from_bracket(
            ticks(&self.interrupt_before)?,
            ticks(&self.awake_before)?,
            ticks(&self.awake_after)?,
            ticks(&self.interrupt_after)?,
        )
        .map_err(|_| Error::StateCorrupt)
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Evidence {
    pub(crate) incarnation: Incarnation,
    pub(crate) request: String,
    pub(crate) response: String,
    pub(crate) sent: CounterEvidence,
    pub(crate) received: CounterEvidence,
    pub(crate) policy: String,
    pub(crate) floor_before: u64,
    pub(crate) base_root_count: usize,
    pub(crate) denied_key_ids: Vec<String>,
    pub(crate) earliest: u64,
    pub(crate) latest: u64,
}
impl Evidence {
    pub(crate) fn verify(
        &self,
        binding: &Binding,
        original: &[u8],
        roots: &[String],
    ) -> Result<()> {
        self.incarnation.validate()?;
        let policy = Policy::parse(self.policy.as_bytes(), binding)?;
        check_denied(&self.denied_key_ids)?;
        if !policy
            .denied_key_ids
            .iter()
            .all(|id| self.denied_key_ids.contains(id))
        {
            return Err(Error::StateCorrupt);
        }
        let sent = self.sent.data()?;
        let received = self.received.data()?;
        received
            .elapsed_since(sent)
            .map_err(|_| Error::StateCorrupt)?;
        let request = Document::parse(self.request.as_bytes(), InputKind::Credential)
            .map_err(|_| Error::StateCorrupt)?
            .time_request_shape()
            .map_err(|_| Error::StateCorrupt)?;
        if request.canonical_bytes() != self.request.as_bytes()
            || request.product() != binding.product
            || request.component() != binding.component
        {
            return Err(Error::StateCorrupt);
        }
        if self.base_root_count > roots.len() {
            return Err(Error::StateCorrupt);
        }
        let references: Vec<&[u8]> = roots.iter().map(|s| s.as_bytes()).collect();
        let checked = verify_time_response(
            self.response.as_bytes(),
            TimeVerificationInputs::new(
                original,
                &references,
                &request,
                SuppliedTimeObservation {
                    sent_monotonic_millis: sent.suspend_inclusive().floor_millis()?,
                    received_monotonic_millis: received.suspend_inclusive().ceil_millis()?,
                    max_round_trip_millis: policy.max_round_trip_ms,
                    max_uncertainty_millis: policy.max_uncertainty_ms,
                    trusted_time_floor_millis: self.floor_before,
                },
                &[],
            ),
        )
        .map_err(|_| Error::StateCorrupt)?;
        // Authenticate retained lineage without retroactively revoking an
        // already protected historical Root. Apply current known deny to the
        // actual protected predecessor + every newly accepted successor and
        // the time signature. transition() binds this predecessor to old state.
        let base = if self.base_root_count == 0 {
            original
        } else {
            roots[self.base_root_count - 1].as_bytes()
        };
        verify_time_response(
            self.response.as_bytes(),
            TimeVerificationInputs::new(
                base,
                &references[self.base_root_count..],
                &request,
                SuppliedTimeObservation {
                    sent_monotonic_millis: sent.suspend_inclusive().floor_millis()?,
                    received_monotonic_millis: received.suspend_inclusive().ceil_millis()?,
                    max_round_trip_millis: policy.max_round_trip_ms,
                    max_uncertainty_millis: policy.max_uncertainty_ms,
                    trusted_time_floor_millis: self.floor_before,
                },
                &self.denied_key_ids,
            ),
        )
        .map_err(|_| Error::StateCorrupt)?;
        if checked.earliest_millis() != self.earliest || checked.latest_millis() != self.latest {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Unknown,
    CleanupPending,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct State {
    pub(crate) protocol_version: u8,
    pub(crate) binding: Binding,
    pub(crate) revision: u64,
    pub(crate) previous_sha256: Option<String>,
    pub(crate) incarnation: Option<Incarnation>,
    pub(crate) status: Status,
    pub(crate) cleanup_pending: bool,
    pub(crate) floor: Option<u64>,
    /// Exact canonical successors, starting at immutable original Root + 1.
    pub(crate) roots: Vec<String>,
    pub(crate) denied_key_ids: Vec<String>,
    pub(crate) evidence: Option<Evidence>,
}
impl State {
    pub(crate) fn genesis(binding: Binding, original: &[u8]) -> Result<Self> {
        root_version(original, &binding.product)?;
        let state = Self {
            protocol_version: 1,
            binding,
            revision: 1,
            previous_sha256: None,
            incarnation: None,
            status: Status::Unknown,
            cleanup_pending: true,
            floor: None,
            roots: Vec::new(),
            denied_key_ids: Vec::new(),
            evidence: None,
        };
        state.validate(original)?;
        Ok(state)
    }
    pub(crate) fn current_root<'a>(&'a self, original: &'a [u8]) -> &'a [u8] {
        self.roots.last().map_or(original, |s| s.as_bytes())
    }
    pub(crate) fn parse(raw: &[u8], original: &[u8]) -> Result<Self> {
        let s: Self = json::decode(raw, false)?;
        if json::encode(&s, false)? != raw {
            return Err(Error::StateCorrupt);
        }
        s.validate(original)?;
        Ok(s)
    }
    pub(crate) fn validate(&self, original: &[u8]) -> Result<()> {
        self.binding.validate()?;
        check_denied(&self.denied_key_ids)?;
        if self.protocol_version != 1
            || !self.cleanup_pending
            || self.revision == 0
            || self.revision > json::SAFE_MAX
            || self.roots.len() > 4096
            || self.floor.is_some_and(|n| n > json::SAFE_MAX)
            || (self.revision == 1) != (self.previous_sha256.is_none())
            || self
                .previous_sha256
                .as_ref()
                .is_some_and(|s| !json::digest(s))
        {
            return Err(Error::StateCorrupt);
        }
        if let Some(i) = &self.incarnation {
            i.validate()?;
        }
        let mut version = root_version(original, &self.binding.product)?;
        for root in &self.roots {
            let next = root_version(root.as_bytes(), &self.binding.product)?;
            if version.checked_add(1) != Some(next) {
                return Err(Error::StateCorrupt);
            }
            version = next;
        }
        match &self.evidence {
            None if self.floor.is_none()
                && self.roots.is_empty()
                && self.status == Status::Unknown => {}
            Some(e) if self.floor == Some(e.earliest) && self.incarnation.is_some() => {
                e.verify(&self.binding, original, &self.roots)?;
                if !e
                    .denied_key_ids
                    .iter()
                    .all(|id| self.denied_key_ids.contains(id))
                {
                    return Err(Error::StateCorrupt);
                }
            }
            _ => return Err(Error::StateCorrupt),
        }
        Ok(())
    }
    pub(crate) fn next_unknown(&self, incarnation: Incarnation, previous: &[u8]) -> Result<Self> {
        let mut next = self.clone();
        next.revision = next
            .revision
            .checked_add(1)
            .filter(|n| *n <= json::SAFE_MAX)
            .ok_or(Error::Unavailable)?;
        next.previous_sha256 = Some(json::sha256(previous));
        next.incarnation = Some(incarnation);
        next.status = Status::Unknown;
        Ok(next)
    }
    pub(crate) fn transition(&self, next: &Self, previous: &[u8], original: &[u8]) -> Result<()> {
        next.validate(original)?;
        if next.binding != self.binding
            || next.revision != self.revision.checked_add(1).ok_or(Error::Unavailable)?
            || next.previous_sha256 != Some(json::sha256(previous))
            || next.floor.unwrap_or(0) < self.floor.unwrap_or(0)
            || !next.roots.starts_with(&self.roots)
            || !self
                .denied_key_ids
                .iter()
                .all(|id| next.denied_key_ids.contains(id))
        {
            return Err(Error::StateCorrupt);
        }
        match next.status {
            Status::Unknown => {
                if next.floor != self.floor
                    || next.roots != self.roots
                    || next.evidence != self.evidence
                {
                    return Err(Error::StateCorrupt);
                }
            }
            Status::CleanupPending => {
                let e = next.evidence.as_ref().ok_or(Error::StateCorrupt)?;
                if next.incarnation != self.incarnation
                    || Some(&e.incarnation) != next.incarnation.as_ref()
                    || next.denied_key_ids != e.denied_key_ids
                    || e.floor_before != self.floor.unwrap_or(0)
                    || e.base_root_count != self.roots.len()
                {
                    return Err(Error::StateCorrupt);
                }
            }
        }
        Ok(())
    }
}
pub(crate) fn root_version(raw: &[u8], product: &str) -> Result<u64> {
    let env = decode_canonical_envelope(raw, InputKind::Metadata)
        .map_err(|_| Error::StateCorrupt)?
        .check_full_schema()
        .map_err(|_| Error::StateCorrupt)?;
    let shape = env
        .envelope()
        .signed()
        .root_policy_shape()
        .map_err(|_| Error::StateCorrupt)?;
    if shape.product() != product {
        return Err(Error::StateCorrupt);
    }
    Ok(shape.version())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn vector() -> serde_json::Value {
        serde_json::from_slice(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../specs/upgrade/v1/vectors/time/anchor.json"
        )))
        .unwrap()
    }
    fn binding() -> Binding {
        Binding {
            installation_scope_id: "scope".into(),
            product: "pinvou".into(),
            component: "app".into(),
            target_key: "windows-x86_64-nsis-perMachine".into(),
            profile_kind: "production".into(),
            profile_id: "host".into(),
            genesis_seed_sha256: "a".repeat(64),
            genesis_manifest_sha256: "b".repeat(64),
            original_install_operation_id: "original".into(),
        }
    }
    fn policy() -> Policy {
        Policy {
            protocol_version: 1,
            binding: binding(),
            max_round_trip_ms: 20,
            max_uncertainty_ms: 5,
            denied_key_ids: Vec::new(),
        }
    }
    fn incarnation(epoch: u64) -> Incarnation {
        Incarnation {
            host_epoch: epoch,
            instance_nonce: "c".repeat(64),
            process: ProcessSelector {
                process_id: 3,
                creation_file_time: crate::identity::FileTime::from_native(1, 2),
            },
        }
    }
    fn anchored() -> (State, Vec<u8>, Vec<u8>) {
        let v = vector();
        let c = &v["cases"][0];
        let original = c["trustedRootBytes"].as_str().unwrap().as_bytes().to_vec();
        let initial = State::genesis(binding(), &original).unwrap();
        let before = json::encode(&initial, false).unwrap();
        let unknown = initial.next_unknown(incarnation(1), &before).unwrap();
        let raw = json::encode(&unknown, false).unwrap();
        let mut s = unknown.clone();
        s.revision += 1;
        s.previous_sha256 = Some(json::sha256(&raw));
        s.status = Status::CleanupPending;
        s.roots = c["rootSuccessors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_owned())
            .collect();
        s.floor = Some(c["expected"]["earliest"].as_u64().unwrap());
        s.evidence = Some(Evidence {
            incarnation: incarnation(1),
            request: serde_json::to_string(&c["request"]).unwrap(),
            response: c["responseBytes"].as_str().unwrap().into(),
            sent: CounterEvidence::from_data(
                CounterObservation::from_bracket(10_000_000, 9_000_000, 9_000_000, 10_000_000)
                    .unwrap(),
            ),
            received: CounterEvidence::from_data(
                CounterObservation::from_bracket(10_200_000, 9_200_000, 9_200_000, 10_200_000)
                    .unwrap(),
            ),
            policy: String::from_utf8(json::encode(&policy(), false).unwrap()).unwrap(),
            floor_before: 0,
            base_root_count: 0,
            denied_key_ids: Vec::new(),
            earliest: s.floor.unwrap(),
            latest: c["expected"]["latest"].as_u64().unwrap(),
        });
        unknown.transition(&s, &raw, &original).unwrap();
        (s, raw, original)
    }
    #[test]
    fn retained_real_signature_vector_rejects_response_root_nonce_and_floor_mutation() {
        let (s, _, original) = anchored();
        let bytes = json::encode(&s, false).unwrap();
        State::parse(&bytes, &original).unwrap();
        for change in 0..4 {
            let mut bad = s.clone();
            match change {
                0 => bad.floor = Some(bad.floor.unwrap() + 1),
                1 => {
                    bad.evidence.as_mut().unwrap().response = bad
                        .evidence
                        .as_ref()
                        .unwrap()
                        .response
                        .replace("1700000000100", "1700000000101")
                }
                2 => {
                    bad.evidence.as_mut().unwrap().request = bad
                        .evidence
                        .as_ref()
                        .unwrap()
                        .request
                        .replace("BwcHBwc", "AwcHBwc")
                }
                _ => bad.roots.clear(),
            }
            assert!(bad.validate(&original).is_err());
        }
    }
    #[test]
    fn incarnation_unknown_retains_floor_original_evidence_and_uncleared_barrier() {
        let (s, _, original) = anchored();
        let raw = json::encode(&s, false).unwrap();
        let next = s.next_unknown(incarnation(2), &raw).unwrap();
        s.transition(&next, &raw, &original).unwrap();
        assert!(next.status == Status::Unknown && next.cleanup_pending && next.floor == s.floor);
        assert!(next.evidence.as_ref().unwrap().incarnation == incarnation(1));
        assert!(next.incarnation == Some(incarnation(2)));
        let next_raw = json::encode(&next, false).unwrap();
        let mut stale = next.clone();
        stale.revision += 1;
        stale.previous_sha256 = Some(json::sha256(&next_raw));
        stale.status = Status::CleanupPending;
        let e = stale.evidence.as_mut().unwrap();
        e.floor_before = next.floor.unwrap();
        e.base_root_count = next.roots.len();
        // Historical counters must retain the original incarnation and cannot
        // become a fresh anchor merely by changing the current incarnation.
        assert!(next.transition(&stale, &next_raw, &original).is_err());
        let mut bad = next.clone();
        bad.floor = None;
        assert!(s.transition(&bad, &raw, &original).is_err());
        let mut bad = next.clone();
        bad.cleanup_pending = false;
        assert!(bad.validate(&original).is_err());
        let mut bad = next;
        bad.binding.genesis_manifest_sha256 = "d".repeat(64);
        assert!(s.transition(&bad, &raw, &original).is_err());
    }
    #[test]
    fn observed_deny_survives_failed_refresh_and_cannot_be_omitted_from_new_evidence() {
        let (old, _, original) = anchored();
        let raw = json::encode(&old, false).unwrap();
        let mut pending = old.next_unknown(incarnation(1), &raw).unwrap();
        pending.denied_key_ids.push("a".repeat(64));
        old.transition(&pending, &raw, &original).unwrap();
        let saved = json::encode(&pending, false).unwrap();
        let mut forgotten = pending.next_unknown(incarnation(1), &saved).unwrap();
        forgotten.denied_key_ids.clear();
        assert!(pending.transition(&forgotten, &saved, &original).is_err());
        // The real signed vector is reused only as DATA to isolate this
        // transition guard; this does not assert a new live nonce/SCM source.
        let mut fresh = pending.clone();
        fresh.revision += 1;
        fresh.previous_sha256 = Some(json::sha256(&saved));
        fresh.status = Status::CleanupPending;
        let e = fresh.evidence.as_mut().unwrap();
        e.floor_before = pending.floor.unwrap();
        e.base_root_count = pending.roots.len();
        assert!(pending.transition(&fresh, &saved, &original).is_err());
        fresh.evidence.as_mut().unwrap().denied_key_ids = pending.denied_key_ids.clone();
        pending.transition(&fresh, &saved, &original).unwrap();
    }
    #[test]
    fn policy_has_no_missing_defaults_and_is_bound_to_original_scope() {
        let p = policy();
        let raw = json::encode(&p, false).unwrap();
        Policy::parse(&raw, &binding()).unwrap();
        let mut other = binding();
        other.installation_scope_id = "other".into();
        assert!(Policy::parse(&raw, &other).is_err());
        let mut v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        v.as_object_mut().unwrap().remove("maxUncertaintyMs");
        assert!(Policy::parse(&json::encode(&v, false).unwrap(), &binding()).is_err());
        let mut zero = p;
        zero.max_round_trip_ms = 0;
        zero.max_uncertainty_ms = 0;
        Policy::parse(&json::encode(&zero, false).unwrap(), &binding()).unwrap();
    }
    #[test]
    fn counter_evidence_never_accepts_alias_ticks_or_independently_truncated_bounds() {
        let mut c = CounterEvidence::from_data(
            CounterObservation::from_bracket(9999, 9000, 9001, 10001).unwrap(),
        );
        assert_eq!(
            c.data()
                .unwrap()
                .suspend_inclusive()
                .floor_millis()
                .unwrap(),
            0
        );
        assert_eq!(
            c.data().unwrap().suspend_inclusive().ceil_millis().unwrap(),
            2
        );
        c.interrupt_before = "09999".into();
        assert!(c.data().is_err());
    }
}
