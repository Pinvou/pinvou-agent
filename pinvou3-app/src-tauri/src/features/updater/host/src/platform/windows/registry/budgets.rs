//! Native protected budget-window records. A window is a timing intention,
//! never evidence that an installer, writer, health check or consume has run.
//! Real stage owners must compose their first action and this window in the
//! same protected operation; no public IPC or execution permission is exposed.
use super::super::{installed::StartedServiceContext, live_time};
use super::*;
use crate::{
    time_budget::{Assessment, BudgetWindow, TimeInterval, WindowId, WindowKind},
    trusted_time::{Binding, CounterEvidence, Incarnation},
};
use serde::{Deserialize, Serialize};

pub(super) const NAMESPACE: &str = "time-budgets-v1";
const MAX_WINDOWS: usize = 4096;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    key: String,
    revision: u64,
    sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Head {
    protocol_version: u8,
    binding: Binding,
    revision: u64,
    previous_sha256: Option<String>,
    windows: Vec<Entry>,
    live_floor: Option<u64>,
}
impl Head {
    fn genesis(binding: Binding) -> Self {
        Self {
            protocol_version: 1,
            binding,
            revision: 1,
            previous_sha256: None,
            windows: Vec::new(),
            live_floor: None,
        }
    }
    fn validate(&self, binding: &Binding) -> Result<()> {
        binding.validate()?;
        if self.protocol_version != 1
            || self.binding != *binding
            || self.revision == 0
            || self.revision > json::SAFE_MAX
            || (self.revision == 1) != self.previous_sha256.is_none()
            || self
                .previous_sha256
                .as_ref()
                .is_some_and(|s| !json::digest(s))
            || self.live_floor.is_some_and(|v| v > json::SAFE_MAX)
            || (self.revision == 1 && self.live_floor.is_some())
            || self.windows.len() > MAX_WINDOWS
            || self.windows.iter().any(|e| {
                !json::digest(&e.key)
                    || !json::digest(&e.sha256)
                    || e.revision == 0
                    || e.revision > json::SAFE_MAX
            })
            || self.windows.windows(2).any(|w| w[0].key >= w[1].key)
            || (self.revision == 1 && !self.windows.is_empty())
        {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }
    fn successor(&self, next: &Self, raw: &[u8]) -> Result<()> {
        next.validate(&self.binding)?;
        if next.revision != self.revision.checked_add(1).ok_or(Error::Unavailable)?
            || next.previous_sha256 != Some(json::sha256(raw))
            || next.live_floor.unwrap_or(0) < self.live_floor.unwrap_or(0)
            || (self.live_floor.is_some() && next.live_floor.is_none())
        {
            return Err(Error::StateCorrupt);
        }
        let mut changes = 0;
        for old in &self.windows {
            let new = next
                .windows
                .iter()
                .find(|e| e.key == old.key)
                .ok_or(Error::StateCorrupt)?;
            if new != old {
                if new.revision != old.revision.checked_add(1).ok_or(Error::Unavailable)? {
                    return Err(Error::StateCorrupt);
                }
                changes += 1;
            }
        }
        for new in &next.windows {
            if !self.windows.iter().any(|e| e.key == new.key) {
                if new.revision != 1 {
                    return Err(Error::StateCorrupt);
                }
                changes += 1;
            }
        }
        if changes != 1 {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }
}

/// Historical source identity, not a persistable live-clock capability.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Observation {
    time_revision: u64,
    time_head_sha256: String,
    policy_sha256: String,
    incarnation: Incarnation,
    counters: CounterEvidence,
    earliest: u64,
    latest: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    revision: u64,
    previous_sha256: Option<String>,
    window: BudgetWindow,
    observations: Vec<Observation>,
}
impl Record {
    fn validate(&self, id: &WindowId) -> Result<()> {
        self.window.validate()?;
        if self.window.id() != id
            || self.revision == 0
            || self.revision > json::SAFE_MAX
            || (self.revision == 1) != self.previous_sha256.is_none()
            || self
                .previous_sha256
                .as_ref()
                .is_some_and(|s| !json::digest(s))
        {
            return Err(Error::StateCorrupt);
        }
        if self.observations.len() > 2 {
            return Err(Error::StateCorrupt);
        }
        for o in &self.observations {
            if o.time_revision == 0
                || o.time_revision > json::SAFE_MAX
                || !json::digest(&o.time_head_sha256)
                || !json::digest(&o.policy_sha256)
                || o.earliest > o.latest
                || o.latest > json::SAFE_MAX
            {
                return Err(Error::StateCorrupt);
            }
            o.incarnation.validate()?;
            o.counters.data()?;
        }
        Ok(())
    }
}

fn read_head(key: &Key, binding: &Binding, tx: Option<&Handle>) -> Result<(Head, Vec<u8>)> {
    let genesis_raw = read(key, "Genesis")?;
    let genesis = Head::genesis(binding.clone());
    if genesis_raw != json::encode(&genesis, false)? {
        return Err(Error::StateCorrupt);
    }
    let raw = read(key, "Head")?;
    let h: Head = json::decode(&raw, false)?;
    h.validate(binding)?;
    if raw != json::encode(&h, false)? {
        return Err(Error::StateCorrupt);
    }
    let current = open(
        key.0,
        &format!("history\\{}", revision_name(h.revision)),
        false,
        tx,
    )?
    .ok_or(Error::StateCorrupt)?;
    if read(&current, "Record")? != raw {
        return Err(Error::StateCorrupt);
    }
    if h.revision == 1 {
        if raw != genesis_raw {
            return Err(Error::StateCorrupt);
        }
    } else {
        let prev = open(
            key.0,
            &format!("history\\{}", revision_name(h.revision - 1)),
            false,
            tx,
        )?
        .ok_or(Error::StateCorrupt)?;
        let prev_raw = read(&prev, "Record")?;
        let before: Head = json::decode(&prev_raw, false)?;
        before.validate(binding)?;
        if json::encode(&before, false)? != prev_raw {
            return Err(Error::StateCorrupt);
        }
        before.successor(&h, &prev_raw)?;
    }
    Ok((h, raw))
}
fn window_key(id: &WindowId) -> Result<String> {
    Ok(json::sha256(&json::encode(id, false)?))
}
fn read_window(
    key: &Key,
    h: &Head,
    id: &WindowId,
    tx: Option<&Handle>,
) -> Result<Option<(Record, Vec<u8>)>> {
    let name = window_key(id)?;
    let Some(entry) = h.windows.iter().find(|e| e.key == name) else {
        return Ok(None);
    };
    let path = format!("windows\\{}\\{}", name, revision_name(entry.revision));
    let record_key = open(key.0, &path, false, tx)?.ok_or(Error::StateCorrupt)?;
    let raw = read(&record_key, "Record")?;
    if json::sha256(&raw) != entry.sha256 {
        return Err(Error::StateCorrupt);
    }
    let record: Record = json::decode(&raw, false)?;
    record.validate(id)?;
    if record.revision != entry.revision || json::encode(&record, false)? != raw {
        return Err(Error::StateCorrupt);
    }
    if record.revision > 1 {
        let prior_key = open(
            key.0,
            &format!("windows\\{}\\{}", name, revision_name(entry.revision - 1)),
            false,
            tx,
        )?
        .ok_or(Error::StateCorrupt)?;
        let prior_raw = read(&prior_key, "Record")?;
        let prior: Record = json::decode(&prior_raw, false)?;
        prior.validate(id)?;
        if json::encode(&prior, false)? != prior_raw
            || record.previous_sha256 != Some(json::sha256(&prior_raw))
            || prior.revision.checked_add(1) != Some(record.revision)
        {
            return Err(Error::StateCorrupt);
        }
        prior.window.check_successor(&record.window)?;
    }
    Ok(Some((record, raw)))
}

/// Fresh only, called in the original registration transaction. This establishes
/// this budget namespace, not an empty backup/secret/stage census.
pub(super) fn initialize_in(private: &Key, tx: &Handle, host: &HostRegistration) -> Result<()> {
    let head = Head::genesis(Binding::from_host(host));
    let raw = json::encode(&head, false)?;
    let key = create(private.0, NAMESPACE, tx, false, true)?;
    let history = create(key.0, "history", tx, false, true)?;
    let first = create(history.0, &revision_name(1), tx, false, true)?;
    create(key.0, "windows", tx, false, true)?;
    write(&key, "Genesis", &raw)?;
    write(&key, "Head", &raw)?;
    write(&first, "Record", &raw)?;
    require_absent_value(&key, "Policy")?;
    let (_, observed) = read_head(&key, &head.binding, Some(tx))?;
    if observed != raw {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}
pub(super) fn verify_fresh_in(private: &Key, host: &HostRegistration) -> Result<()> {
    let key = open(private.0, NAMESPACE, false, None)?.ok_or(Error::StateCommitUnknown)?;
    let (head, _) = read_head(&key, &Binding::from_host(host), None)?;
    if head.revision != 1 || !head.windows.is_empty() {
        return Err(Error::StateCommitUnknown);
    }
    Ok(())
}

/// Global lower watermark from genuine committed native observations. It is
/// separate from a window's spent high-water and cannot be reset by reissuing
/// a fresh signed time anchor. Missing old namespaces never imply zero.
pub(super) fn floor_for_binding(
    key: &Key,
    binding: &Binding,
    tx: Option<&Handle>,
) -> Result<Option<u64>> {
    Ok(read_head(key, binding, tx)?.0.live_floor)
}
pub(super) fn floor_in(
    private: &Key,
    actual: &HostRegistration,
    tx: Option<&Handle>,
) -> Result<Option<u64>> {
    let key = open(private.0, NAMESPACE, false, tx)?.ok_or(Error::SetupIncomplete)?;
    Ok(read_head(&key, &Binding::from_host(actual), tx)?
        .0
        .live_floor)
}
impl Store {
    pub(in crate::platform::windows) fn budget_time_floor(
        &self,
        seed: &Seed,
        expected: &HostRegistration,
    ) -> Result<Option<u64>> {
        let actual = self.read(seed)?.ok_or(Error::SetupIncomplete)?;
        if actual.digest()? != expected.digest()? {
            return Err(Error::ScopeConflict);
        }
        let private = self.key(false, None)?.ok_or(Error::SetupIncomplete)?;
        let floor = floor_in(&private, &actual, None)?;
        let after = self.read(seed)?.ok_or(Error::SetupIncomplete)?;
        if after.digest()? != actual.digest()? {
            return Err(Error::ScopeConflict);
        }
        Ok(floor)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Durations {
    prep_freeze_ms: u64,
    backup_ms: u64,
    validation_ms: u64,
    preinstall_ms: u64,
    install_ms: u64,
    activate_ms: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BudgetPolicy {
    protocol_version: u8,
    binding: Binding,
    revision: u64,
    durations: Durations,
}
impl BudgetPolicy {
    fn parse(raw: &[u8], binding: &Binding) -> Result<Self> {
        let p: Self = json::decode(raw, false)?;
        let d = &p.durations;
        if p.protocol_version != 1
            || p.binding != *binding
            || p.revision == 0
            || p.revision > json::SAFE_MAX
            || json::encode(&p, false)? != raw
            || [
                d.prep_freeze_ms,
                d.backup_ms,
                d.validation_ms,
                d.preinstall_ms,
                d.install_ms,
                d.activate_ms,
            ]
            .iter()
            .any(|v| *v == 0 || *v > json::SAFE_MAX)
            || d.validation_ms > 30 * 60 * 1000
            || [d.preinstall_ms, d.install_ms, d.activate_ms]
                .iter()
                .any(|v| *v > 2 * 60 * 60 * 1000)
        {
            return Err(Error::StateCorrupt);
        }
        p.binding.validate()?;
        Ok(p)
    }
    fn duration(&self, kind: WindowKind) -> Result<u64> {
        Ok(match kind {
            WindowKind::PrepFreeze => self.durations.prep_freeze_ms,
            WindowKind::Backup => self.durations.backup_ms,
            WindowKind::Validation => self.durations.validation_ms,
            WindowKind::Preinstall => self.durations.preinstall_ms,
            WindowKind::Install => self.durations.install_ms,
            WindowKind::Activate => self.durations.activate_ms,
            _ => return Err(Error::StateCorrupt),
        })
    }
}
fn policy(key: &Key, binding: &Binding) -> Result<(BudgetPolicy, Vec<u8>)> {
    // The existing native helper distinguishes absence before the strict read.
    let name = wide("Policy");
    let mut size = 0;
    #[allow(unsafe_code)]
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
    Ok((BudgetPolicy::parse(&raw, binding)?, raw))
}
fn qualified(kind: WindowKind) -> bool {
    matches!(
        kind,
        WindowKind::PrepFreeze
            | WindowKind::Backup
            | WindowKind::Validation
            | WindowKind::Preinstall
            | WindowKind::Install
            | WindowKind::Activate
    )
}

/// Internal budget intentions only. The future actual stage owner must validate
/// its original operation/session and combine this call with that owner's native
/// commit. A WindowId/session number is DATA, never authenticated stage authority.
pub(in crate::platform::windows) enum Command {
    Begin {
        original_session_cutoff_ms: Option<u64>,
    },
    Assess,
    Tighten {
        session_cutoff_ms: Option<u64>,
    },
}
pub(in crate::platform::windows) struct Snapshot {
    pub(in crate::platform::windows) window: BudgetWindow,
    pub(in crate::platform::windows) assessment: Assessment,
    pub(in crate::platform::windows) revision: u64,
}
fn parent(
    key: &Key,
    head: &Head,
    id: &WindowId,
    kind: WindowKind,
    tx: &Handle,
) -> Result<BudgetWindow> {
    let parent_id = WindowId::new(id.binding().clone(), id.operation_id().to_owned(), kind)?;
    let mut window = read_window(key, head, &parent_id, Some(tx))?
        .ok_or(Error::PrerequisiteUnavailable)?
        .0
        .window;
    if kind == WindowKind::HealthTotal {
        retain_family(key, head, &mut window, tx)?;
    }
    Ok(window)
}
fn retain_family(key: &Key, head: &Head, total: &mut BudgetWindow, tx: &Handle) -> Result<()> {
    for kind in [WindowKind::HealthCheck1, WindowKind::HealthCheck2] {
        let id = WindowId::new(
            total.id().binding().clone(),
            total.id().operation_id().to_owned(),
            kind,
        )?;
        if let Some((record, _)) = read_window(key, head, &id, Some(tx))? {
            total.retain_health_child(&record.window)?;
        }
    }
    Ok(())
}
fn retain_samples(
    initial: Option<(TimeInterval, Observation)>,
    final_sample: Option<(TimeInterval, Observation)>,
) -> Vec<Observation> {
    initial
        .into_iter()
        .chain(final_sample)
        .map(|(_, observation)| observation)
        .collect()
}
fn observed_floor(previous: Option<u64>, observations: &[Observation]) -> Option<u64> {
    observations.iter().fold(previous, |floor, sample| {
        Some(floor.unwrap_or(0).max(sample.earliest))
    })
}

fn observe(
    host: &StartedServiceContext,
    source: Option<&time::LiveTimeReadSet<'_>>,
) -> Result<Option<(TimeInterval, Observation)>> {
    let Some(source) = source else {
        return Ok(None);
    };
    let sample = match live_time::sample_live_time(host, source) {
        Ok(s) => s,
        Err(
            Error::SourceUnavailable
            | Error::PrerequisiteUnavailable
            | Error::SetupIncomplete
            | Error::Unavailable,
        ) => return Ok(None),
        Err(e) => return Err(e),
    };
    let bounds = sample.bounds();
    let interval = TimeInterval::new(bounds.earliest_millis(), bounds.latest_millis())?;
    Ok(Some((
        interval,
        Observation {
            time_revision: source.state().revision,
            time_head_sha256: source.head_sha256(),
            policy_sha256: json::sha256(&source.policy().raw),
            incarnation: source
                .state()
                .incarnation
                .clone()
                .ok_or(Error::SourceUnavailable)?,
            counters: sample.counter_evidence(),
            earliest: bounds.earliest_millis(),
            latest: bounds.latest_millis(),
        },
    )))
}

impl Store {
    /// Records and evaluates only a budget window. It never performs an actual
    /// stage, clears cleanup_pending, relaxes a fence, deletes a secret or
    /// grants installation permission. Missing legacy namespaces are refused.
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn budget_window(
        &self,
        host: &StartedServiceContext,
        id: &WindowId,
        command: Command,
    ) -> Result<Snapshot> {
        let seed = &host.context.seed;
        if WindowId::new(
            id.binding().clone(),
            id.operation_id().to_owned(),
            id.kind(),
        )? != *id
        {
            return Err(Error::StateCorrupt);
        }
        let before = self.read(seed)?.ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&before)?;
        if Binding::from_host(&before) != *id.binding() {
            return Err(Error::ScopeConflict);
        }
        let _guard = BootstrapGuard::acquire(seed, false)?;
        let tx = Handle::owned(unsafe {
            CreateTransaction(ptr::null_mut(), ptr::null_mut(), 0, 0, 0, 0, ptr::null())
        })?;
        let actual = self
            .read_in(seed, Some(&tx))?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&actual)?;
        if actual.digest()? != before.digest()? {
            return Err(Error::ScopeConflict);
        }
        let private = self.key(true, Some(&tx))?.ok_or(Error::SetupIncomplete)?;
        let key = open(private.0, NAMESPACE, true, Some(&tx))?.ok_or(Error::SetupIncomplete)?;
        let (head, head_raw) = read_head(&key, id.binding(), Some(&tx))?;
        let old = read_window(&key, &head, id, Some(&tx))?;
        // The time source is constructed here from the exact protected native
        // key and original transaction, never supplied by this method's caller.
        let source = match time::current_in(&private, &actual, &tx) {
            Ok(s) => Some(s),
            Err(
                Error::PrerequisiteUnavailable
                | Error::SourceUnavailable
                | Error::SetupIncomplete
                | Error::Unavailable,
            ) => None,
            Err(e) => return Err(e),
        };
        let initial = observe(host, source.as_ref())?;
        let total = match id.kind() {
            WindowKind::HealthCheck1 | WindowKind::HealthCheck2 => {
                Some(parent(&key, &head, id, WindowKind::HealthTotal, &tx)?)
            }
            _ => None,
        };
        let mut captured_policy = None;
        let mut window = match (&old, &command) {
            (Some((record, _)), _) => record.window.clone(),
            (
                None,
                Command::Begin {
                    original_session_cutoff_ms,
                },
            ) => {
                let origin = initial.as_ref().ok_or(Error::SourceUnavailable)?.0;
                if qualified(id.kind()) {
                    let (p, raw) = policy(&key, id.binding())?;
                    let duration = p.duration(id.kind())?;
                    captured_policy = Some(raw);
                    BudgetWindow::begin_qualified(
                        id.clone(),
                        origin,
                        duration,
                        *original_session_cutoff_ms,
                    )?
                } else {
                    if original_session_cutoff_ms.is_some() {
                        return Err(Error::StateCorrupt);
                    }
                    match id.kind() {
                        WindowKind::HealthTotal => {
                            BudgetWindow::begin_health_total(id.clone(), origin)?
                        }
                        WindowKind::ConsumeWait => {
                            BudgetWindow::begin_consume_wait(id.clone(), origin)?
                        }
                        WindowKind::HealthCheck1 | WindowKind::HealthCheck2 => {
                            let previous = if id.kind() == WindowKind::HealthCheck2 {
                                Some(parent(&key, &head, id, WindowKind::HealthCheck1, &tx)?)
                            } else {
                                None
                            };
                            BudgetWindow::begin_health_check(
                                id.clone(),
                                origin,
                                total.as_ref().ok_or(Error::PrerequisiteUnavailable)?,
                                previous.as_ref(),
                            )?
                        }
                        _ => return Err(Error::StateCorrupt),
                    }
                }
            }
            (None, _) => return Err(Error::PrerequisiteUnavailable),
        };
        match command {
            Command::Begin {
                original_session_cutoff_ms,
            } if old.is_some() => {
                // Reopening an original intention never chooses a new origin
                // or later duration, including after policy changes/restarts.
                if let Some(cutoff) = original_session_cutoff_ms {
                    if let Some(old_cutoff) = window.session_cutoff_ms() {
                        window.tighten_session_cutoff(cutoff.min(old_cutoff))?;
                    } else {
                        return Err(Error::StateCorrupt);
                    }
                } else if window.session_cutoff_ms().is_some() {
                    return Err(Error::StateCorrupt);
                }
            }
            Command::Tighten { session_cutoff_ms } => {
                if qualified(id.kind()) {
                    let (p, raw) = policy(&key, id.binding())?;
                    window.tighten_duration(p.duration(id.kind())?.min(window.duration_ms()))?;
                    captured_policy = Some(raw);
                }
                if let Some(cutoff) = session_cutoff_ms {
                    let prior = window.session_cutoff_ms().ok_or(Error::StateCorrupt)?;
                    window.tighten_session_cutoff(cutoff.min(prior))?;
                }
            }
            _ => {}
        }
        if id.kind() == WindowKind::HealthTotal {
            retain_family(&key, &head, &mut window, &tx)?;
        }
        // Every successful real sample contributes timing knowledge, even if
        // the later sample becomes unavailable. Do not drop its upper spend
        // or confirmed reach, and do not use it as the final current result.
        if let Some((bounds, _)) = &initial {
            let _ = window.assess(Some(*bounds), total.as_ref())?;
        }
        // Fresh second native sampling includes local verification/model work.
        // The result is still an as-of assessment, not a future action permit.
        let observation = observe(host, source.as_ref())?;
        let assessment = window.assess(observation.as_ref().map(|x| x.0), total.as_ref())?;
        if let Some((record, _)) = &old {
            record.window.check_successor(&window)?;
        }
        let record = Record {
            revision: old.as_ref().map_or(Ok(1), |x| {
                x.0.revision.checked_add(1).ok_or(Error::Unavailable)
            })?,
            previous_sha256: old.as_ref().map(|x| json::sha256(&x.1)),
            window,
            observations: retain_samples(initial, observation),
        };
        record.validate(id)?;
        let raw = json::encode(&record, false)?;
        let name = window_key(id)?;
        let mut next = head.clone();
        next.revision = next.revision.checked_add(1).ok_or(Error::Unavailable)?;
        next.previous_sha256 = Some(json::sha256(&head_raw));
        next.live_floor = observed_floor(next.live_floor, &record.observations);
        let entry = Entry {
            key: name.clone(),
            revision: record.revision,
            sha256: json::sha256(&raw),
        };
        match next.windows.binary_search_by(|e| e.key.cmp(&name)) {
            Ok(i) => next.windows[i] = entry,
            Err(i) => {
                if next.windows.len() == MAX_WINDOWS {
                    return Err(Error::Unavailable);
                }
                next.windows.insert(i, entry);
            }
        }
        head.successor(&next, &head_raw)?;
        let next_raw = json::encode(&next, false)?;
        if let Some(s) = &source {
            s.recheck(host)?;
        }
        if let Some(captured) = captured_policy {
            if policy(&key, id.binding())?.1 != captured {
                return Err(Error::ScopeConflict);
            }
        }
        if read(&key, "Head")? != head_raw {
            return Err(Error::ScopeConflict);
        }
        let final_host = self
            .read_in(seed, Some(&tx))?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&final_host)?;
        if final_host.digest()? != actual.digest()? {
            return Err(Error::ScopeConflict);
        }
        let windows = open(key.0, "windows", true, Some(&tx))?.ok_or(Error::StateCorrupt)?;
        let own = if old.is_none() {
            create(windows.0, &name, &tx, false, true)?
        } else {
            open(windows.0, &name, true, Some(&tx))?.ok_or(Error::StateCorrupt)?
        };
        let revision = create(own.0, &revision_name(record.revision), &tx, false, true)?;
        write(&revision, "Record", &raw)?;
        let history = open(key.0, "history", true, Some(&tx))?.ok_or(Error::StateCorrupt)?;
        let index_revision = create(history.0, &revision_name(next.revision), &tx, false, true)?;
        write(&index_revision, "Record", &next_raw)?;
        write(&key, "Head", &next_raw)?;
        let (seen, seen_raw) = read_head(&key, id.binding(), Some(&tx))?;
        let seen_window = read_window(&key, &seen, id, Some(&tx))?.ok_or(Error::StateCorrupt)?;
        if seen_raw != next_raw || seen_window.1 != raw {
            return Err(Error::StateCorrupt);
        }
        if let Some(s) = &source {
            s.recheck(host)?;
        }
        drop(source);
        drop(index_revision);
        drop(history);
        drop(revision);
        drop(own);
        drop(windows);
        drop(key);
        drop(private);
        if unsafe { CommitTransaction(tx.raw()) } == 0 {
            return Err(Error::StateCommitUnknown);
        }
        drop(tx);
        let verify = || -> Result<()> {
            let current = self.read(seed)?.ok_or(Error::StateCommitUnknown)?;
            host.authenticate_running(&current)?;
            if current.digest()? != actual.digest()? {
                return Err(Error::StateCommitUnknown);
            }
            let private = self.key(false, None)?.ok_or(Error::StateCommitUnknown)?;
            let key = open(private.0, NAMESPACE, false, None)?.ok_or(Error::StateCommitUnknown)?;
            let history = open(
                key.0,
                &format!("history\\{}", revision_name(next.revision)),
                false,
                None,
            )?
            .ok_or(Error::StateCommitUnknown)?;
            let written = open(
                key.0,
                &format!("windows\\{}\\{}", name, revision_name(record.revision)),
                false,
                None,
            )?
            .ok_or(Error::StateCommitUnknown)?;
            if unsafe { RegFlushKey(key.0) } != 0
                || unsafe { RegFlushKey(history.0) } != 0
                || unsafe { RegFlushKey(written.0) } != 0
            {
                return Err(Error::StateCommitUnknown);
            }
            let (readback, bytes) = read_head(&key, id.binding(), None)?;
            let (_, window_bytes) =
                read_window(&key, &readback, id, None)?.ok_or(Error::StateCommitUnknown)?;
            if bytes != next_raw || window_bytes != raw {
                return Err(Error::StateCommitUnknown);
            }
            Ok(())
        };
        verify().map_err(|_| Error::StateCommitUnknown)?;
        Ok(Snapshot {
            window: record.window,
            assessment,
            revision: record.revision,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn progressed(before: &Head, key: &str) -> Head {
        let mut after = before.clone();
        after.revision += 1;
        after.previous_sha256 = Some(json::sha256(&json::encode(before, false).unwrap()));
        after.windows.push(Entry {
            key: key.into(),
            revision: 1,
            sha256: "c".repeat(64),
        });
        after.windows.sort_by(|a, b| a.key.cmp(&b.key));
        after
    }
    #[test]
    fn native_index_retains_original_windows_and_global_time_watermark() {
        let first = Head::genesis(binding());
        let first_raw = json::encode(&first, false).unwrap();
        let mut saved = progressed(&first, &"1".repeat(64));
        saved.live_floor = Some(100);
        first.successor(&saved, &first_raw).unwrap();
        let raw = json::encode(&saved, false).unwrap();
        let next = progressed(&saved, &"2".repeat(64));
        saved.successor(&next, &raw).unwrap();
        let mut bad = next.clone();
        bad.live_floor = Some(99);
        assert!(saved.successor(&bad, &raw).is_err());
        let mut bad = next.clone();
        bad.live_floor = None;
        assert!(saved.successor(&bad, &raw).is_err());
        let mut bad = next.clone();
        bad.windows.remove(0);
        assert!(saved.successor(&bad, &raw).is_err());
        let mut bad = next;
        bad.windows[0].revision += 1;
        assert!(saved.successor(&bad, &raw).is_err());
    }
    #[test]
    fn native_index_requires_exact_previous_and_one_window_change() {
        let first = Head::genesis(binding());
        let raw = json::encode(&first, false).unwrap();
        let next = progressed(&first, &"1".repeat(64));
        first.successor(&next, &raw).unwrap();
        let mut bad = next.clone();
        bad.previous_sha256 = Some("0".repeat(64));
        assert!(first.successor(&bad, &raw).is_err());
        let mut bad = next.clone();
        bad.windows[0].revision = 2;
        assert!(first.successor(&bad, &raw).is_err());
        let mut bad = next;
        bad.windows.push(Entry {
            key: "2".repeat(64),
            revision: 1,
            sha256: "d".repeat(64),
        });
        assert!(first.successor(&bad, &raw).is_err());
    }
    #[test]
    fn mandatory_target_budget_policy_cannot_default_widen_or_change_scope() {
        let p = BudgetPolicy {
            protocol_version: 1,
            binding: binding(),
            revision: 1,
            durations: Durations {
                prep_freeze_ms: 100,
                backup_ms: 100,
                validation_ms: 30 * 60 * 1000,
                preinstall_ms: 2 * 60 * 60 * 1000,
                install_ms: 2 * 60 * 60 * 1000,
                activate_ms: 2 * 60 * 60 * 1000,
            },
        };
        let raw = json::encode(&p, false).unwrap();
        BudgetPolicy::parse(&raw, &binding()).unwrap();
        let mut v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        v["durations"].as_object_mut().unwrap().remove("backupMs");
        assert!(BudgetPolicy::parse(&json::encode(&v, false).unwrap(), &binding()).is_err());
        let mut v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        v["durations"]["validationMs"] = serde_json::json!(30 * 60 * 1000 + 1);
        assert!(BudgetPolicy::parse(&json::encode(&v, false).unwrap(), &binding()).is_err());
        let mut other = binding();
        other.target_key = "other".into();
        assert!(BudgetPolicy::parse(&raw, &other).is_err());
        let mut v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        v["durations"]["backupMs"] = serde_json::json!(0);
        assert!(BudgetPolicy::parse(&json::encode(&v, false).unwrap(), &binding()).is_err());
    }
    fn observation(lower: u64, upper: u64) -> (TimeInterval, Observation) {
        use crate::{
            identity::{FileTime, ProcessSelector},
            monotonic::CounterObservation,
        };
        (
            TimeInterval::new(lower, upper).unwrap(),
            Observation {
                time_revision: 1,
                time_head_sha256: "a".repeat(64),
                policy_sha256: "b".repeat(64),
                incarnation: Incarnation {
                    host_epoch: 1,
                    instance_nonce: "c".repeat(64),
                    process: ProcessSelector {
                        process_id: 1,
                        creation_file_time: FileTime::from_native(1, 2),
                    },
                },
                counters: CounterEvidence::from_data(
                    CounterObservation::from_bracket(10, 1, 2, 11).unwrap(),
                ),
                earliest: lower,
                latest: upper,
            },
        )
    }
    #[test]
    fn unavailable_final_sample_cannot_forget_origin_floor_or_upper_budget_spend() {
        // Publication-control DATA only; these observations do not mint an SCM
        // source, native transaction or stage/installation permission.
        let initial = observation(100, 1001);
        let final_sample = None;
        let id = WindowId::new(binding(), "operation".into(), WindowKind::Validation).unwrap();
        let mut window =
            BudgetWindow::begin_qualified(id, TimeInterval::new(0, 0).unwrap(), 1000, None)
                .unwrap();
        let _ = window.assess(Some(initial.0), None).unwrap();
        assert!(
            window
                .assess(
                    final_sample
                        .as_ref()
                        .map(|x: &(TimeInterval, Observation)| x.0),
                    None
                )
                .unwrap()
                == Assessment::Unknown
        );
        let samples = retain_samples(Some(initial), final_sample);
        assert_eq!(samples.len(), 1);
        assert_eq!(observed_floor(Some(50), &samples), Some(100));
        assert_eq!(window.spent_max_ms(), 1001);
        assert!(
            window
                .assess(Some(TimeInterval::new(101, 102).unwrap()), None)
                .unwrap()
                == Assessment::Unknown
        );
        assert_eq!(observed_floor(Some(150), &samples), Some(150));
        let both = retain_samples(Some(observation(100, 110)), Some(observation(120, 130)));
        assert_eq!(both.len(), 2);
        assert_eq!(observed_floor(None, &both), Some(120));
        assert!(retain_samples(None, None).is_empty());
    }
}
