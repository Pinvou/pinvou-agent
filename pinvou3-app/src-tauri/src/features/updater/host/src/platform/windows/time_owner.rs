//! Actual SCM-bound time acquisition. This batch commits evidence and a cleanup
//! barrier only; it never advertises a durable Known clock or recovery permission.
use super::{
    installed::StartedServiceContext,
    original_root::ProtectedOriginalRoot,
    registry::{
        Store,
        time::{PolicySnapshot, TimeSnapshot},
    },
    time_transport::TimeTransport,
};
use crate::{
    error::{Error, Result},
    json,
    trusted_time::{CounterEvidence, Evidence, Incarnation, Status, root_version},
};
use pinvou_upgrade_codec::{Document, InputKind};
use pinvou_upgrade_control_http::RootGetResult;
use pinvou_upgrade_verifier::{
    SuppliedTimeObservation, TimeVerificationInputs, verify_time_response,
};

pub(super) struct TimeOwner {
    snapshot: TimeSnapshot,
    original: ProtectedOriginalRoot,
    incarnation: Incarnation,
    commit_uncertain: bool,
}
impl TimeOwner {
    /// Recovery, never initialization. A scope predating this owner remains
    /// SetupIncomplete. Even genuine new genesis proves no empty obligations.
    pub(super) fn open(host: &StartedServiceContext) -> Result<Self> {
        let original = host.original_root_source()?;
        let own = &host.context;
        let store = Store::new(&own.seed);
        let before = store.read(&own.seed)?.ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&before)?;
        let mut snapshot = store.time_load(&own.seed, &before)?;
        if snapshot.original_root.as_slice() != original.root_bytes(host)? {
            return Err(Error::StateCorrupt);
        }
        let incarnation = Incarnation::from_host(&before)?;
        if snapshot.state.incarnation.as_ref() != Some(&incarnation) {
            // Invalidate continuity durably BEFORE any network/policy work.
            // Original floor, Root lineage, evidence and barrier all survive.
            let unknown = snapshot
                .state
                .next_unknown(incarnation.clone(), &snapshot.raw)?;
            snapshot = store.time_commit(host, &snapshot, &unknown, None)?;
        }
        let after = store.read(&own.seed)?.ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&after)?;
        if Incarnation::from_host(&after)? != incarnation {
            return Err(Error::ServiceIdentityRefused);
        }
        Ok(Self {
            snapshot,
            original,
            incarnation,
            commit_uncertain: false,
        })
    }
    /// One explicitly controlled attempt. The startup worker calls this once
    /// then waits for stop; there is no hidden reconnect timer or expiry policy.
    /// Cancellation/errors leave Unknown/barrier, never an inferred cleanup.
    pub(super) async fn refresh(
        &mut self,
        host: &StartedServiceContext,
        transport: &TimeTransport,
    ) -> Result<()> {
        if self.commit_uncertain {
            return Err(Error::StateCommitUnknown);
        }
        self.original.recheck(host)?;
        let seed = &host.context.seed;
        let store = Store::new(seed);
        let current = store.read(seed)?.ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&current)?;
        if Incarnation::from_host(&current)? != self.incarnation {
            return Err(Error::ServiceIdentityRefused);
        }
        let actual = store.time_load(seed, &current)?;
        if actual.raw != self.snapshot.raw || actual.original_root != self.snapshot.original_root {
            return Err(Error::ScopeConflict);
        }
        if self.snapshot.state.status != Status::Unknown {
            let next = self
                .snapshot
                .state
                .next_unknown(self.incarnation.clone(), &self.snapshot.raw)?;
            self.commit_uncertain = true;
            self.snapshot = store.time_commit(host, &self.snapshot, &next, None)?;
            self.commit_uncertain = false;
        }
        // Policy missing is a safe fixed error, not a default profile. Capture
        // bytes are finally re-read by time_commit in its actual native TX.
        let policy = store.time_policy(seed, &self.snapshot)?;
        let mut denied = self.snapshot.state.denied_key_ids.clone();
        denied.extend(policy.policy.denied_key_ids.iter().cloned());
        denied.sort();
        denied.dedup();
        if denied.len() > 4096 {
            return Err(Error::Unavailable);
        }
        if denied != self.snapshot.state.denied_key_ids {
            // Persist newly observed protected deny BEFORE any network await.
            // A later policy edit cannot erase this accumulated high-water.
            let mut unknown = self
                .snapshot
                .state
                .next_unknown(self.incarnation.clone(), &self.snapshot.raw)?;
            unknown.denied_key_ids = denied.clone();
            self.commit_uncertain = true;
            self.snapshot = store.time_commit(host, &self.snapshot, &unknown, Some(&policy))?;
            self.commit_uncertain = false;
        }
        let floor = self
            .snapshot
            .state
            .floor
            .unwrap_or(0)
            .max(store.budget_time_floor(seed, &current)?.unwrap_or(0));
        let base_root_count = self.snapshot.state.roots.len();
        let trusted_root = self
            .snapshot
            .state
            .current_root(&self.snapshot.original_root)
            .to_vec();
        let mut roots = self.snapshot.state.roots.clone();
        root_capacity(&roots)?;
        loop {
            let selected = roots
                .last()
                .map_or(self.snapshot.original_root.as_slice(), |s| s.as_bytes());
            let version = root_version(selected, &self.snapshot.state.binding.product)?;
            let nonce = transport.fresh_nonce()?;
            let request = json::encode(
                &serde_json::json!({"protocolVersion":1,
                "product":self.snapshot.state.binding.product,"component":self.snapshot.state.binding.component,
                "requestNonce":nonce,"root":{"version":version,"sha256":json::sha256(selected)}}),
                false,
            )?;
            let request_shape = Document::parse(&request, InputKind::Credential)
                .map_err(|_| Error::StateCorrupt)?
                .time_request_shape()
                .map_err(|_| Error::StateCorrupt)?;
            let sent = host.sample_monotonic()?;
            let response = transport.post_time(&request).await;
            if let Ok(response) = response {
                let received = host.sample_monotonic()?;
                received.elapsed_since(&sent)?;
                let refs: Vec<&[u8]> = roots[base_root_count..]
                    .iter()
                    .map(|s| s.as_bytes())
                    .collect();
                let observation = SuppliedTimeObservation {
                    sent_monotonic_millis: sent.counters().suspend_inclusive().floor_millis()?,
                    received_monotonic_millis: received
                        .counters()
                        .suspend_inclusive()
                        .ceil_millis()?,
                    max_round_trip_millis: policy.policy.max_round_trip_ms,
                    max_uncertainty_millis: policy.policy.max_uncertainty_ms,
                    trusted_time_floor_millis: floor,
                };
                if verify_time_response(
                    &response,
                    TimeVerificationInputs::new(
                        &trusted_root,
                        &refs,
                        &request_shape,
                        observation,
                        &denied,
                    ),
                )
                .is_ok()
                {
                    // Include the first actual signature verification in the
                    // receive/verification bracket, retaining original send.
                    let verified_at = host.sample_monotonic()?;
                    verified_at.elapsed_since(&sent)?;
                    let checked = verify_time_response(
                        &response,
                        TimeVerificationInputs::new(
                            &trusted_root,
                            &refs,
                            &request_shape,
                            SuppliedTimeObservation {
                                received_monotonic_millis: verified_at
                                    .counters()
                                    .suspend_inclusive()
                                    .ceil_millis()?,
                                ..observation
                            },
                            &denied,
                        ),
                    )
                    .map_err(|_| Error::SourceUnavailable)?;
                    let evidence = Evidence {
                        request: String::from_utf8(request).map_err(|_| Error::StateCorrupt)?,
                        response: String::from_utf8(response)
                            .map_err(|_| Error::SourceUnavailable)?,
                        sent: CounterEvidence::from_data(sent.counters()),
                        received: CounterEvidence::from_data(verified_at.counters()),
                        incarnation: self.incarnation.clone(),
                        policy: String::from_utf8(policy.raw.clone())
                            .map_err(|_| Error::StateCorrupt)?,
                        floor_before: floor,
                        base_root_count,
                        denied_key_ids: denied.clone(),
                        earliest: checked.earliest_millis(),
                        latest: checked.latest_millis(),
                    };
                    return self.publish(host, &store, policy, roots, denied, evidence);
                }
            }
            // A missing successor is only a failed acquisition attempt, never
            // proof that selected is latest or sufficient to advance the floor.
            let next_version = version
                .checked_add(1)
                .filter(|n| *n <= json::SAFE_MAX)
                .ok_or(Error::Unavailable)?;
            if roots.len() >= 4096 {
                return Err(Error::Unavailable);
            }
            match transport
                .get_root(&self.snapshot.state.binding.product, next_version)
                .await?
            {
                RootGetResult::Missing => return Err(Error::SourceUnavailable),
                RootGetResult::Found(root) => {
                    if root.version() != next_version
                        || root.product() != self.snapshot.state.binding.product
                    {
                        return Err(Error::SourceUnavailable);
                    }
                    // Check cumulative retained bytes BEFORE copying another
                    // original. The HTTP adapter already bounds a single Root.
                    let total = root_capacity(&roots)?
                        .checked_add(root.bytes().len())
                        .filter(|n| *n <= json::RECORD_MAX)
                        .ok_or(Error::Unavailable)?;
                    let _ = total;
                    roots.push(
                        String::from_utf8(root.bytes().to_vec())
                            .map_err(|_| Error::SourceUnavailable)?,
                    );
                }
            }
        }
    }
    fn publish(
        &mut self,
        host: &StartedServiceContext,
        store: &Store,
        policy: PolicySnapshot,
        roots: Vec<String>,
        denied: Vec<String>,
        evidence: Evidence,
    ) -> Result<()> {
        self.original.recheck(host)?;
        let mut next = self
            .snapshot
            .state
            .next_unknown(self.incarnation.clone(), &self.snapshot.raw)?;
        next.status = Status::CleanupPending;
        next.floor = Some(evidence.earliest);
        next.roots = roots;
        next.denied_key_ids = denied;
        next.evidence = Some(evidence);
        next.validate(&self.snapshot.original_root)?;
        // Refuse every further refresh after an ambiguous/erroring publisher.
        // Reopening authenticates actual committed state; it never resets it.
        self.commit_uncertain = true;
        self.snapshot = store.time_commit(host, &self.snapshot, &next, Some(&policy))?;
        self.commit_uncertain = false;
        // No Known/cleanup-complete state or permit is returned. The evidence's
        // interval is historical at its recorded counter, not commit-time UTC.
        // A future consumer needs its own real counter/continuity/drift and a
        // complete backup/secret/original-budget census before any release.
        Ok(())
    }
}
fn root_capacity(roots: &[String]) -> Result<usize> {
    if roots.len() > 4096 {
        return Err(Error::Unavailable);
    }
    roots.iter().try_fold(0usize, |total, root| {
        total
            .checked_add(root.len())
            .filter(|n| *n <= json::RECORD_MAX)
            .ok_or(Error::Unavailable)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn successor_capacity_is_checked_before_retaining_more_originals() {
        let full = vec!["x".repeat(json::RECORD_MAX)];
        assert_eq!(root_capacity(&full).unwrap(), json::RECORD_MAX);
        let mut excessive = full;
        excessive.push("x".into());
        assert!(root_capacity(&excessive).is_err());
        assert!(root_capacity(&vec![String::new(); 4097]).is_err());
    }
}
