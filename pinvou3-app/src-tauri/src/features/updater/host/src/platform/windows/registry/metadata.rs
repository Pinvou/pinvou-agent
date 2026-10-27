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
}
