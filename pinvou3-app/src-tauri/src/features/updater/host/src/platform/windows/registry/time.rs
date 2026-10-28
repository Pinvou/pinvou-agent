//! Fixed native64 protected time sibling. Missing is never a fresh owner.
//! Its genesis is created exclusively inside original Store::commit(None).
mod policy_fault;
use super::super::installed::StartedServiceContext;
use super::*;
use crate::trusted_time::{
    Binding, CounterEvidence, Evidence, Incarnation, Policy, PolicyFault, State, Status,
};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    policy_fault: Option<PolicyFault>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    zero_retention: Option<crate::retention_custody::ZeroRetentionReference>,
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
        policy_fault: state.policy_fault.clone(),
        zero_retention: state.zero_retention.clone(),
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
        policy_fault: h.policy_fault,
        zero_retention: h.zero_retention,
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
    if state.status == Status::CleanupVerifiedZero {
        retention::validate_completed_in(private, host, &state, transaction)?;
    } else if open(private.0, retention::NAMESPACE, false, transaction)?.is_some() {
        // Existing fresh custody must keep every historical completion linked,
        // even after the current state becomes Unknown or a new anchor arrives.
        // Missing legacy custody is deliberately neither initialized nor healed.
        retention::read_in(private, host, transaction)?;
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

fn initial_policy_bytes(binding: &Binding) -> Result<Vec<u8>> {
    json::encode(&Policy::initial(binding.clone())?, false)
}

fn verify_initial_policy_in(key: &Key, binding: &Binding) -> Result<()> {
    if policy_in(key, binding)?.raw != initial_policy_bytes(binding)? {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}

fn initialize_policy_in(key: &Key, binding: &Binding) -> Result<()> {
    require_absent_value(key, "Policy")?;
    write(key, "Policy", &initial_policy_bytes(binding)?)?;
    verify_initial_policy_in(key, binding)
}

/// Definitive original-commit readback only; this never creates or replaces
/// policy. Existing loads and refreshes keep their mandatory actual-policy read.
pub(super) fn verify_fresh_policy_in(private: &Key, host: &HostRegistration) -> Result<()> {
    let key = open(private.0, NAMESPACE, false, None)?.ok_or(Error::StateCorrupt)?;
    verify_initial_policy_in(&key, &Binding::from_host(host))
}
// Bounded native adapter census capacity, not a retention or download policy.
// Overflow refuses the census; it never truncates or removes history.
const MAX_CENSUS_TIME_HEADERS: u32 = 16_384;

/// Streaming ledger structure DATA. Native callers supply all actual retained
/// rows; this does not validate signatures or construct a clock/cleanup permit.
struct RetainedCompletionCensus {
    binding: Binding,
    genesis_raw: Vec<u8>,
    head_revision: u64,
    head_sha256: String,
    next_revision: u64,
    previous_sha256: Option<String>,
    references: Vec<crate::retention_custody::ZeroRetentionReference>,
}
impl RetainedCompletionCensus {
    fn new(head_raw: &[u8], genesis_raw: &[u8], binding: &Binding) -> Result<Self> {
        let head: Header = json::decode(head_raw, false)?;
        if json::encode(&head, false)? != head_raw || head.binding != *binding || head.revision == 0
        {
            return Err(Error::StateCorrupt);
        }
        if head.revision > u64::from(MAX_CENSUS_TIME_HEADERS) {
            return Err(Error::Unavailable);
        }
        Ok(Self {
            binding: binding.clone(),
            genesis_raw: genesis_raw.to_vec(),
            head_revision: head.revision,
            head_sha256: json::sha256(head_raw),
            next_revision: 1,
            previous_sha256: None,
            references: Vec::new(),
        })
    }
    fn observe(&mut self, name: &str, raw: &[u8]) -> Result<()> {
        let row: Header = json::decode(raw, false)?;
        if let Some(fault) = &row.policy_fault {
            fault.validate(
                row.revision,
                row.previous_sha256.as_deref(),
                row.status,
                row.incarnation.as_ref(),
            )?;
        }
        if name != revision_name(self.next_revision)
            || row.revision != self.next_revision
            || row.revision > self.head_revision
            || row.binding != self.binding
            || row.previous_sha256 != self.previous_sha256
            || json::encode(&row, false)? != raw
            || self.next_revision == 1 && raw != self.genesis_raw
        {
            return Err(Error::StateCorrupt);
        }
        match (row.status, row.cleanup_pending, row.zero_retention) {
            (Status::CleanupVerifiedZero, false, Some(reference)) => {
                reference.validate()?;
                if reference.completion_revision != row.revision {
                    return Err(Error::StateCorrupt);
                }
                if self.references.len() >= retention::MAX_COMPLETIONS as usize {
                    return Err(Error::Unavailable);
                }
                self.references.push(reference);
            }
            (Status::Unknown | Status::CleanupPending, true, None) => {}
            _ => return Err(Error::StateCorrupt),
        }
        self.previous_sha256 = Some(json::sha256(raw));
        self.next_revision = self
            .next_revision
            .checked_add(1)
            .ok_or(Error::Unavailable)?;
        Ok(())
    }
    fn finish(self) -> Result<Vec<crate::retention_custody::ZeroRetentionReference>> {
        if self.next_revision != self.head_revision + 1
            || self.previous_sha256.as_ref() != Some(&self.head_sha256)
        {
            return Err(Error::StateCorrupt);
        }
        Ok(self.references)
    }
}
/// Read the whole fixed history through actual protected native handles. Only
/// one bounded record body is resident at a time. No recursion into state_in.
pub(super) fn retained_completion_references_in(
    private: &Key,
    host: &HostRegistration,
    tx: Option<&Handle>,
) -> Result<Vec<crate::retention_custody::ZeroRetentionReference>> {
    let key = open(private.0, NAMESPACE, false, tx)?.ok_or(Error::StateCorrupt)?;
    let original_root = read(private, METADATA_BOOTSTRAP_ROOT)?;
    let binding = Binding::from_host(host);
    let expected_genesis = json::encode(
        &header(&State::genesis(binding.clone(), &original_root)?),
        false,
    )?;
    if read(&key, "Genesis")? != expected_genesis {
        return Err(Error::StateCorrupt);
    }
    let mut census =
        RetainedCompletionCensus::new(&read(&key, "Head")?, &expected_genesis, &binding)?;
    let history = open(key.0, "history", false, tx)?.ok_or(Error::StateCorrupt)?;
    if !names(&history, true, 0)?.is_empty() {
        return Err(Error::StateCorrupt);
    }
    let mut revisions = names(&history, false, MAX_CENSUS_TIME_HEADERS)?;
    revisions.sort_unstable();
    for name in revisions {
        let row = open(history.0, &name, false, tx)?.ok_or(Error::StateCorrupt)?;
        census.observe(&name, &read(&row, "Record")?)?;
    }
    census.finish()
}
/// Verify the actual two-way retained link without recursively reading current
/// Time state. Historical completions are not live UTC or a current clean gate.
pub(super) fn check_retained_completion_in(
    private: &Key,
    c: &crate::retention_custody::Completion,
    body: &[u8],
    tx: Option<&Handle>,
) -> Result<()> {
    let key = open(private.0, NAMESPACE, false, tx)?.ok_or(Error::StateCorrupt)?;
    let base = open(
        key.0,
        &format!("history\\{}", revision_name(c.anchor_time_revision)),
        false,
        tx,
    )?
    .ok_or(Error::StateCorrupt)?;
    let base_raw = read(&base, "Record")?;
    let anchor: Header = json::decode(&base_raw, false)?;
    if json::sha256(&base_raw) != c.anchor_time_sha256
        || json::encode(&anchor, false)? != base_raw
        || anchor.binding != c.binding
        || anchor.revision != c.anchor_time_revision
        || anchor.status != Status::CleanupPending
        || !anchor.cleanup_pending
        || anchor.zero_retention.is_some()
        || anchor.incarnation.as_ref() != Some(&c.incarnation)
    {
        return Err(Error::StateCorrupt);
    }
    crate::retention_custody::check_first_scan(
        &anchor
            .evidence
            .as_ref()
            .ok_or(Error::StateCorrupt)?
            .received,
        &c.final_counter,
    )
    .map_err(|_| Error::StateCorrupt)?;
    let completed = open(
        key.0,
        &format!("history\\{}", revision_name(c.completed_time_revision)),
        false,
        tx,
    )?
    .ok_or(Error::StateCorrupt)?;
    let completed_raw = read(&completed, "Record")?;
    let mut next: Header = json::decode(&completed_raw, false)?;
    if json::encode(&next, false)? != completed_raw
        || next.status != Status::CleanupVerifiedZero
        || next.cleanup_pending
        || next.zero_retention.as_ref() != Some(&c.reference()?)
        || next.previous_sha256.as_ref() != Some(&c.anchor_time_sha256)
        || next.revision != c.completed_time_revision
        || json::sha256(body) != c.reference()?.completion_sha256
    {
        return Err(Error::StateCorrupt);
    }
    // Completion changes no original anchor evidence, roots, floor, deny or
    // incarnation. Reverse the finite administrative transition and compare.
    PolicyFault::transition(
        anchor.policy_fault.as_ref(),
        next.policy_fault.as_ref(),
        Status::CleanupVerifiedZero,
        next.revision,
        &base_raw,
    )?;
    next.revision = anchor.revision;
    next.previous_sha256 = anchor.previous_sha256.clone();
    next.status = Status::CleanupPending;
    next.cleanup_pending = true;
    next.zero_retention = None;
    next.policy_fault = anchor.policy_fault.clone();
    if json::encode(&next, false)? != base_raw {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}
// Metadata damage requires repair; it is not evidence of a damaged time
// policy and must not trigger the policy-incident publisher.
fn metadata_root_transition_error(error: Error) -> Error {
    match error {
        Error::StateCorrupt => Error::RepairRequired,
        other => other,
    }
}

/// `event` and `member` are authenticated by the same product visitor.
/// A missing or invalid policy/state refuses rather than contributing empty data.
fn check_member_denies_in(
    seed: &Seed,
    event: &crate::product_catalog::Event,
    member: &HostRegistration,
    next: &State,
    original: &[u8],
    tx: &Handle,
) -> Result<()> {
    let path = format!(
        "Software\\Pinvou\\{}\\{}\\private",
        seed.updater_leaf(),
        event.profile_id
    );
    let private = open(HKEY_LOCAL_MACHINE, &path, false, Some(tx))?.ok_or(Error::StateCorrupt)?;
    let member_state = state_in(&private, member, Some(tx))?;
    if member_state
        .state
        .policy_fault
        .as_ref()
        .is_some_and(PolicyFault::is_active)
    {
        return Err(Error::PrerequisiteUnavailable);
    }
    let time = open(private.0, NAMESPACE, false, Some(tx))?.ok_or(Error::SetupIncomplete)?;
    let policy = policy_in(&time, &member_state.state.binding)?;
    let evidence = next.evidence.as_ref().ok_or(Error::StateCorrupt)?;
    for denied in [
        &member_state.state.denied_key_ids,
        &policy.policy.denied_key_ids,
    ] {
        evidence.check_additional_denied(&next.binding, original, &next.roots, denied)?;
    }
    Ok(())
}

fn stage_state_in(
    private: &Key,
    actual: &HostRegistration,
    current: &TimeSnapshot,
    next: &State,
    tx: &Handle,
) -> Result<Vec<u8>> {
    // This helper is reserved for cleanup-only transitions; it does not own
    // the metadata census required to accept a changed Root.
    if next.roots != current.state.roots {
        return Err(Error::ScopeConflict);
    }
    current
        .state
        .transition(next, &current.raw, &current.original_root)?;
    let key = open(private.0, NAMESPACE, true, Some(tx))?.ok_or(Error::SetupIncomplete)?;
    let history = open(key.0, "history", true, Some(tx))?.ok_or(Error::StateCorrupt)?;
    let revision = create(history.0, &revision_name(next.revision), tx, false, true)?;
    let raw = write_record(&revision, next)?;
    write(&key, "Head", &raw)?;
    if state_in(private, actual, Some(tx))?.raw != raw {
        return Err(Error::StateCorrupt);
    }
    Ok(raw)
}
/// A native transaction-bound read set, minted only below from fixed protected
/// keys. Detached State/Policy values cannot construct this capability.
pub(in crate::platform::windows) struct LiveTimeReadSet<'tx> {
    private: &'tx Key,
    transaction: &'tx Handle,
    host_digest: String,
    snapshot: TimeSnapshot,
    policy: PolicySnapshot,
}
impl LiveTimeReadSet<'_> {
    pub(in crate::platform::windows) fn state(&self) -> &State {
        &self.snapshot.state
    }
    pub(in crate::platform::windows) fn policy(&self) -> &PolicySnapshot {
        &self.policy
    }
    pub(in crate::platform::windows) fn original_root(&self) -> &[u8] {
        &self.snapshot.original_root
    }
    /// Detached identity DATA for a budget's historical observation record.
    pub(in crate::platform::windows) fn head_sha256(&self) -> String {
        json::sha256(&self.snapshot.raw)
    }
    pub(in crate::platform::windows) fn protected_floor_millis(&self) -> Result<u64> {
        let key = open(
            self.private.0,
            budgets::NAMESPACE,
            false,
            Some(self.transaction),
        )?
        .ok_or(Error::SetupIncomplete)?;
        let floor =
            budgets::floor_for_binding(&key, &self.snapshot.state.binding, Some(self.transaction))?;
        Ok(self
            .snapshot
            .state
            .floor
            .unwrap_or(0)
            .max(floor.unwrap_or(0)))
    }
    /// Recheck the exact read set in its original native transaction. The
    /// caller retains the original BootstrapGuard; this does not acquire a
    /// second guard or open a detached transaction around a captured DTO.
    pub(in crate::platform::windows) fn recheck(&self, host: &StartedServiceContext) -> Result<()> {
        let seed = &host.context.seed;
        let store = Store::new(seed);
        let actual = store
            .read_in(seed, Some(self.transaction))?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&actual)?;
        if actual.digest()? != self.host_digest {
            return Err(Error::ScopeConflict);
        }
        let observed = state_in(self.private, &actual, Some(self.transaction))?;
        if observed.raw != self.snapshot.raw
            || observed.original_root != self.snapshot.original_root
        {
            return Err(Error::ScopeConflict);
        }
        let time = open(self.private.0, NAMESPACE, false, Some(self.transaction))?
            .ok_or(Error::SetupIncomplete)?;
        if policy_in(&time, &observed.state.binding)?.raw != self.policy.raw {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
}
/// Registry descendants alone can mint this source. This function does not
/// initialize missing state, repair policy or remove the recovery barrier.
pub(super) fn current_in<'tx>(
    private: &'tx Key,
    actual: &HostRegistration,
    transaction: &'tx Handle,
) -> Result<LiveTimeReadSet<'tx>> {
    let snapshot = state_in(private, actual, Some(transaction))?;
    let time =
        open(private.0, NAMESPACE, false, Some(transaction))?.ok_or(Error::SetupIncomplete)?;
    let policy = policy_in(&time, &snapshot.state.binding)?;
    Ok(LiveTimeReadSet {
        private,
        transaction,
        host_digest: actual.digest()?,
        snapshot,
        policy,
    })
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
    // Only this original-absence transaction provisions the approved limits.
    // Existing/legacy loads never enroll or heal a missing Policy value.
    initialize_policy_in(&key, &state.binding)?;
    state_in(private, host, Some(transaction))?;
    Ok(())
}
impl Store {
    /// Complete ONLY fresh authenticated zero-obligation custody. There is no
    /// caller-provided source, numeric time, permission bool or owner callback.
    /// Existing legacy/missing custody remains paused and is never initialized.
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn complete_zero_retention(
        &self,
        host: &StartedServiceContext,
    ) -> Result<()> {
        let seed = &host.context.seed;
        let _guard = BootstrapGuard::acquire(seed, false)?;
        let tx = Handle::owned(unsafe {
            CreateTransaction(ptr::null_mut(), ptr::null_mut(), 0, 0, 0, 0, ptr::null())
        })?;
        let actual = self
            .read_in(seed, Some(&tx))?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&actual)?;
        let private = self.key(true, Some(&tx))?.ok_or(Error::SetupIncomplete)?;
        let inventory = retention::read_in(&private, &actual, Some(&tx))?;
        let current = state_in(&private, &actual, Some(&tx))?;
        if current.state.status != Status::CleanupPending {
            return Err(Error::ScopeConflict);
        }
        let source = current_in(&private, &actual, &tx)?;
        let initial = super::super::live_time::sample_live_time(host, &source)?;
        // The full real census is repeated after initial sampling. There is no
        // nonzero cleanup fallback. Any registered producer forbids this profile.
        if retention::read_in(&private, &actual, Some(&tx))? != inventory {
            return Err(Error::ScopeConflict);
        }
        let final_result = initial.recheck(host);
        let (raw, next_revision, late_failure, saved_floor) = match final_result {
            Ok(final_sample) => {
                source.recheck(host)?;
                if retention::read_in(&private, &actual, Some(&tx))? != inventory {
                    return Err(Error::ScopeConflict);
                }
                budgets::retain_samples_floor_in(
                    &private,
                    &actual,
                    &tx,
                    &source,
                    &initial,
                    Some(&final_sample),
                )?;
                let within_scan = crate::retention_custody::check_first_scan(
                    &current
                        .state
                        .evidence
                        .as_ref()
                        .ok_or(Error::StateCorrupt)?
                        .received,
                    &final_sample.counter_evidence(),
                );
                if let Err(error) = within_scan {
                    // Late invocation/lock wait is not evidence that the first
                    // scan met its ceiling. Preserve BOTH successful samples,
                    // keep Unknown, and return the fixed failure after commit.
                    let next = current
                        .state
                        .next_unknown(Incarnation::from_host(&actual)?, &current.raw)?;
                    let raw = stage_state_in(&private, &actual, &current, &next, &tx)?;
                    (
                        raw,
                        next.revision,
                        Some(error),
                        final_sample.bounds().earliest_millis(),
                    )
                } else {
                    let next_revision = current
                        .state
                        .revision
                        .checked_add(1)
                        .filter(|n| *n <= json::SAFE_MAX)
                        .ok_or(Error::Unavailable)?;
                    let completion = crate::retention_custody::Completion {
                        protocol_version: 1,
                        binding: current.state.binding.clone(),
                        inventory_revision: 1,
                        inventory_sha256: json::sha256(&inventory),
                        anchor_time_revision: current.state.revision,
                        anchor_time_sha256: json::sha256(&current.raw),
                        completed_time_revision: next_revision,
                        policy_sha256: json::sha256(&source.policy().raw),
                        incarnation: Incarnation::from_host(&actual)?,
                        initial_counter: initial.counter_evidence(),
                        final_counter: final_sample.counter_evidence(),
                        earliest: final_sample.bounds().earliest_millis(),
                        latest: final_sample.bounds().latest_millis(),
                    };
                    completion.validate(&current.state.binding, &inventory)?;
                    let mut next = current.state.clone();
                    next.revision = next_revision;
                    next.previous_sha256 = Some(json::sha256(&current.raw));
                    next.status = Status::CleanupVerifiedZero;
                    next.cleanup_pending = false;
                    next.zero_retention = Some(completion.reference()?);
                    next.policy_fault = next
                        .policy_fault
                        .as_ref()
                        .map(|f| f.recovered(next_revision));
                    // Final fixed source/SCM/readset check precedes the two writes;
                    // both completion and Time Head publish under this one TxR.
                    source.recheck(host)?;
                    retention::stage_completion_in(&private, &tx, &completion, &inventory)?;
                    let raw = stage_state_in(&private, &actual, &current, &next, &tx)?;
                    (
                        raw,
                        next_revision,
                        None,
                        final_sample.bounds().earliest_millis(),
                    )
                }
            }
            Err(error @ (Error::SourceUnavailable | Error::Unavailable)) => {
                // A valid initial native sample remains knowledge even when a
                // later counter/root interval is unavailable. Authenticate the
                // original source/SCM and census, retain its lower watermark,
                // then publish Unknown with all original evidence intact.
                source.recheck(host)?;
                if retention::read_in(&private, &actual, Some(&tx))? != inventory {
                    return Err(Error::ScopeConflict);
                }
                budgets::retain_samples_floor_in(&private, &actual, &tx, &source, &initial, None)?;
                let next = current
                    .state
                    .next_unknown(Incarnation::from_host(&actual)?, &current.raw)?;
                let raw = stage_state_in(&private, &actual, &current, &next, &tx)?;
                (
                    raw,
                    next.revision,
                    Some(error),
                    initial.bounds().earliest_millis(),
                )
            }
            Err(error) => return Err(error),
        };
        drop(initial);
        drop(source);
        drop(private);
        if unsafe { CommitTransaction(tx.raw()) } == 0 {
            return Err(Error::StateCommitUnknown);
        }
        drop(tx);
        let verify = || -> Result<()> {
            let actual_after = self.read(seed)?.ok_or(Error::StateCommitUnknown)?;
            host.authenticate_running(&actual_after)?;
            if actual_after.digest()? != actual.digest()? {
                return Err(Error::StateCommitUnknown);
            }
            let private = self.key(false, None)?.ok_or(Error::StateCommitUnknown)?;
            let key = open(private.0, NAMESPACE, false, None)?.ok_or(Error::StateCommitUnknown)?;
            let history = open(
                key.0,
                &format!("history\\{}", revision_name(next_revision)),
                false,
                None,
            )?
            .ok_or(Error::StateCommitUnknown)?;
            let custody = open(private.0, retention::NAMESPACE, false, None)?
                .ok_or(Error::StateCommitUnknown)?;
            let budget = open(private.0, budgets::NAMESPACE, false, None)?
                .ok_or(Error::StateCommitUnknown)?;
            if unsafe { RegFlushKey(key.0) } != 0
                || unsafe { RegFlushKey(history.0) } != 0
                || unsafe { RegFlushKey(custody.0) } != 0
                || unsafe { RegFlushKey(budget.0) } != 0
            {
                return Err(Error::StateCommitUnknown);
            }
            if state_in(&private, &actual_after, None)?.raw != raw
                || retention::read_in(&private, &actual_after, None)? != inventory
            {
                return Err(Error::StateCommitUnknown);
            }
            if budgets::floor_in(&private, &actual_after, None)?.map_or(true, |v| v < saved_floor) {
                return Err(Error::StateCommitUnknown);
            }
            Ok(())
        };
        verify().map_err(|_| Error::StateCommitUnknown)?;
        match late_failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    /// Read-only diagnostic under the same short native mutex/transaction as
    /// budget/time reads. No COMMIT, initialization, network or state repair.
    pub(in crate::platform::windows) fn diagnostic_time(
        &self,
        host: &StartedServiceContext,
    ) -> crate::TrustedTimeState {
        match self.diagnostic_time_checked(host) {
            Ok(state) => state,
            Err(error) => protected_diagnostic_error(error),
        }
    }
    #[allow(unsafe_code)]
    fn diagnostic_time_checked(
        &self,
        host: &StartedServiceContext,
    ) -> Result<crate::TrustedTimeState> {
        let seed = &host.context.seed;
        let _guard = BootstrapGuard::acquire(seed, false)?;
        let transaction = Handle::owned(unsafe {
            CreateTransaction(ptr::null_mut(), ptr::null_mut(), 0, 0, 0, 0, ptr::null())
        })?;
        let actual = self
            .read_in(seed, Some(&transaction))?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&actual)?;
        let private = self
            .key(false, Some(&transaction))?
            .ok_or(Error::SetupIncomplete)?;
        // Authenticate all protected sources even when no live interval can be
        // offered. A valid empty budget floor is not a missing budget namespace.
        let _snapshot = state_in(&private, &actual, Some(&transaction))?;
        if _snapshot
            .state
            .policy_fault
            .as_ref()
            .is_some_and(PolicyFault::is_active)
        {
            return Ok(crate::TrustedTimeState::IntegrityFault);
        }
        let _floor = budgets::floor_in(&private, &actual, Some(&transaction))?;
        let source = current_in(&private, &actual, &transaction)?;
        let state = source.state();
        let incarnation = Incarnation::from_host(&actual)?;
        if state.status == Status::Unknown || state.incarnation.as_ref() != Some(&incarnation) {
            source.recheck(host)?;
            return Ok(crate::TrustedTimeState::Unknown);
        }
        // Uses actual stored Root/policy/incarnation plus a fresh native sample;
        // the copied old anchor alone cannot produce this diagnostic state.
        let _current = super::super::live_time::sample_live_time(host, &source)?;
        source.recheck(host)?;
        Ok(if state.status == Status::CleanupVerifiedZero {
            crate::TrustedTimeState::CleanupComplete
        } else {
            crate::TrustedTimeState::CleanupPending
        })
    }
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
        if next.status == Status::CleanupVerifiedZero {
            return Err(Error::StateCorrupt);
        }
        let seed = &host.context.seed;
        let before = self.read(seed)?.ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&before)?;
        let _guard = BootstrapGuard::acquire(seed, false)?;
        // Only state changes that can affect another member's Root validation
        // require the shared product lock. Ordinary time refresh stays local.
        let _product_guard = if next.roots != expected.state.roots
            || next.denied_key_ids != expected.state.denied_key_ids
            || next.policy_fault != expected.state.policy_fault
        {
            Some(super::product_catalog::ProductGuard::acquire(seed)?)
        } else {
            None
        };
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
        // Only the positively-observed native fault publisher and the fixed
        // zero-custody completion path can change incident state.
        if next.policy_fault != current.state.policy_fault {
            return Err(Error::StateCorrupt);
        }
        current
            .state
            .transition(next, &current.raw, &current.original_root)?;
        if next.roots != current.state.roots {
            super::product_catalog::visit_members_in(
                seed,
                &actual,
                &transaction,
                |event, member| {
                    super::metadata::require_member_empty_for_root_transition_in(
                        seed,
                        event,
                        member,
                        &transaction,
                    )?;
                    check_member_denies_in(
                        seed,
                        event,
                        member,
                        next,
                        &current.original_root,
                        &transaction,
                    )
                },
            )
            .map_err(metadata_root_transition_error)?;
        }
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
        if next.status == Status::CleanupPending {
            let actual_floor = current
                .state
                .floor
                .unwrap_or(0)
                .max(budgets::floor_in(&private, &actual, Some(&transaction))?.unwrap_or(0));
            if next
                .evidence
                .as_ref()
                .ok_or(Error::StateCorrupt)?
                .floor_before
                != actual_floor
            {
                return Err(Error::ScopeConflict);
            }
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

fn protected_diagnostic_error(error: Error) -> crate::TrustedTimeState {
    use crate::TrustedTimeState as State;
    match error {
        Error::Unsupported | Error::ProfileDisabled | Error::OperationUnsupported => {
            State::Unsupported
        }
        Error::SetupIncomplete | Error::InitialTrustUnavailable => State::SetupRequired,
        Error::PrerequisiteUnavailable => State::MissingPolicy,
        // Only this actual native protected-store/ACL path may confirm damage.
        Error::StateCorrupt => State::IntegrityFault,
        // Existing native security probes also use SecurityRefused when an OS
        // query fails; source refusal alone does not confirm component damage.
        Error::SecurityRefused => State::SourceUnavailable,
        // A new incarnation or legal read race is not component damage.
        Error::ServiceIdentityRefused | Error::ScopeConflict => State::Unknown,
        _ => State::SourceUnavailable,
    }
}
#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    #[test]
    fn ordinary_offline_crossboot_and_missing_policy_do_not_become_integrity_fault() {
        use crate::TrustedTimeState as State;
        assert_eq!(
            protected_diagnostic_error(Error::SourceUnavailable),
            State::SourceUnavailable
        );
        assert_eq!(
            protected_diagnostic_error(Error::Unavailable),
            State::SourceUnavailable
        );
        assert_eq!(
            protected_diagnostic_error(Error::StateCommitUnknown),
            State::SourceUnavailable
        );
        assert_eq!(
            protected_diagnostic_error(Error::ServiceIdentityRefused),
            State::Unknown
        );
        assert_eq!(
            protected_diagnostic_error(Error::ScopeConflict),
            State::Unknown
        );
        assert_eq!(
            protected_diagnostic_error(Error::PrerequisiteUnavailable),
            State::MissingPolicy
        );
        assert_eq!(
            protected_diagnostic_error(Error::SetupIncomplete),
            State::SetupRequired
        );
        assert_eq!(
            protected_diagnostic_error(Error::StateCorrupt),
            State::IntegrityFault
        );
        assert_eq!(
            protected_diagnostic_error(Error::SecurityRefused),
            State::SourceUnavailable
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_damage_is_not_a_time_policy_incident() {
        assert_eq!(
            metadata_root_transition_error(Error::StateCorrupt),
            Error::RepairRequired
        );
        assert_eq!(
            metadata_root_transition_error(Error::ScopeConflict),
            Error::ScopeConflict
        );
        assert_eq!(
            metadata_root_transition_error(Error::StateCommitUnknown),
            Error::StateCommitUnknown
        );
    }
    #[test]
    #[allow(unsafe_code)]
    fn actual_initial_policy_is_written_once_and_missing_reads_never_heal() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        struct OwnedTestKey {
            key: Option<Key>,
            path: Vec<u16>,
        }
        impl Drop for OwnedTestKey {
            fn drop(&mut self) {
                drop(self.key.take());
                if !self.path.is_empty() {
                    unsafe {
                        RegDeleteTreeW(HKEY_CURRENT_USER, self.path.as_ptr());
                    }
                }
            }
        }
        impl OwnedTestKey {
            fn finish(mut self) {
                drop(self.key.take());
                assert_eq!(
                    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, self.path.as_ptr()) },
                    0
                );
                self.path.clear();
            }
        }
        let path = wide(&format!(
            "Software\\PinvouInitialTimePolicyTest-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut handle = ptr::null_mut();
        let mut disposition = 0;
        assert_eq!(
            unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    path.as_ptr(),
                    0,
                    ptr::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_READ | KEY_WRITE,
                    ptr::null(),
                    &mut handle,
                    &mut disposition,
                )
            },
            0
        );
        let key = Key(handle);
        assert_eq!(disposition, REG_CREATED_NEW_KEY);
        let owned = OwnedTestKey {
            key: Some(key),
            path,
        };
        let key = owned.key.as_ref().unwrap();
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
        assert!(matches!(
            policy_in(key, &binding),
            Err(Error::PrerequisiteUnavailable)
        ));
        initialize_policy_in(key, &binding).unwrap();
        let first = policy_in(key, &binding).unwrap();
        assert_eq!(first.policy.max_round_trip_ms, 30_000);
        assert_eq!(first.policy.max_uncertainty_ms, 1_000);
        assert_eq!(
            initialize_policy_in(key, &binding),
            Err(Error::StateCorrupt)
        );
        assert_eq!(policy_in(key, &binding).unwrap().raw, first.raw);
        let mut other = binding.clone();
        other.original_install_operation_id = "other".into();
        assert!(matches!(policy_in(key, &other), Err(Error::StateCorrupt)));
        let mut administrative = first.policy;
        administrative.max_round_trip_ms = 15_000;
        let changed = json::encode(&administrative, false).unwrap();
        write(key, "Policy", &changed).unwrap();
        assert_eq!(
            initialize_policy_in(key, &binding),
            Err(Error::StateCorrupt)
        );
        assert_eq!(policy_in(key, &binding).unwrap().raw, changed);
        assert_eq!(
            verify_initial_policy_in(key, &binding),
            Err(Error::StateCorrupt)
        );
        assert_eq!(
            unsafe { RegDeleteValueW(key.0, wide("Policy").as_ptr()) },
            0
        );
        assert!(matches!(
            policy_in(key, &binding),
            Err(Error::PrerequisiteUnavailable)
        ));
        require_absent_value(key, "Policy").unwrap();
        owned.finish();
    }

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
        assert!(
            !serde_json::from_slice::<serde_json::Value>(&raw)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("zeroRetention")
        );
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        value["cleanupComplete"] = serde_json::Value::Bool(true);
        assert!(json::decode::<Header>(&json::encode(&value, false).unwrap(), false).is_err());
        value.as_object_mut().unwrap().remove("cleanupComplete");
        value.as_object_mut().unwrap().remove("roots");
        assert!(json::decode::<Header>(&json::encode(&value, false).unwrap(), false).is_err());
    }
}

