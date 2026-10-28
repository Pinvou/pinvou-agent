//! First-install Root watermark only. There is no metadata acceptance writer.
use super::*;
use serde::Serialize;

const NAMESPACE: &str = "metadata-state-v1";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Genesis<'a> {
    state_version: u8,
    revision: u8,
    // This exact anchor includes the bound scope and original Root's size,
    // envelope hash and version. It is the initial Root high watermark.
    root_anchor: &'a str,
}

fn genesis(anchor: &[u8]) -> Result<Vec<u8>> {
    let root_anchor = std::str::from_utf8(anchor).map_err(|_| Error::StateCorrupt)?;
    json::encode(
        &Genesis {
            state_version: 1,
            revision: 1,
            root_anchor,
        },
        false,
    )
}

fn check_bytes(expected: &[u8], genesis: &[u8], head: &[u8], history: &[u8]) -> Result<()> {
    if genesis != expected || head != expected || history != expected {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}

fn exact_names(actual: &[String], expected: &[&str]) -> Result<()> {
    if actual.len() != expected.len()
        || expected
            .iter()
            .any(|name| !actual.iter().any(|found| found == name))
    {
        return Err(Error::ScopeConflict);
    }
    Ok(())
}

/// Prove the current owner has only its original empty genesis. This reads the
/// actual protected namespace in the Root publisher's held transaction; an
/// absent or unknown layout is never interpreted as an empty active set.
/// Once an accepting metadata owner exists this gate must be replaced by a
/// same-transaction complete active-set and Root-transition check.
pub(super) fn require_empty_for_root_transition_in(
    private: &Key,
    seed: &Seed,
    host: &HostRegistration,
    tx: &Handle,
) -> Result<()> {
    if !crate::bootstrap::initialized(&host.known_owner_namespaces) {
        return Err(Error::SetupIncomplete);
    }
    let expected = OwnerBootstrap::new(seed, &host.installation_scope_id)?;
    let initial = genesis(expected.metadata.record_bytes())?;
    let key = open(private.0, NAMESPACE, false, Some(tx))?.ok_or(Error::SetupIncomplete)?;
    let first =
        open(key.0, "history\\0000000000000001", false, Some(tx))?.ok_or(Error::StateCorrupt)?;
    if read(&key, "Genesis")? != initial || read(&first, "Record")? != initial {
        return Err(Error::StateCorrupt);
    }
    // A future accepting owner may advance Head. That alone is not proof of
    // damage, but cannot prove an empty active set for this gate.
    if read(&key, "Head")? != initial {
        return Err(Error::ScopeConflict);
    }
    exact_names(&names(&key, true, 2)?, &["Genesis", "Head"])?;
    exact_names(&names(&key, false, 1)?, &["history"])?;
    let history = open(key.0, "history", false, Some(tx))?.ok_or(Error::StateCorrupt)?;
    exact_names(&names(&history, true, 0)?, &[])?;
    exact_names(&names(&history, false, 1)?, &["0000000000000001"])?;
    exact_names(&names(&first, true, 1)?, &["Record"])?;
    exact_names(&names(&first, false, 0)?, &[])
}

pub(super) fn check_in(
    private: &Key,
    seed: &Seed,
    host: &HostRegistration,
    tx: Option<&Handle>,
) -> Result<()> {
    // Legacy and damaged installs cannot be distinguished here; neither may
    // be interpreted as a new empty metadata store.
    if !crate::bootstrap::initialized(&host.known_owner_namespaces) {
        return Err(Error::SetupIncomplete);
    }
    let expected = OwnerBootstrap::new(seed, &host.installation_scope_id)?;
    let raw = genesis(expected.metadata.record_bytes())?;
    let key = open(private.0, NAMESPACE, false, tx)?.ok_or(Error::SetupIncomplete)?;
    let first = open(key.0, "history\\0000000000000001", false, tx)?.ok_or(Error::StateCorrupt)?;
    check_bytes(
        &raw,
        &read(&key, "Genesis")?,
        &read(&key, "Head")?,
        &read(&first, "Record")?,
    )
}

pub(super) fn initialize_in(
    private: &Key,
    tx: &Handle,
    seed: &Seed,
    host: &HostRegistration,
) -> Result<()> {
    let anchor = OwnerBootstrap::new(seed, &host.installation_scope_id)?;
    let raw = genesis(anchor.metadata.record_bytes())?;
    let key = create(private.0, NAMESPACE, tx, false, true)?;
    let history = create(key.0, "history", tx, false, true)?;
    let first = create(history.0, &revision_name(1), tx, false, true)?;
    write(&key, "Genesis", &raw)?;
    write(&key, "Head", &raw)?;
    write(&first, "Record", &raw)?;
    check_in(private, seed, host, Some(tx))
}

#[allow(unsafe_code)]
pub(super) fn verify_fresh_in(private: &Key, seed: &Seed, host: &HostRegistration) -> Result<()> {
    let key = open(private.0, NAMESPACE, false, None)?.ok_or(Error::StateCommitUnknown)?;
    let first =
        open(key.0, "history\\0000000000000001", false, None)?.ok_or(Error::StateCommitUnknown)?;
    if unsafe { RegFlushKey(key.0) } != 0 || unsafe { RegFlushKey(first.0) } != 0 {
        return Err(Error::StateCommitUnknown);
    }
    check_in(private, seed, host, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_initial_state_is_accepted() {
        let a = genesis(br#"{"scope":"a","version":1}"#).unwrap();
        let b = genesis(br#"{"scope":"b","version":1}"#).unwrap();
        assert!(check_bytes(&a, &a, &a, &a).is_ok());
        assert!(check_bytes(&a, &b, &a, &a).is_err());
        assert!(check_bytes(&a, &a, &b, &a).is_err());
        assert!(check_bytes(&a, &a, &a, &[]).is_err());
    }
    #[test]
    fn namespace_inventory_requires_exact_names() {
        assert!(exact_names(&["Head".into(), "Genesis".into()], &["Genesis", "Head"]).is_ok());
        assert!(exact_names(&["Genesis".into()], &["Genesis", "Head"]).is_err());
        assert!(
            exact_names(
                &["Genesis".into(), "Head".into(), "extra".into()],
                &["Genesis", "Head"]
            )
            .is_err()
        );
        assert!(exact_names(&["Genesis".into(), "Genesis".into()], &["Genesis", "Head"]).is_err());
        assert!(exact_names(&["head".into()], &["Head"]).is_err());
        assert!(exact_names(&[], &[]).is_ok());
    }
}
