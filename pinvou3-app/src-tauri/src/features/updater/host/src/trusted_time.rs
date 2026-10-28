//! Protected time-owner DATA and retained cryptographic evidence. No parsed
//! record grants UTC continuity, cleanup completion or execution permission.
mod policy_fault;
use crate::{
    error::{Error, Result},
    identity::ProcessSelector,
    json,
    monotonic::CounterObservation,
    registration::HostRegistration,
};
use pinvou_upgrade_codec::{Document, InputKind, decode_canonical_envelope};
use pinvou_upgrade_verifier::{SuppliedTimeObservation, verify_time_response_source};
pub(crate) use policy_fault::PolicyFault;
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
    pub(crate) fn validate(&self) -> Result<()> {
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
    pub(crate) fn validate(&self) -> Result<()> {
        if self.host_epoch == 0
            || self.host_epoch > json::SAFE_MAX
            || !json::digest(&self.instance_nonce)
        {
            return Err(Error::StateCorrupt);
        }
        self.process.validate().map_err(|_| Error::StateCorrupt)
    }
}
/// Protected per-original Policy DATA. Only fresh original enrollment writes the
/// approved initial limits; reads never supply a fallback for missing policy.
/// Later administrative changes cannot reset persisted time or budget state.
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
    pub(crate) fn initial(binding: Binding) -> Result<Self> {
        binding.validate()?;
        Ok(Self {
            protocol_version: 1,
            binding,
            max_round_trip_ms: 30_000,
            max_uncertainty_ms: 1_000,
            denied_key_ids: Vec::new(),
        })
    }
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
    pub(crate) fn data(&self) -> Result<CounterObservation> {
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
        let checked = verify_time_response_source(
            self.response.as_bytes(),
            original,
            roots.len(),
            |index| Ok(std::borrow::Cow::Borrowed(roots[index].as_bytes())),
            &request,
            SuppliedTimeObservation {
                sent_monotonic_millis: sent.suspend_inclusive().floor_millis()?,
                received_monotonic_millis: received.suspend_inclusive().ceil_millis()?,
                max_round_trip_millis: policy.max_round_trip_ms,
                max_uncertainty_millis: policy.max_uncertainty_ms,
                trusted_time_floor_millis: self.floor_before,
            },
            &[],
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
        let successors = &roots[self.base_root_count..];
        verify_time_response_source(
            self.response.as_bytes(),
            base,
            successors.len(),
            |index| Ok(std::borrow::Cow::Borrowed(successors[index].as_bytes())),
            &request,
            SuppliedTimeObservation {
                sent_monotonic_millis: sent.suspend_inclusive().floor_millis()?,
                received_monotonic_millis: received.suspend_inclusive().ceil_millis()?,
                max_round_trip_millis: policy.max_round_trip_ms,
                max_uncertainty_millis: policy.max_uncertainty_ms,
                trusted_time_floor_millis: self.floor_before,
            },
            &self.denied_key_ids,
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
    CleanupVerifiedZero,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) policy_fault: Option<PolicyFault>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) zero_retention: Option<crate::retention_custody::ZeroRetentionReference>,
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
            policy_fault: None,
            zero_retention: None,
            floor: None,
            roots: Vec::new(),
            denied_key_ids: Vec::new(),
            evidence: None,
        };
        state.validate(original)?;
        Ok(state)
    }
    /// Pure eligibility DATA only. The native live source must additionally
    /// authenticate the actual store, live SCM incarnation and fresh counters.
    pub(crate) fn anchor_for_live(
        &self,
        actual_policy: &[u8],
        incarnation: &Incarnation,
    ) -> Result<&Evidence> {
        match (
            self.status,
            self.cleanup_pending,
            self.zero_retention.is_some(),
        ) {
            (Status::CleanupPending, true, false) | (Status::CleanupVerifiedZero, false, true) => {}
            _ => return Err(Error::SourceUnavailable),
        }
        let evidence = self.evidence.as_ref().ok_or(Error::SourceUnavailable)?;
        if self.incarnation.as_ref() != Some(incarnation) || evidence.incarnation != *incarnation {
            return Err(Error::ServiceIdentityRefused);
        }
        // Even a loosening or exact semantic rewrite requires a fresh anchor;
        // this source never borrows new policy into an old signed observation.
        Policy::parse(actual_policy, &self.binding)?;
        if actual_policy != evidence.policy.as_bytes()
            || self.denied_key_ids != evidence.denied_key_ids
        {
            return Err(Error::SourceUnavailable);
        }
        if self.floor != Some(evidence.earliest) {
            return Err(Error::StateCorrupt);
        }
        Ok(evidence)
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
        match (self.status, self.cleanup_pending, &self.zero_retention) {
            (Status::Unknown | Status::CleanupPending, true, None) => {}
            (Status::CleanupVerifiedZero, false, Some(reference)) => {
                reference.validate()?;
                if reference.completion_revision != self.revision {
                    return Err(Error::StateCorrupt);
                }
            }
            _ => return Err(Error::StateCorrupt),
        }
        if let Some(i) = &self.incarnation {
            i.validate()?;
        }
        if let Some(fault) = &self.policy_fault {
            fault.validate(
                self.revision,
                self.previous_sha256.as_deref(),
                self.status,
                self.incarnation.as_ref(),
            )?;
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
        next.cleanup_pending = true;
        next.zero_retention = None;
        Ok(next)
    }
    pub(crate) fn transition(&self, next: &Self, previous: &[u8], original: &[u8]) -> Result<()> {
        next.validate(original)?;
        PolicyFault::transition(
            self.policy_fault.as_ref(),
            next.policy_fault.as_ref(),
            next.status,
            next.revision,
            previous,
        )?;
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
            Status::CleanupVerifiedZero => {
                if self.status != Status::CleanupPending
                    || next.floor != self.floor
                    || next.roots != self.roots
                    || next.evidence != self.evidence
                    || next.denied_key_ids != self.denied_key_ids
                    || next.incarnation != self.incarnation
                {
                    return Err(Error::StateCorrupt);
                }
                // This proves a transition shape only. The native fixed publisher
                // authenticates the inventory/completion in the SAME transaction.
            }
            Status::CleanupPending => {
                let e = next.evidence.as_ref().ok_or(Error::StateCorrupt)?;
                if next.incarnation != self.incarnation
                    || Some(&e.incarnation) != next.incarnation.as_ref()
                    || next.denied_key_ids != e.denied_key_ids
                    // Native publication additionally requires exact equality
                    // with max(actual time floor, protected budget live floor).
                    || e.floor_before < self.floor.unwrap_or(0)
                    || e.base_root_count != self.roots.len()
                    || self.policy_fault.as_ref().is_some_and(PolicyFault::is_active)
                        && self.evidence.as_ref().is_some_and(|old| old.request == e.request)
                {
                    return Err(Error::StateCorrupt);
                }
            }
        }
        Ok(())
    }
    /// Transition DATA only. Native production callers must positively observe
    /// invalid protected Policy bytes and authenticate the intact original state.
    pub(crate) fn next_policy_fault(
        &self,
        incarnation: Incarnation,
        previous: &[u8],
        policy_sha256: String,
    ) -> Result<Self> {
        if !json::digest(&policy_sha256)
            || self
                .policy_fault
                .as_ref()
                .is_some_and(PolicyFault::is_active)
        {
            return Err(Error::ScopeConflict);
        }
        let mut next = self.next_unknown(incarnation.clone(), previous)?;
        next.policy_fault = Some(PolicyFault::observed(
            next.revision,
            previous,
            policy_sha256,
            incarnation,
        ));
        Ok(next)
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

/// Enclosing UTC DATA at one counter observation. These numbers alone cannot
/// prove SCM continuity, a live source, a cleanup census or a current permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UtcInterval {
    earliest: u64,
    latest: u64,
}
impl UtcInterval {
    pub(crate) fn earliest_millis(self) -> u64 {
        self.earliest
    }
    pub(crate) fn latest_millis(self) -> u64 {
        self.latest
    }
}

