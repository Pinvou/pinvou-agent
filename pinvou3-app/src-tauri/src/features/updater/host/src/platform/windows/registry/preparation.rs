//! Protected preparation read set. Initialization belongs only to the original
//! registration transaction; a missing namespace never means an idle scope.
//! These reads describe occupancy and original budget identity. They do not
//! authenticate a remote Session, establish saved data, or grant a freeze.
use super::*;
use crate::{
    time_budget::{WindowId, WindowKind},
    trusted_time::Binding,
};
use pinvou_upgrade_ownership::{
    BootstrapBinding, InitialOwnership,
    live::{LIVE_NAMESPACE, OperationRecord, OperationStatus, ScopeHead, Token},
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HeadRecord {
    head: ScopeHead,
    previous_sha256: Option<String>,
    operation_sha256: Option<String>,
}

pub(super) fn binding_for(actual: &HostRegistration) -> Result<BootstrapBinding> {
    BootstrapBinding::new(
        &actual.installation_scope_id,
        &actual.product,
        &actual.component,
        &actual.target_key,
        &actual.profile_kind,
        &actual.profile_id,
        &actual.genesis_seed_sha256,
    )
    .map_err(|_| Error::StateCorrupt)
}

fn initial(actual: &HostRegistration) -> Result<HeadRecord> {
    Ok(HeadRecord {
        head: ScopeHead::initial(&binding_for(actual)?).map_err(|_| Error::StateCorrupt)?,
        previous_sha256: None,
        operation_sha256: None,
    })
}

fn parse_head(raw: &[u8], actual: &HostRegistration) -> Result<HeadRecord> {
    let record: HeadRecord = json::decode(raw, false)?;
    let head = ScopeHead::parse(&json::encode(&record.head, false)?, &binding_for(actual)?)
        .map_err(|_| Error::StateCorrupt)?;
    if json::encode(&record, false)? != raw
        || (head.revision() == 0) != record.previous_sha256.is_none()
        || record
            .previous_sha256
            .as_ref()
            .is_some_and(|s| !json::digest(s))
        || head.current_operation_id().is_some() != record.operation_sha256.is_some()
        || record
            .operation_sha256
            .as_ref()
            .is_some_and(|s| !json::digest(s))
    {
        return Err(Error::StateCorrupt);
    }
    Ok(record)
}

fn read_head(key: &Key, actual: &HostRegistration, tx: Option<&Handle>) -> Result<HeadRecord> {
    let genesis = read(key, "Genesis")?;
    if genesis != json::encode(&initial(actual)?, false)? {
        return Err(Error::StateCorrupt);
    }
    let raw = read(key, "Head")?;
    let current = parse_head(&raw, actual)?;
    let revision = current.head.revision();
    let own = open(
        key.0,
        &format!("history\\{}", revision_name(revision)),
        false,
        tx,
    )?
    .ok_or(Error::StateCorrupt)?;
    if read(&own, "Record")? != raw {
        return Err(Error::StateCorrupt);
    }
    if revision == 0 {
        if raw != genesis {
            return Err(Error::StateCorrupt);
        }
    } else {
        let prior = open(
            key.0,
            &format!("history\\{}", revision_name(revision - 1)),
            false,
            tx,
        )?
        .ok_or(Error::StateCorrupt)?;
        let prior_raw = read(&prior, "Record")?;
        let before = parse_head(&prior_raw, actual)?;
        let owner_step = current
            .head
            .owner_epoch()
            .checked_sub(before.head.owner_epoch());
        let freeze_step = current
            .head
            .freeze_epoch()
            .checked_sub(before.head.freeze_epoch());
        if before.head.revision().checked_add(1) != Some(revision)
            || current.previous_sha256 != Some(json::sha256(&prior_raw))
            || !matches!(owner_step, Some(0 | 1))
            || !matches!(freeze_step, Some(0 | 1))
            || (freeze_step == Some(1) && owner_step != Some(1))
        {
            return Err(Error::StateCorrupt);
        }
    }
    Ok(current)
}

/// Read-only DATA whose fields came from the protected same-transaction keys.
/// Keeping this value does not retain the transaction, its mutex or a fence.
pub(super) struct NativePreparationRead {
    head: ScopeHead,
    operation: OperationRecord,
}
impl NativePreparationRead {
    pub(super) fn operation(&self) -> &OperationRecord {
        &self.operation
    }
    pub(super) fn token(&self) -> Token {
        Token {
            revision: self.head.revision(),
            owner_epoch: self.operation.preparation().owner_epoch(),
            freeze_epoch: self.operation.preparation().freeze_epoch(),
        }
    }
}

/// The native writer owner calls this while holding its actual operation mutex.
/// Neither absent keys nor arbitrary caller flags can manufacture idle state.
pub(super) fn current_in(
    private: &Key,
    actual: &HostRegistration,
    tx: Option<&Handle>,
) -> Result<Option<NativePreparationRead>> {
    let binding = binding_for(actual)?;
    InitialOwnership::parse(&read(private, PREPARATION_BOOTSTRAP)?, &binding)
        .map_err(|_| Error::StateCorrupt)?;
    let key = open(private.0, LIVE_NAMESPACE, false, tx)?.ok_or(Error::SetupIncomplete)?;
    let record = read_head(&key, actual, tx)?;
    let Some(operation_id) = record.head.current_operation_id() else {
        return Ok(None);
    };
    let id = WindowId::new(
        Binding::from_host(actual),
        operation_id.to_owned(),
        WindowKind::PrepFreeze,
    )?;
    // Canonical IDs can contain ':'. They are never used as registry paths.
    let window_key = budgets::window_key(&id)?;
    let operation_key =
        open(key.0, &format!("operations\\{window_key}"), false, tx)?.ok_or(Error::StateCorrupt)?;
    let raw = read(&operation_key, "Head")?;
    let operation =
        OperationRecord::parse(&raw, &binding, operation_id).map_err(|_| Error::StateCorrupt)?;
    let preparation = operation.preparation();
    if operation.status() != &OperationStatus::Active
        || Some(operation.revision()) != record.head.current_operation_revision()
        || record.operation_sha256 != Some(json::sha256(&raw))
        || preparation.owner_epoch() != record.head.owner_epoch()
        || preparation
            .freeze_epoch()
            .is_some_and(|epoch| epoch != record.head.freeze_epoch())
    {
        return Err(Error::StateCorrupt);
    }
    let history = open(
        operation_key.0,
        &format!("history\\{}", revision_name(operation.revision())),
        false,
        tx,
    )?
    .ok_or(Error::StateCorrupt)?;
    if read(&history, "Record")? != raw {
        return Err(Error::StateCorrupt);
    }
    let (window, revision) = budgets::preparation_window_in(private, actual, tx, operation_id)?
        .ok_or(Error::StateCorrupt)?;
    check_window_pair(&operation, &window, revision, &window_key)?;
    Ok(Some(NativePreparationRead {
        head: record.head,
        operation,
    }))
}

/// Validate only the historical protected pair. A successful comparison is
/// intentionally insufficient to authorize any future time-dependent action.
fn check_window_pair(
    operation: &OperationRecord,
    window: &crate::time_budget::BudgetWindow,
    revision: u64,
    window_key: &str,
) -> Result<()> {
    let preparation = operation.preparation();
    let observed = preparation.budget();
    if window.id().kind() != WindowKind::PrepFreeze
        || window.id().operation_id() != operation.operation_id()
        || budgets::window_key(window.id())? != window_key
        || observed.window_revision != revision
        || observed.origin.window_key_sha256 != window_key
        || observed.origin.first_lower_ms != window.origin().lower_ms()
        || observed.origin.first_upper_ms != window.origin().upper_ms()
        || observed.origin.original_duration_ms != window.original_duration_ms()
        || observed.origin.original_deadline_ms != window.original_deadline_ms()
        || observed.session_cutoff_ms != window.session_cutoff_ms().ok_or(Error::StateCorrupt)?
        || observed.spent_max_ms != window.spent_max_ms()
    {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}

/// Called exclusively by Store::commit(None), inside the original scope TxR.
/// There is deliberately no load-time migration or missing-key initializer.
pub(super) fn initialize_in(private: &Key, tx: &Handle, actual: &HostRegistration) -> Result<()> {
    let binding = binding_for(actual)?;
    InitialOwnership::parse(&read(private, PREPARATION_BOOTSTRAP)?, &binding)
        .map_err(|_| Error::StateCorrupt)?;
    let raw = json::encode(&initial(actual)?, false)?;
    let key = create(private.0, LIVE_NAMESPACE, tx, false, true)?;
    let history = create(key.0, "history", tx, false, true)?;
    let first = create(history.0, &revision_name(0), tx, false, true)?;
    create(key.0, "operations", tx, false, true)?;
    write(&key, "Genesis", &raw)?;
    write(&key, "Head", &raw)?;
    write(&first, "Record", &raw)?;
    if read_head(&key, actual, Some(tx))?.head.revision() != 0 {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}

pub(super) fn verify_fresh_in(private: &Key, actual: &HostRegistration) -> Result<()> {
    let key = open(private.0, LIVE_NAMESPACE, false, None)?.ok_or(Error::StateCommitUnknown)?;
    let operations = open(key.0, "operations", false, None)?.ok_or(Error::StateCommitUnknown)?;
    let head = read_head(&key, actual, None)?;
    if head.head.revision() != 0
        || head.head.current_operation_id().is_some()
        || !names(&operations, false, 0)?.is_empty()
        || !names(&operations, true, 0)?.is_empty()
    {
        return Err(Error::StateCommitUnknown);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
