//! Fixed TxR journal. No process/job create, assignment, grant or release here.
use super::*;
use pinvou_upgrade_ownership::live::OperationStatus;

pub(super) struct Loaded {
    pub(super) genesis: record::Genesis,
    pub(super) head: record::Head,
    pub(super) head_raw: Vec<u8>,
    pub(super) active: Vec<(record::Row, Vec<u8>)>,
    pub(super) pending: Vec<(record::Row, Vec<u8>)>,
}
pub(super) enum Change {
    Reserve(record::Row),
    Admit,
    Drain,
    Close,
    Release,
}
fn exact(key: &Key, values: bool, expected: &[&str]) -> Result<()> {
    let mut actual = names(key, values, expected.len() as u32)?;
    actual.sort();
    let mut want: Vec<_> = expected.iter().map(|v| v.to_string()).collect();
    want.sort();
    if actual != want {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}
fn namespace(private: &Key, write_access: bool, tx: Option<&Handle>) -> Result<Key> {
    open(private.0, NAMESPACE, write_access, tx)?.ok_or(Error::SetupIncomplete)
}
fn genesis(
    private: &Key,
    key: &Key,
    host: &HostRegistration,
) -> Result<(record::Genesis, Vec<u8>)> {
    let immutable = read(private, PREPARATION_BOOTSTRAP)?;
    let expected = record::Genesis::new(host, &immutable)?;
    let raw = read(key, "Genesis")?;
    let parsed: record::Genesis = record::decode(&raw)?;
    if parsed != expected || record::encode(&parsed)? != raw {
        return Err(Error::StateCorrupt);
    }
    Ok((parsed, raw))
}
fn row_version(
    rows: &Key,
    entry: &record::Entry,
    g: &record::Genesis,
    tx: Option<&Handle>,
) -> Result<(record::Row, Vec<u8>)> {
    let revision = open(
        rows.0,
        &format!("{}\\history\\{}", entry.id, revision_name(entry.revision)),
        false,
        tx,
    )?
    .ok_or(Error::StateCorrupt)?;
    exact(&revision, true, &["Record"])?;
    exact(&revision, false, &[])?;
    let raw = read(&revision, "Record")?;
    let row: record::Row = record::decode(&raw)?;
    row.validate(g)?;
    if row.entry(&raw) != *entry {
        return Err(Error::StateCorrupt);
    }
    Ok((row, raw))
}
fn current_row(
    rows: &Key,
    entry: &record::Entry,
    g: &record::Genesis,
    tx: Option<&Handle>,
) -> Result<(record::Row, Vec<u8>)> {
    let row_key = open(rows.0, &entry.id, false, tx)?.ok_or(Error::StateCorrupt)?;
    exact(&row_key, true, &["Head"])?;
    exact(&row_key, false, &["history"])?;
    let raw = read(&row_key, "Head")?;
    let (row, historic) = row_version(rows, entry, g, tx)?;
    if raw != historic {
        return Err(Error::StateCorrupt);
    }
    let first_key = open(
        row_key.0,
        &format!("history\\{}", revision_name(1)),
        false,
        tx,
    )?
    .ok_or(Error::StateCorrupt)?;
    exact(&first_key, true, &["Record"])?;
    exact(&first_key, false, &[])?;
    let first_raw = read(&first_key, "Record")?;
    let first: record::Row = record::decode(&first_raw)?;
    first.validate(g)?;
    if first.id != row.id
        || first.identity != row.identity
        || first.revision != 1
        || first.phase != record::Phase::Reserved
        || (row.revision == 1 && raw != first_raw)
        || (row.revision > 1 && row.first_record_sha256 != Some(json::sha256(&first_raw)))
    {
        return Err(Error::StateCorrupt);
    }
    if row.revision > 1 {
        // Constant read cost: current + adjacent predecessor + immutable first.
        // Native protected producers validate every transition before publish;
        // retained older versions are individually addressable, never rescanned.
        let prior_entry = record::Entry {
            id: row.id.clone(),
            revision: row.revision - 1,
            sha256: row.previous_sha256.clone().ok_or(Error::StateCorrupt)?,
        };
        let (prior, previous_raw) = row_version(rows, &prior_entry, g, tx)?;
        if prior.identity != first.identity
            || (prior.revision > 1 && prior.first_record_sha256 != Some(json::sha256(&first_raw)))
        {
            return Err(Error::StateCorrupt);
        }
        prior.successor(&row, &previous_raw)?;
    }
    Ok((row, raw))
}
/// Bounded exact-key observation, including historical rows no longer indexed.
/// Only a successful native open returning None proves absence; corrupt or
/// inaccessible keys propagate an error, and existing rows use normal history
/// validation. No enumeration or lifetime history scan is needed.
pub(super) fn lookup_row(
    private: &Key,
    host: &HostRegistration,
    id: &str,
) -> Result<Option<(record::Row, Vec<u8>)>> {
    if !json::digest(id) {
        return Err(Error::StateCorrupt);
    }
    let loaded = load(private, host, None)?;
    let key = namespace(private, false, None)?;
    let rows = open(key.0, "rows", false, None)?.ok_or(Error::StateCorrupt)?;
    let Some(row_key) = open(rows.0, id, false, None)? else {
        return Ok(None);
    };
    let raw = read(&row_key, "Head")?;
    let row: record::Row = record::decode(&raw)?;
    if row.id != id {
        return Err(Error::StateCorrupt);
    }
    current_row(&rows, &row.entry(&raw), &loaded.genesis, None).map(Some)
}
#[allow(unsafe_code)]
pub(super) fn confirm_closed(
    private: &Key,
    host: &HostRegistration,
    reference: &WriterReference,
) -> Result<(record::Row, Vec<u8>)> {
    let loaded = load(private, host, None)?;
    if loaded.genesis.scope != reference.scope
        || loaded.genesis.genesis != reference.genesis
        || loaded
            .head
            .active
            .iter()
            .any(|entry| entry.id == reference.entry.id)
    {
        return Err(Error::ScopeConflict);
    }
    let key = namespace(private, false, None)?;
    let rows = open(key.0, "rows", false, None)?.ok_or(Error::StateCorrupt)?;
    let row_key =
        open(rows.0, &reference.entry.id, false, None)?.ok_or(Error::PrerequisiteUnavailable)?;
    let raw = read(&row_key, "Head")?;
    let value: record::Row = record::decode(&raw)?;
    let (row, exact) = current_row(&rows, &value.entry(&raw), &loaded.genesis, None)?;
    let (original, _) = row_version(&rows, &reference.entry, &loaded.genesis, None)?;
    let selected_first = if original.revision == 1 {
        Some(reference.entry.sha256.clone())
    } else {
        original.first_record_sha256.clone()
    };
    if row.phase != record::Phase::Closed
        || row.identity != original.identity
        || row.first_record_sha256 != selected_first
        || row.identity.origin_host_epoch > host.host_epoch
        || host.service_identity.service_sid.as_deref() != Some(row.identity.service_sid.as_str())
    {
        return Err(Error::ScopeConflict);
    }
    let version = open(
        row_key.0,
        &format!("history\\{}", revision_name(row.revision)),
        false,
        None,
    )?
    .ok_or(Error::StateCorrupt)?;
    for handle in [private.0, key.0, row_key.0, version.0] {
        if unsafe { RegFlushKey(handle) } != 0 {
            return Err(Error::StateCommitUnknown);
        }
    }
    if read(&row_key, "Head")? != exact || read(&version, "Record")? != exact {
        return Err(Error::StateCommitUnknown);
    }
    Ok((row, exact))
}
pub(super) fn load(private: &Key, host: &HostRegistration, tx: Option<&Handle>) -> Result<Loaded> {
    let key = namespace(private, false, tx)?;
    exact(&key, true, &["Genesis", "Head"])?;
    exact(&key, false, &["history", "rows"])?;
    let (genesis, genesis_raw) = genesis(private, &key, host)?;
    let head_raw = read(&key, "Head")?;
    let head: record::Head = record::decode(&head_raw)?;
    head.validate(&genesis_raw)?;
    let history = open(key.0, "history", false, tx)?.ok_or(Error::StateCorrupt)?;
    let row =
        open(history.0, &revision_name(head.revision), false, tx)?.ok_or(Error::StateCorrupt)?;
    if read(&row, "Head")? != head_raw {
        return Err(Error::StateCorrupt);
    }
    let rows = open(key.0, "rows", false, tx)?.ok_or(Error::StateCorrupt)?;
    if head.revision > 1 {
        let previous = open(history.0, &revision_name(head.revision - 1), false, tx)?
            .ok_or(Error::StateCorrupt)?;
        let previous_raw = read(&previous, "Head")?;
        let prior: record::Head = record::decode(&previous_raw)?;
        prior.validate(&genesis_raw)?;
        let changed = head.changed.as_ref().ok_or(Error::StateCorrupt)?;
        let (changed_row, changed_raw) = current_row(&rows, changed, &genesis, tx)?;
        if changed_row.identity.origin_host_epoch > host.host_epoch
            || host.service_identity.service_sid.as_deref()
                != Some(changed_row.identity.service_sid.as_str())
        {
            return Err(Error::StateCorrupt);
        }
        prior.successor(&head, &previous_raw, &changed_row, &changed_raw)?;
    }
    let mut active = Vec::with_capacity(head.active.len());
    for entry in &head.active {
        let row = current_row(&rows, entry, &genesis, tx)?;
        if row.0.phase == record::Phase::Closed
            || row.0.identity.origin_host_epoch > host.host_epoch
            || host.service_identity.service_sid.as_deref()
                != Some(row.0.identity.service_sid.as_str())
        {
            return Err(Error::StateCorrupt);
        }
        active.push(row);
    }
    let mut pending = Vec::with_capacity(head.pending_release.len());
    for entry in &head.pending_release {
        let row = current_row(&rows, entry, &genesis, tx)?;
        if row.0.phase != record::Phase::Closed
            || row.0.identity.origin_host_epoch > host.host_epoch
            || host.service_identity.service_sid.as_deref()
                != Some(row.0.identity.service_sid.as_str())
        {
            return Err(Error::StateCorrupt);
        }
        pending.push(row);
    }
    Ok(Loaded {
        genesis,
        head,
        head_raw,
        active,
        pending,
    })
}
pub(super) fn init_in(private: &Key, tx: &Handle, host: &HostRegistration) -> Result<()> {
    // ONLY fresh original Store::commit(None). This never runs from load,
    // repair, retry or recovery. Existing namespace/orphan is rejected.
    let immutable = read(private, PREPARATION_BOOTSTRAP)?;
    let genesis = record::Genesis::new(host, &immutable)?;
    let genesis_raw = record::encode(&genesis)?;
    let head_raw = record::encode(&record::Head::initial(&genesis_raw))?;
    let key = create(private.0, NAMESPACE, tx, false, true)?;
    let history = create(key.0, "history", tx, false, true)?;
    create(key.0, "rows", tx, false, true)?;
    let initial = create(history.0, &revision_name(1), tx, false, true)?;
    write(&key, "Genesis", &genesis_raw)?;
    write(&key, "Head", &head_raw)?;
    write(&initial, "Head", &head_raw)?;
    if load(private, host, Some(tx))?.head_raw != head_raw {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}
fn drain_in(
    private: &Key,
    host: &HostRegistration,
    tx: &Handle,
    row: &record::Row,
) -> Result<record::Drain> {
    // This reader is root's real native preparation+original budget read set,
    // not a caller-provided occupied flag or a separately assessed budget.
    let preparation =
        preparation::current_in(private, host, Some(tx))?.ok_or(Error::ScopeConflict)?;
    let operation = preparation.operation();
    if !matches!(operation.status(), OperationStatus::Active) {
        return Err(Error::ScopeConflict);
    }
    let source = operation.preparation();
    let token = preparation.token();
    let participant = row.identity.participant(&row.id)?;
    if !source.inventory().participants.contains(&participant) {
        return Err(Error::ScopeConflict);
    }
    Ok(record::Drain {
        identity: source.identity().clone(),
        owner_epoch: token.owner_epoch,
        freeze_epoch: token.freeze_epoch.ok_or(Error::ScopeConflict)?,
        inventory_revision: source.inventory().revision,
        inventory_sha256: source.inventory().sha256.clone(),
        participant,
        budget_origin: source.budget().origin.clone(),
    })
}

/// Caller holds the original BootstrapGuard. Native custody is checked by the
/// sealed parent token before entry. TxR contains registry effects only.
#[allow(unsafe_code)]
pub(super) fn commit(
    store: &Store,
    host: &StartedServiceContext,
    expected_head: &[u8],
    expected_row: Option<&[u8]>,
    change: Change,
) -> Result<(Loaded, record::Row, Vec<u8>)> {
    let seed = &host.context.seed;
    let tx = Handle::owned(unsafe {
        CreateTransaction(ptr::null_mut(), ptr::null_mut(), 0, 0, 0, 0, ptr::null())
    })?;
    let (wanted_head, wanted_row, row_id) = {
        let actual = store
            .read_in(seed, Some(&tx))?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&actual)?;
        let private = store.key(true, Some(&tx))?.ok_or(Error::SetupIncomplete)?;
        let current = load(&private, &actual, Some(&tx))?;
        if current.head_raw != expected_head {
            return Err(Error::ScopeConflict);
        }
        let releasing = matches!(&change, Change::Release);
        let old = match expected_row {
            None => None,
            Some(raw) => Some(
                current
                    .active
                    .iter()
                    .chain(current.pending.iter())
                    .find(|(_, bytes)| bytes == raw)
                    .ok_or(Error::ScopeConflict)?,
            ),
        };
        let next = match change {
            Change::Reserve(row) => {
                keeper::require_active_record_in(&private, &actual, Some(&tx))?;
                if old.is_some()
                    || row.revision != 1
                    || row.phase != record::Phase::Reserved
                    || row.identity.origin_host_epoch != actual.host_epoch
                    || actual.service_identity.service_sid.as_deref()
                        != Some(row.identity.service_sid.as_str())
                    || current
                        .active
                        .iter()
                        .chain(current.pending.iter())
                        .any(|(r, _)| r.identity.original == row.identity.original)
                {
                    return Err(Error::ScopeConflict);
                }
                if preparation::current_in(&private, &actual, Some(&tx))?.is_some() {
                    return Err(Error::Busy);
                }
                row
            }
            Change::Admit => {
                keeper::require_active_record_in(&private, &actual, Some(&tx))?;
                let (row, raw) = old.ok_or(Error::ScopeConflict)?;
                if preparation::current_in(&private, &actual, Some(&tx))?.is_some() {
                    return Err(Error::Busy);
                }
                row.advance(raw, record::Phase::Admitted, None)?
            }
            Change::Drain => {
                let (row, raw) = old.ok_or(Error::ScopeConflict)?;
                let next =
                    row.draining_for_observation(raw, drain_in(&private, &actual, &tx, row)?)?;
                if next == *row {
                    // Same epochs/binding after an ACK or budget observation
                    // head revision: no fake lifecycle revision or commit.
                    let unchanged = row.clone();
                    let unchanged_raw = raw.clone();
                    return Ok((current, unchanged, unchanged_raw));
                }
                next
            }
            Change::Close => {
                let (row, raw) = old.ok_or(Error::ScopeConflict)?;
                let drain = match row.phase {
                    record::Phase::Reserved => {
                        if preparation::current_in(&private, &actual, Some(&tx))?.is_some() {
                            Some(drain_in(&private, &actual, &tx, row)?)
                        } else {
                            None
                        }
                    }
                    record::Phase::Draining => {
                        let observed = drain_in(&private, &actual, &tx, row)?;
                        row.require_current_drain(&observed)?;
                        Some(observed)
                    }
                    record::Phase::Admitted => {
                        // Ordinary completed process trees must retire without
                        // requiring an unrelated T28 operation. Native zero is
                        // mandatory at the sealed caller; an active preparation
                        // instead requires its exact prior Draining binding.
                        if preparation::current_in(&private, &actual, Some(&tx))?.is_some() {
                            return Err(Error::ScopeConflict);
                        }
                        None
                    }
                    _ => return Err(Error::ScopeConflict),
                };
                row.advance(raw, record::Phase::Closed, drain)?
            }
            Change::Release => {
                let (row, _) = old.ok_or(Error::ScopeConflict)?;
                if row.phase != record::Phase::Closed {
                    return Err(Error::ScopeConflict);
                }
                row.clone()
            }
        };
        next.validate(&current.genesis)?;
        let raw = record::encode(&next)?;
        let head = if releasing {
            current.head.release(&current.head_raw, &next.entry(&raw))?
        } else {
            current.head.advance(&current.head_raw, &next, &raw)?
        };
        let head_raw = record::encode(&head)?;
        let key = namespace(&private, true, Some(&tx))?;
        let rows = open(key.0, "rows", true, Some(&tx))?.ok_or(Error::StateCorrupt)?;
        let row = if next.revision == 1 {
            create(rows.0, &next.id, &tx, false, true)?
        } else {
            open(rows.0, &next.id, true, Some(&tx))?.ok_or(Error::StateCorrupt)?
        };
        let versions = if next.revision == 1 {
            create(row.0, "history", &tx, false, true)?
        } else {
            open(row.0, "history", true, Some(&tx))?.ok_or(Error::StateCorrupt)?
        };
        if !releasing {
            let version = create(versions.0, &revision_name(next.revision), &tx, false, true)?;
            write(&version, "Record", &raw)?;
            write(&row, "Head", &raw)?;
        }
        let history = open(key.0, "history", true, Some(&tx))?.ok_or(Error::StateCorrupt)?;
        let revision = create(history.0, &revision_name(head.revision), &tx, false, true)?;
        write(&revision, "Head", &head_raw)?;
        write(&key, "Head", &head_raw)?;
        if load(&private, &actual, Some(&tx))?.head_raw != head_raw {
            return Err(Error::StateCorrupt);
        }
        (head_raw, raw, next.id)
        // Every transacted key drops before definitive CommitTransaction.
    };
    if unsafe { CommitTransaction(tx.raw()) } == 0 {
        return Err(Error::StateCommitUnknown);
    }
    drop(tx);
    let verify = || -> Result<(Loaded, record::Row, Vec<u8>)> {
        let actual = store.read(seed)?.ok_or(Error::StateCommitUnknown)?;
        host.authenticate_running(&actual)?;
        let private = store.key(false, None)?.ok_or(Error::StateCommitUnknown)?;
        let key = namespace(&private, false, None)?;
        let row = open(key.0, &format!("rows\\{row_id}"), false, None)?
            .ok_or(Error::StateCommitUnknown)?;
        let head: record::Head = record::decode(&wanted_head)?;
        let row_value: record::Row = record::decode(&wanted_row)?;
        let head_version = open(
            key.0,
            &format!("history\\{}", revision_name(head.revision)),
            false,
            None,
        )?
        .ok_or(Error::StateCommitUnknown)?;
        let row_version = open(
            row.0,
            &format!("history\\{}", revision_name(row_value.revision)),
            false,
            None,
        )?
        .ok_or(Error::StateCommitUnknown)?;
        for handle in [private.0, key.0, row.0, head_version.0, row_version.0] {
            if unsafe { RegFlushKey(handle) } != 0 {
                return Err(Error::StateCommitUnknown);
            }
        }
        let loaded = load(&private, &actual, None)?;
        if loaded.head_raw != wanted_head
            || read(&row, "Head")? != wanted_row
            || read(&head_version, "Head")? != wanted_head
            || read(&row_version, "Record")? != wanted_row
        {
            return Err(Error::StateCommitUnknown);
        }
        Ok((loaded, row_value, wanted_row.clone()))
    };
    verify().map_err(|_| Error::StateCommitUnknown)
}