/// Advance the entire signed anchor interval using enclosing native elapsed
/// bounds. The Windows owner separately proves that both samples belong to the
/// same real SCM incarnation and that the anchor/policy are still protected.
/// Suspend-inclusive native elapsed is the established project's elapsed-time
/// reference; this adds no drift, anchor-age or absolute-UTC metrology policy.
pub(crate) fn advance_interval(
    anchor_earliest: u64,
    anchor_latest: u64,
    received: CounterObservation,
    current: CounterObservation,
) -> Result<UtcInterval> {
    if anchor_earliest > anchor_latest || anchor_latest > json::SAFE_MAX {
        return Err(Error::Unavailable);
    }
    // The original received bracket must remain the origin. A caller cannot
    // replace it with a fresh sample to erase sleep or already elapsed time.
    let elapsed = current.elapsed_since(received)?.suspend_inclusive();
    let earliest = anchor_earliest
        .checked_add(elapsed.floor_millis()?)
        .filter(|n| *n <= json::SAFE_MAX)
        .ok_or(Error::Unavailable)?;
    let latest = anchor_latest
        .checked_add(elapsed.ceil_millis()?)
        .filter(|n| *n <= json::SAFE_MAX)
        .ok_or(Error::Unavailable)?;
    Ok(UtcInterval { earliest, latest })
}

