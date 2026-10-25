//! Fixed native64 protected time sibling. Missing is never a fresh owner.
//! Its genesis is created exclusively inside original Store::commit(None).
use super::super::installed::StartedServiceContext;
use super::*;
use crate::trusted_time::{Binding, CounterEvidence, Evidence, Incarnation, Policy, State, Status};
use serde::{Deserialize, Serialize};

const NAMESPACE: &str = "trusted-time-v1";
/// Actual read DATA, never a caller-created native source capability.
pub(crate) struct TimeSnapshot {
    pub(crate) raw: Vec<u8>,
    pub(crate) state: State,
    pub(crate) original_root: Vec<u8>,
}
pub(crate) struct PolicySnapshot {
    pub(crate) raw: Vec<u8>,
    pub(crate) policy: Policy,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Blob {
    size: usize,
    sha256: String,
}
impl Blob {
    fn of(bytes: &[u8]) -> Self {
        Self {
            size: bytes.len(),
            sha256: json::sha256(bytes),
        }
    }
    fn read(&self, key: &Key, name: &str, limit: usize) -> Result<Vec<u8>> {
        if self.size == 0 || self.size > limit || !json::digest(&self.sha256) {
            return Err(Error::StateCorrupt);
        }
        let raw = read(key, name)?;
        if raw.len() != self.size || json::sha256(&raw) != self.sha256 {
            return Err(Error::StateCorrupt);
        }
        Ok(raw)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EvidenceHeader {
    request: Blob,
    response: Blob,
    policy: Blob,
    sent: CounterEvidence,
    received: CounterEvidence,
    incarnation: Incarnation,
    floor_before: u64,
    base_root_count: usize,
    denied_key_ids: Vec<String>,
    earliest: u64,
    latest: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Header {
    protocol_version: u8,
    binding: Binding,
    revision: u64,
    previous_sha256: Option<String>,
    incarnation: Option<Incarnation>,
    status: Status,
    cleanup_pending: bool,
    floor: Option<u64>,
    roots: Vec<Blob>,
    denied_key_ids: Vec<String>,
    evidence: Option<EvidenceHeader>,
}
fn header(state: &State) -> Header {
    Header {
        protocol_version: state.protocol_version,
        binding: state.binding.clone(),
        revision: state.revision,
        previous_sha256: state.previous_sha256.clone(),
        incarnation: state.incarnation.clone(),
        status: state.status,
        cleanup_pending: state.cleanup_pending,
        floor: state.floor,
        roots: state.roots.iter().map(|s| Blob::of(s.as_bytes())).collect(),
        denied_key_ids: state.denied_key_ids.clone(),
        evidence: state.evidence.as_ref().map(|e| EvidenceHeader {
            request: Blob::of(e.request.as_bytes()),
            response: Blob::of(e.response.as_bytes()),
            policy: Blob::of(e.policy.as_bytes()),
            sent: e.sent.clone(),
            received: e.received.clone(),
            incarnation: e.incarnation.clone(),
            floor_before: e.floor_before,
            base_root_count: e.base_root_count,
            denied_key_ids: e.denied_key_ids.clone(),
            earliest: e.earliest,
            latest: e.latest,
        }),
    }
}
fn root_capacity(roots: &[Blob]) -> Result<()> {
    if roots.len() > 4096 {
        return Err(Error::Unavailable);
    }
    let mut total = 0usize;
    for r in roots {
        if r.size == 0 || r.size > json::RECORD_MAX {
            return Err(Error::StateCorrupt);
        }
        total = total
            .checked_add(r.size)
            .filter(|n| *n <= json::RECORD_MAX)
            .ok_or(Error::Unavailable)?;
    }
    Ok(())
}
fn read_record(key: &Key, raw: &[u8], original: &[u8]) -> Result<State> {
    let h: Header = json::decode(raw, false)?;
    if json::encode(&h, false)? != raw {
        return Err(Error::StateCorrupt);
    }
    root_capacity(&h.roots)?;
    fn text(bytes: Vec<u8>) -> Result<String> {
        String::from_utf8(bytes).map_err(|_| Error::StateCorrupt)
    }
    let mut roots = Vec::with_capacity(h.roots.len());
    for (i, r) in h.roots.iter().enumerate() {
        roots.push(text(r.read(
            key,
            &format!("Root-{i}"),
            json::RECORD_MAX,
        )?)?);
    }
    let evidence = if let Some(e) = h.evidence {
        Some(Evidence {
            request: text(e.request.read(key, "Request", json::FRAME_MAX)?)?,
            response: text(e.response.read(key, "Response", json::FRAME_MAX)?)?,
            policy: text(e.policy.read(key, "PolicyEvidence", json::RECORD_MAX)?)?,
            sent: e.sent,
            received: e.received,
            incarnation: e.incarnation,
            floor_before: e.floor_before,
            base_root_count: e.base_root_count,
            denied_key_ids: e.denied_key_ids,
            earliest: e.earliest,
            latest: e.latest,
        })
    } else {
        None
    };
    let state = State {
        protocol_version: h.protocol_version,
        binding: h.binding,
        revision: h.revision,
        previous_sha256: h.previous_sha256,
        incarnation: h.incarnation,
        status: h.status,
        cleanup_pending: h.cleanup_pending,
        floor: h.floor,
        roots,
        denied_key_ids: h.denied_key_ids,
        evidence,
    };
    state.validate(original)?;
    Ok(state)
}
fn write_record(key: &Key, state: &State) -> Result<Vec<u8>> {
    let h = header(state);
    root_capacity(&h.roots)?;
    let raw = json::encode(&h, false)?;
    for (i, r) in state.roots.iter().enumerate() {
        write(key, &format!("Root-{i}"), r.as_bytes())?;
    }
    if let Some(e) = &state.evidence {
        if e.request.len() > json::FRAME_MAX || e.response.len() > json::FRAME_MAX {
            return Err(Error::Unavailable);
        }
        write(key, "Request", e.request.as_bytes())?;
        write(key, "Response", e.response.as_bytes())?;
        write(key, "PolicyEvidence", e.policy.as_bytes())?;
    }
    write(key, "Record", &raw)?;
    Ok(raw)
}
fn state_in(
    private: &Key,
    host: &HostRegistration,
    transaction: Option<&Handle>,
) -> Result<TimeSnapshot> {
    if !crate::bootstrap::initialized(&host.known_owner_namespaces) {
        return Err(Error::SetupIncomplete);
    }
    let original_root = read(private, METADATA_BOOTSTRAP_ROOT)?;
    let key = open(private.0, NAMESPACE, false, transaction)?.ok_or(Error::SetupIncomplete)?;
    let binding = Binding::from_host(host);
    let genesis_raw = read(&key, "Genesis")?;
    let genesis: Header = json::decode(&genesis_raw, false)?;
    let expected = State::genesis(binding.clone(), &original_root)?;
    if genesis_raw != json::encode(&header(&expected), false)? {
        return Err(Error::StateCorrupt);
    }
    let raw = read(&key, "Head")?;
    let h: Header = json::decode(&raw, false)?;
    let history = open(
        key.0,
        &format!("history\\{}", revision_name(h.revision)),
        false,
        transaction,
    )?
    .ok_or(Error::StateCorrupt)?;
    if read(&history, "Record")? != raw {
        return Err(Error::StateCorrupt);
    }
    let state = read_record(&history, &raw, &original_root)?;
    if state.binding != binding {
        return Err(Error::StateCorrupt);
    }
    if state.revision == 1 {
        if raw != genesis_raw {
            return Err(Error::StateCorrupt);
        }
    } else {
        let previous = open(
            key.0,
            &format!("history\\{}", revision_name(state.revision - 1)),
            false,
            transaction,
        )?
        .ok_or(Error::StateCorrupt)?;
        let prior_raw = read(&previous, "Record")?;
        let prior = read_record(&previous, &prior_raw, &original_root)?;
        prior.transition(&state, &prior_raw, &original_root)?;
    }
    // This is not an obligation census. Even the genuine genesis is Unknown
    // with a barrier; no absence of backups/secrets/budgets is inferred.
    let _ = genesis;
    Ok(TimeSnapshot {
        raw,
        state,
        original_root,
    })
}
#[allow(unsafe_code)]
fn policy_in(key: &Key, binding: &Binding) -> Result<PolicySnapshot> {
    let name = wide("Policy");
    let mut size = 0;
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut size,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Err(Error::PrerequisiteUnavailable);
    }
    if status != 0 {
        return Err(Error::Unavailable);
    }
    let raw = read(key, "Policy")?;
    let policy = Policy::parse(&raw, binding)?;
    Ok(PolicySnapshot { raw, policy })
}
/// Called only after the actual new original scope was proven absent in this
/// SAME transaction. It cannot adopt an existing/legacy/partial registration.
pub(super) fn initialize_in(
    private: &Key,
    transaction: &Handle,
    host: &HostRegistration,
) -> Result<()> {
    let actual_root = read(private, METADATA_BOOTSTRAP_ROOT)?;
    let state = State::genesis(Binding::from_host(host), &actual_root)?;
    let raw = json::encode(&header(&state), false)?;
    let key = create(private.0, NAMESPACE, transaction, false, true)?;
    let history = create(key.0, "history", transaction, false, true)?;
    let first = create(history.0, &revision_name(1), transaction, false, true)?;
    write(&key, "Genesis", &raw)?;
    write(&key, "Head", &raw)?;
    write_record(&first, &state)?;
    // Policy is intentionally absent: local administrators must explicitly
    // supply the protected, scope/genesis-bound value. No defaults are written.
    require_absent_value(&key, "Policy")?;
    state_in(private, host, Some(transaction))?;
    Ok(())
}
impl Store {
    pub(crate) fn time_load(
        &self,
        seed: &Seed,
        expected: &HostRegistration,
    ) -> Result<TimeSnapshot> {
        let actual = self.read(seed)?.ok_or(Error::SetupIncomplete)?;
        if actual.digest()? != expected.digest()? {
            return Err(Error::ScopeConflict);
        }
        let private = self.key(false, None)?.ok_or(Error::SetupIncomplete)?;
        let before = read(&private, "Head")?;
        if decode_record(&before, seed)?.digest()? != actual.digest()? {
            return Err(Error::ScopeConflict);
        }
        let snapshot = state_in(&private, &actual, None)?;
        if read(&private, "Head")? != before {
            return Err(Error::ScopeConflict);
        }
        Ok(snapshot)
    }
    pub(crate) fn time_policy(
        &self,
        seed: &Seed,
        expected: &TimeSnapshot,
    ) -> Result<PolicySnapshot> {
        let actual = self.read(seed)?.ok_or(Error::SetupIncomplete)?;
        if Binding::from_host(&actual) != expected.state.binding {
            return Err(Error::ScopeConflict);
        }
        let private = self.key(false, None)?.ok_or(Error::SetupIncomplete)?;
        let time = open(private.0, NAMESPACE, false, None)?.ok_or(Error::SetupIncomplete)?;
        if read(&time, "Head")? != expected.raw {
            return Err(Error::ScopeConflict);
        }
        let policy = policy_in(&time, &expected.state.binding)?;
        if read(&time, "Head")? != expected.raw {
            return Err(Error::ScopeConflict);
        }
        Ok(policy)
    }
    /// Private fixed publisher: SCM authority, complete original custody,
    /// expected state and (for an anchor) exact captured Policy are rechecked
    /// INSIDE the same native transaction. No network work occurs under it.
    #[allow(unsafe_code)]
    pub(crate) fn time_commit(
        &self,
        host: &StartedServiceContext,
        expected: &TimeSnapshot,
        next: &State,
        policy: Option<&PolicySnapshot>,
    ) -> Result<TimeSnapshot> {
        let seed = &host.context.seed;
        let before = self.read(seed)?.ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&before)?;
        let _guard = BootstrapGuard::acquire(seed, false)?;
        let transaction = Handle::owned(unsafe {
            CreateTransaction(ptr::null_mut(), ptr::null_mut(), 0, 0, 0, 0, ptr::null())
        })?;
        let actual = self
            .read_in(seed, Some(&transaction))?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&actual)?;
        if before.digest()? != actual.digest()?
            || Binding::from_host(&actual) != expected.state.binding
            || next.incarnation != Some(Incarnation::from_host(&actual)?)
        {
            return Err(Error::ScopeConflict);
        }
        let private = self
            .key(true, Some(&transaction))?
            .ok_or(Error::SetupIncomplete)?;
        let current = state_in(&private, &actual, Some(&transaction))?;
        if current.raw != expected.raw || current.original_root != expected.original_root {
            return Err(Error::ScopeConflict);
        }
        current
            .state
            .transition(next, &current.raw, &current.original_root)?;
        let key =
            open(private.0, NAMESPACE, true, Some(&transaction))?.ok_or(Error::SetupIncomplete)?;
        match (next.status, policy) {
            (Status::Unknown, None) => {
                if next.denied_key_ids != current.state.denied_key_ids {
                    return Err(Error::StateCorrupt);
                }
            }
            (status, Some(captured)) => {
                let actual_policy = policy_in(&key, &next.binding)?;
                if actual_policy.raw != captured.raw {
                    return Err(Error::ScopeConflict);
                }
                let mut known = current.state.denied_key_ids.clone();
                known.extend(actual_policy.policy.denied_key_ids.iter().cloned());
                known.sort();
                known.dedup();
                if next.denied_key_ids != known {
                    return Err(Error::StateCorrupt);
                }
                if status == Status::CleanupPending
                    && next
                        .evidence
                        .as_ref()
                        .ok_or(Error::StateCorrupt)?
                        .policy
                        .as_bytes()
                        != captured.raw
                {
                    return Err(Error::StateCorrupt);
                }
            }
            _ => return Err(Error::StateCorrupt),
        }
        let raw = json::encode(&header(next), false)?;
        let history =
            open(key.0, "history", true, Some(&transaction))?.ok_or(Error::StateCorrupt)?;
        let revision = create(
            history.0,
            &revision_name(next.revision),
            &transaction,
            false,
            true,
        )?;
        if write_record(&revision, next)? != raw {
            return Err(Error::StateCorrupt);
        }
        write(&key, "Head", &raw)?;
        if state_in(&private, &actual, Some(&transaction))?.raw != raw {
            return Err(Error::StateCorrupt);
        }
        drop(revision);
        drop(history);
        drop(key);
        drop(private);
        if unsafe { CommitTransaction(transaction.raw()) } == 0 {
            return Err(Error::StateCommitUnknown);
        }
        drop(transaction);
        let verify = || -> Result<TimeSnapshot> {
            let private = self.key(false, None)?.ok_or(Error::StateCommitUnknown)?;
            let key = open(private.0, NAMESPACE, false, None)?.ok_or(Error::StateCommitUnknown)?;
            let history = open(
                key.0,
                &format!("history\\{}", revision_name(next.revision)),
                false,
                None,
            )?
            .ok_or(Error::StateCommitUnknown)?;
            if unsafe { RegFlushKey(key.0) } != 0 || unsafe { RegFlushKey(history.0) } != 0 {
                return Err(Error::StateCommitUnknown);
            }
            let after = self.read(seed)?.ok_or(Error::StateCommitUnknown)?;
            host.authenticate_running(&after)?;
            let observed = state_in(&private, &after, None)?;
            if observed.raw != raw {
                return Err(Error::StateCommitUnknown);
            }
            Ok(observed)
        };
        // No later ACL/flush/readback/SCM error is a no-write error.
        verify().map_err(|_| Error::StateCommitUnknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detached_root_descriptors_do_not_json_expand_a_maximum_metadata_original() {
        // Byte-capacity/descriptor DATA control only, not a valid Root or SCM source.
        let raw = vec![b'"'; json::RECORD_MAX];
        let blob = Blob::of(&raw);
        root_capacity(&[blob.clone()]).unwrap();
        let encoded = json::encode(&blob, false).unwrap();
        assert!(encoded.len() < 128);
        assert_eq!(blob.size, json::RECORD_MAX);
        assert_eq!(blob.sha256, json::sha256(&raw));
        assert!(root_capacity(&[blob, Blob::of(b"x")]).is_err());
        assert!(
            root_capacity(&[Blob {
                size: 0,
                sha256: "a".repeat(64)
            }])
            .is_err()
        );
    }
    #[test]
    fn private_head_is_closed_and_requires_original_evidence_identities() {
        let vectors: serde_json::Value = serde_json::from_slice(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../specs/upgrade/v1/vectors/time/anchor.json"
        )))
        .unwrap();
        let original = vectors["cases"][0]["trustedRootBytes"]
            .as_str()
            .unwrap()
            .as_bytes();
        let binding = Binding {
            installation_scope_id: "scope".into(),
            product: "pinvou".into(),
            component: "app".into(),
            target_key: "windows-x86_64-nsis-perMachine".into(),
            profile_kind: "production".into(),
            profile_id: "host".into(),
            genesis_seed_sha256: "a".repeat(64),
            genesis_manifest_sha256: "b".repeat(64),
            original_install_operation_id: "original".into(),
        };
        let state = State::genesis(binding, original).unwrap();
        let raw = json::encode(&header(&state), false).unwrap();
        let h: Header = json::decode(&raw, false).unwrap();
        assert!(h.cleanup_pending && h.floor.is_none());
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        value["cleanupComplete"] = serde_json::Value::Bool(true);
        assert!(json::decode::<Header>(&json::encode(&value, false).unwrap(), false).is_err());
        value.as_object_mut().unwrap().remove("cleanupComplete");
        value.as_object_mut().unwrap().remove("roots");
        assert!(json::decode::<Header>(&json::encode(&value, false).unwrap(), false).is_err());
    }
}