#[cfg(test)]
mod retained_completion_census_tests {
    use super::*;
    fn fixture() -> (Binding, Vec<Vec<u8>>) {
        // Closed Header/linked-byte DATA controls only. These deliberately do
        // not construct a valid native SCM source, time result or clean permit.
        let vectors: serde_json::Value = serde_json::from_slice(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../specs/upgrade/v1/vectors/time/anchor.json"
        )))
        .unwrap();
        let root = vectors["cases"][0]["trustedRootBytes"]
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
        let mut rows: Vec<Vec<u8>> = Vec::new();
        for revision in 1..=8 {
            let mut row = header(&State::genesis(binding.clone(), root).unwrap());
            row.revision = revision;
            row.previous_sha256 = rows.last().map(|raw| json::sha256(raw));
            if revision == 3 || revision == 6 {
                row.status = Status::CleanupVerifiedZero;
                row.cleanup_pending = false;
                row.zero_retention = Some(crate::retention_custody::ZeroRetentionReference {
                    completion_revision: revision,
                    completion_sha256: format!("{revision:x}").repeat(64),
                });
            } else if revision == 2 || revision == 5 || revision == 8 {
                row.status = Status::CleanupPending;
            }
            rows.push(json::encode(&row, false).unwrap());
        }
        (binding, rows)
    }
    #[test]
    fn older_completion_loss_is_detected_after_unknown_and_a_new_anchor() {
        let (binding, rows) = fixture();
        let mut census = RetainedCompletionCensus::new(&rows[7], &rows[0], &binding).unwrap();
        for (index, raw) in rows.iter().enumerate() {
            census
                .observe(&revision_name(index as u64 + 1), raw)
                .unwrap();
        }
        let all = census.finish().unwrap();
        assert_eq!(
            all.iter()
                .map(|r| r.completion_revision)
                .collect::<Vec<_>>(),
            [3, 6]
        );
        retention::exact_completion_references(&all, &all).unwrap();
        // Both references are older than current/previous. Losing revision 3
        // must fail even though revision 6 and the current anchor remain.
        assert!(retention::exact_completion_references(&all, &all[1..]).is_err());
        let mut wrong = all.clone();
        wrong[0].completion_sha256 = "f".repeat(64);
        assert!(retention::exact_completion_references(&all, &wrong).is_err());
        assert!(
            retention::exact_completion_references(&all, &[all[0].clone(), all[0].clone()])
                .is_err()
        );
        let mut hole = RetainedCompletionCensus::new(&rows[7], &rows[0], &binding).unwrap();
        hole.observe(&revision_name(1), &rows[0]).unwrap();
        assert!(hole.observe(&revision_name(3), &rows[2]).is_err());
        let mut short = RetainedCompletionCensus::new(&rows[7], &rows[0], &binding).unwrap();
        short.observe(&revision_name(1), &rows[0]).unwrap();
        assert!(short.finish().is_err());
        let mut tampered = RetainedCompletionCensus::new(&rows[7], &rows[0], &binding).unwrap();
        let mut bad: Header = json::decode(&rows[0], false).unwrap();
        bad.binding.original_install_operation_id = "another".into();
        assert!(
            tampered
                .observe(&revision_name(1), &json::encode(&bad, false).unwrap())
                .is_err()
        );
    }
    #[test]
    fn a_census_capacity_overflow_or_mismatched_head_never_becomes_an_empty_result() {
        let (binding, rows) = fixture();
        let mut huge: Header = json::decode(&rows[7], false).unwrap();
        huge.revision = u64::from(MAX_CENSUS_TIME_HEADERS) + 1;
        assert!(matches!(
            RetainedCompletionCensus::new(&json::encode(&huge, false).unwrap(), &rows[0], &binding),
            Err(Error::Unavailable)
        ));
        let mut wrong_head: Header = json::decode(&rows[7], false).unwrap();
        wrong_head.floor = Some(1);
        let mut census = RetainedCompletionCensus::new(
            &json::encode(&wrong_head, false).unwrap(),
            &rows[0],
            &binding,
        )
        .unwrap();
        for (index, raw) in rows.iter().enumerate() {
            census
                .observe(&revision_name(index as u64 + 1), raw)
                .unwrap();
        }
        assert!(census.finish().is_err());
    }
}