/// Context/window checks only. Actual Root custody and the accepted continuous
/// signature lineage belong to the native source, not this pure DATA helper.
pub(crate) fn check_root_interval(raw: &[u8], product: &str, interval: UtcInterval) -> Result<()> {
    let envelope = decode_canonical_envelope(raw, InputKind::Metadata)
        .map_err(|_| Error::StateCorrupt)?
        .check_full_schema()
        .map_err(|_| Error::StateCorrupt)?;
    let signed = envelope.envelope().signed();
    let shape = signed
        .root_policy_shape()
        .map_err(|_| Error::StateCorrupt)?;
    if shape.product() != product {
        return Err(Error::StateCorrupt);
    }
    // root_policy_shape validates these as safe, positive integer fields. Keep
    // explicit checks here rather than relying on a lossy numeric cast.
    let time = |name: &str| -> Result<u64> {
        signed.value()[name]
            .as_f64()
            .filter(|n| {
                n.is_finite() && *n >= 0.0 && *n <= json::SAFE_MAX as f64 && n.trunc() == *n
            })
            .map(|n| n as u64)
            .ok_or(Error::StateCorrupt)
    };
    let issued = time("issuedAt")?;
    let expires = time("expiresAt")?;
    if issued > interval.earliest || expires <= interval.latest {
        return Err(Error::SourceUnavailable);
    }
    Ok(())
}

#[cfg(test)]
mod live_interval_tests {
    use super::*;
    fn sample(low: u64, awake_low: u64, awake_high: u64, high: u64) -> CounterObservation {
        CounterObservation::from_bracket(low, awake_low, awake_high, high).unwrap()
    }
    #[test]
    fn advancing_preserves_both_anchor_edges_and_outward_fractional_rounding() {
        let received = sample(10_000, 8_000, 15_000, 20_000);
        let current = sample(30_001, 25_000, 35_000, 50_001);
        let interval = advance_interval(100, 120, received, current).unwrap();
        assert_eq!(interval.earliest_millis(), 101);
        assert_eq!(interval.latest_millis(), 125);
        // A point estimate or using only awake time would lose this enclosure.
        assert!(interval.latest_millis() - interval.earliest_millis() > 20);
    }
    #[test]
    fn suspend_time_is_not_erased_and_exact_and_submillisecond_bounds_differ() {
        let received = sample(10_000, 8_000, 8_000, 10_000);
        let exact = sample(20_000, 8_000, 8_000, 20_000);
        assert_eq!(
            advance_interval(100, 120, received, exact).unwrap(),
            UtcInterval {
                earliest: 101,
                latest: 121
            }
        );
        let fractional = sample(19_999, 8_000, 8_000, 19_999);
        assert_eq!(
            advance_interval(100, 120, received, fractional).unwrap(),
            UtcInterval {
                earliest: 100,
                latest: 121
            }
        );
    }
    #[test]
    fn reverse_and_overflowing_observations_never_become_an_interval() {
        let received = sample(10_000, 8_000, 8_000, 10_000);
        let earlier = sample(9_999, 7_999, 7_999, 9_999);
        assert!(advance_interval(100, 120, received, earlier).is_err());
        assert!(advance_interval(121, 120, received, received).is_err());
        let later = sample(20_000, 9_000, 9_000, 20_000);
        assert!(advance_interval(json::SAFE_MAX, json::SAFE_MAX, received, later).is_err());
        assert!(advance_interval(0, json::SAFE_MAX + 1, received, received).is_err());
    }
    #[test]
    fn accepted_root_must_cover_the_complete_current_interval_at_both_edges() {
        let vectors: serde_json::Value = serde_json::from_slice(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../specs/upgrade/v1/vectors/time/anchor.json"
        )))
        .unwrap();
        let raw = vectors["cases"][1]["trustedRootBytes"]
            .as_str()
            .unwrap()
            .as_bytes();
        let root: serde_json::Value = serde_json::from_slice(raw).unwrap();
        let issued = root["signed"]["issuedAt"].as_u64().unwrap();
        let expires = root["signed"]["expiresAt"].as_u64().unwrap();
        check_root_interval(
            raw,
            "pinvou",
            UtcInterval {
                earliest: issued,
                latest: expires - 1,
            },
        )
        .unwrap();
        assert!(
            check_root_interval(
                raw,
                "pinvou",
                UtcInterval {
                    earliest: issued - 1,
                    latest: issued
                }
            )
            .is_err()
        );
        assert!(
            check_root_interval(
                raw,
                "pinvou",
                UtcInterval {
                    earliest: expires - 1,
                    latest: expires
                }
            )
            .is_err()
        );
        assert!(
            check_root_interval(
                raw,
                "other",
                UtcInterval {
                    earliest: issued,
                    latest: issued
                }
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initial_policy_is_canonical_and_bound_to_the_original_installation() {
        let b = binding();
        let policy = Policy::initial(b.clone()).unwrap();
        assert_eq!(policy.max_round_trip_ms, 30_000);
        assert_eq!(policy.max_uncertainty_ms, 1_000);
        assert!(policy.denied_key_ids.is_empty());
        let raw = json::encode(&policy, false).unwrap();
        Policy::parse(&raw, &b).unwrap();
        let mut other = b.clone();
        other.original_install_operation_id = "another-original".into();
        assert!(Policy::parse(&raw, &other).is_err());
        other = b.clone();
        other.genesis_manifest_sha256 = "c".repeat(64);
        assert!(Policy::parse(&raw, &other).is_err());
        other = b;
        other.installation_scope_id.clear();
        assert!(Policy::initial(other).is_err());
        assert!(Policy::parse(b"{}", &binding()).is_err());
    }

    #[test]
    fn initial_policy_verifies_real_signed_evidence_without_granting_fresh_time() {
        let (mut state, _, original) = anchored();
        let raw = json::encode(&Policy::initial(state.binding.clone()).unwrap(), false).unwrap();
        state.evidence.as_mut().unwrap().policy = String::from_utf8(raw.clone()).unwrap();
        state.validate(&original).unwrap();
        state
            .anchor_for_live(&raw, state.incarnation.as_ref().unwrap())
            .unwrap();
        let initial = State::genesis(state.binding.clone(), &original).unwrap();
        assert!(initial.status == Status::Unknown);
        assert!(initial.cleanup_pending && initial.floor.is_none() && initial.evidence.is_none());
        assert!(
            initial
                .anchor_for_live(&raw, state.incarnation.as_ref().unwrap())
                .is_err()
        );
    }

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
    fn policy_fault_preserves_original_state_and_survives_reopen_and_incarnation_change() {
        let (mut state, _, original) = anchored();
        state.denied_key_ids.push("f".repeat(64));
        state.validate(&original).unwrap();
        let before = json::encode(&state, false).unwrap();
        let fault = state
            .next_policy_fault(incarnation(1), &before, "a".repeat(64))
            .unwrap();
        state.transition(&fault, &before, &original).unwrap();
        assert!(
            fault.status == Status::Unknown
                && fault.cleanup_pending
                && fault.zero_retention.is_none()
        );
        assert!(
            fault.floor == state.floor
                && fault.roots == state.roots
                && fault.evidence == state.evidence
                && fault.denied_key_ids == state.denied_key_ids
        );
        let raw = json::encode(&fault, false).unwrap();
        let reopened = State::parse(&raw, &original).unwrap();
        let next = reopened.next_unknown(incarnation(2), &raw).unwrap();
        reopened.transition(&next, &raw, &original).unwrap();
        assert!(next.policy_fault == fault.policy_fault);
        assert!(
            next.next_policy_fault(
                incarnation(2),
                &json::encode(&next, false).unwrap(),
                "b".repeat(64)
            )
            .is_err()
        );
        // Restored valid policy bytes and an old signed anchor are not recovery.
        assert!(
            fault
                .anchor_for_live(
                    state.evidence.as_ref().unwrap().policy.as_bytes(),
                    &incarnation(1)
                )
                .is_err()
        );
        let mut erased = next;
        erased.policy_fault = None;
        assert!(fault.transition(&erased, &raw, &original).is_err());
    }
    #[test]
    fn policy_fault_closes_only_with_a_new_anchor_and_zero_completion_shape() {
        // Real public signed vector DATA, not an installed SCM or live source.
        let (signed, _, original) = anchored();
        let initial = State::genesis(binding(), &original).unwrap();
        let before = json::encode(&initial, false).unwrap();
        let fault = initial
            .next_policy_fault(incarnation(1), &before, "a".repeat(64))
            .unwrap();
        initial.transition(&fault, &before, &original).unwrap();
        let fault_raw = json::encode(&fault, false).unwrap();
        let mut pending = signed;
        pending.revision = fault.revision + 1;
        pending.previous_sha256 = Some(json::sha256(&fault_raw));
        pending.policy_fault = fault.policy_fault.clone();
        fault.transition(&pending, &fault_raw, &original).unwrap();
        let pending_raw = json::encode(&pending, false).unwrap();
        let mut completed = pending.clone();
        completed.revision += 1;
        completed.previous_sha256 = Some(json::sha256(&pending_raw));
        completed.status = Status::CleanupVerifiedZero;
        completed.cleanup_pending = false;
        completed.zero_retention = Some(crate::retention_custody::ZeroRetentionReference {
            completion_revision: completed.revision,
            completion_sha256: "b".repeat(64),
        });
        // Active incidents cannot be ignored by an otherwise complete shape.
        assert!(
            pending
                .transition(&completed, &pending_raw, &original)
                .is_err()
        );
        completed.policy_fault = pending
            .policy_fault
            .as_ref()
            .map(|f| f.recovered(completed.revision));
        pending
            .transition(&completed, &pending_raw, &original)
            .unwrap();
        assert!(!completed.policy_fault.as_ref().unwrap().is_active());
        let raw = json::encode(&completed, false).unwrap();
        let future = completed.next_unknown(incarnation(2), &raw).unwrap();
        completed.transition(&future, &raw, &original).unwrap();
        assert!(future.policy_fault == completed.policy_fault); // History not erased.
        let future_raw = json::encode(&future, false).unwrap();
        let again = future
            .next_policy_fault(incarnation(2), &future_raw, "c".repeat(64))
            .unwrap();
        future.transition(&again, &future_raw, &original).unwrap();
        assert!(again.policy_fault.as_ref().unwrap().is_active());
        assert!(completed.policy_fault != again.policy_fault);
    }
    #[test]
    fn active_policy_fault_rejects_replayed_anchor_and_mutated_incident() {
        let (state, _, original) = anchored();
        let raw = json::encode(&state, false).unwrap();
        let fault = state
            .next_policy_fault(incarnation(1), &raw, "a".repeat(64))
            .unwrap();
        let fault_raw = json::encode(&fault, false).unwrap();
        let mut replayed = state.clone();
        replayed.revision = fault.revision + 1;
        replayed.previous_sha256 = Some(json::sha256(&fault_raw));
        replayed.policy_fault = fault.policy_fault.clone();
        assert!(fault.transition(&replayed, &fault_raw, &original).is_err());
        for (field, value) in [
            ("detectedRevision", serde_json::json!(0)),
            ("previousTimeSha256", serde_json::json!("b".repeat(64))),
            ("policySha256", serde_json::json!("invalid")),
            ("recoveredRevision", serde_json::json!(fault.revision)),
            ("kind", serde_json::json!("offline")),
        ] {
            let mut bad = serde_json::to_value(&fault).unwrap();
            bad["policyFault"][field] = value;
            assert!(State::parse(&json::encode(&bad, false).unwrap(), &original).is_err());
        }
    }
    #[test]
    fn live_anchor_data_rejects_unknown_policy_deny_and_cross_incarnation() {
        // DATA controls using an actual public signature vector. This helper
        // does not mint a native live source or a successful SCM fixture.
        let (state, _, _) = anchored();
        let policy = state.evidence.as_ref().unwrap().policy.as_bytes();
        let current_incarnation = state.incarnation.as_ref().unwrap();
        state.anchor_for_live(policy, current_incarnation).unwrap();
        let mut unknown = state.clone();
        unknown.status = Status::Unknown;
        assert!(
            unknown
                .anchor_for_live(policy, current_incarnation)
                .is_err()
        );
        assert!(state.anchor_for_live(policy, &incarnation(2)).is_err());
        let mut changed: Policy = json::decode(policy, false).unwrap();
        changed.max_round_trip_ms += 1;
        assert!(
            state
                .anchor_for_live(&json::encode(&changed, false).unwrap(), current_incarnation)
                .is_err()
        );
        let mut denied = state.clone();
        denied.denied_key_ids.push("a".repeat(64));
        assert!(denied.anchor_for_live(policy, current_incarnation).is_err());
        let mut absent = state.clone();
        absent.evidence = None;
        assert!(absent.anchor_for_live(policy, current_incarnation).is_err());
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
    #[test]
    fn zero_completion_changes_no_anchor_and_unknown_cannot_retain_it() {
        // Administrative transition DATA only. The native publisher alone must
        // authenticate the referenced custody/completion and actual read set.
        let (pending, _, original) = anchored();
        let before = json::encode(&pending, false).unwrap();
        assert!(
            !serde_json::from_slice::<serde_json::Value>(&before)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("zeroRetention")
        );
        let mut complete = pending.clone();
        complete.revision += 1;
        complete.previous_sha256 = Some(json::sha256(&before));
        complete.status = Status::CleanupVerifiedZero;
        complete.cleanup_pending = false;
        complete.zero_retention = Some(crate::retention_custody::ZeroRetentionReference {
            completion_revision: complete.revision,
            completion_sha256: "a".repeat(64),
        });
        pending.transition(&complete, &before, &original).unwrap();
        for field in ["floor", "cleanupPending", "zeroRetention"] {
            let mut value = serde_json::to_value(&complete).unwrap();
            value[field] = match field {
                "floor" => serde_json::json!(0),
                "cleanupPending" => serde_json::json!(true),
                _ => serde_json::Value::Null,
            };
            assert!(State::parse(&json::encode(&value, false).unwrap(), &original).is_err());
        }
        let raw = json::encode(&complete, false).unwrap();
        let unknown = complete.next_unknown(incarnation(2), &raw).unwrap();
        complete.transition(&unknown, &raw, &original).unwrap();
        assert!(unknown.cleanup_pending && unknown.zero_retention.is_none());
        assert!(
            unknown.floor == complete.floor
                && unknown.evidence == complete.evidence
                && unknown.roots == complete.roots
        );
        let mut forged = complete.clone();
        forged.denied_key_ids.push("f".repeat(64));
        assert!(pending.transition(&forged, &before, &original).is_err());
    }
}
