//! Product Root genesis only. No current Root promotion or acceptance right.
use super::*;
use pinvou_upgrade_codec::{InputKind, decode_canonical_envelope};
use pinvou_upgrade_metadata_owner::{BootstrapAnchor, BootstrapBinding};
use serde::Serialize;

const NAMESPACE: &str = "root-state-v1";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Genesis<'a> {
    state_version: u8,
    revision: u64,
    product: &'a str,
    profile_kind: &'a str,
    original_root_sha256: &'a str,
    original_root_size: u64,
    original_root_version: u64,
    first_enrollment_sha256: String,
}

/// The only expected genesis is derived from the actual protected original
/// Root and the first protected enrollment, including real self-threshold
/// signatures. Signed issuance time here is not a trusted current clock.
fn genesis(first_product_event: &[u8], root: &[u8]) -> Result<Vec<u8>> {
    let local = ProductEvent::parse(first_product_event)?;
    if local.revision != 1 || local.previous_sha256.is_some() {
        return Err(Error::StateCorrupt);
    }
    let event = Event::parse(local.enrollment.as_bytes())?;
    let binding = BootstrapBinding::new(
        &event.scope,
        &event.product,
        &event.component,
        &event.target_key,
        &event.kind,
        &event.profile_id,
        &event.seed_sha256,
    )
    .map_err(|_| Error::StateCorrupt)?;
    BootstrapAnchor::new(
        &binding,
        root,
        event.original_root_size,
        &event.original_root_sha256,
    )
    .map_err(|_| Error::StateCorrupt)?;
    let checked = decode_canonical_envelope(root, InputKind::Metadata)
        .map_err(|_| Error::StateCorrupt)?
        .check_full_schema()
        .map_err(|_| Error::StateCorrupt)?;
    let policy = checked
        .envelope()
        .signed()
        .root_policy_shape()
        .map_err(|_| Error::StateCorrupt)?;
    if policy.product() != event.product {
        return Err(Error::StateCorrupt);
    }
    json::encode(
        &Genesis {
            state_version: 1,
            revision: 1,
            product: &event.product,
            profile_kind: &event.kind,
            original_root_sha256: &event.original_root_sha256,
            original_root_size: event.original_root_size,
            original_root_version: policy.version(),
            first_enrollment_sha256: json::sha256(first_product_event),
        },
        false,
    )
}

/// Called only for product absence proven by the original installation TxR.
pub(super) fn initialize_in(
    product: &Key,
    tx: &Handle,
    first_event: &[u8],
    root: &[u8],
) -> Result<()> {
    let raw = genesis(first_event, root)?;
    let key = create(product.0, NAMESPACE, tx, false, true)?;
    let history = create(key.0, "history", tx, false, true)?;
    let first = create(history.0, &revision_name(1), tx, false, true)?;
    write(&key, "Genesis", &raw)?;
    write(&key, "Head", &raw)?;
    write(&first, "Record", &raw)?;
    check_in(product, Some(tx), first_event, root)
}

/// Fixed genesis read only. Unknown, missing and progressed state refuse; a
/// future Root writer must replace this with authenticated history/current read.
pub(super) fn check_in(
    product: &Key,
    tx: Option<&Handle>,
    first_event: &[u8],
    root: &[u8],
) -> Result<()> {
    let raw = genesis(first_event, root)?;
    let key = entry(product, NAMESPACE, tx)?;
    exact(&key, true, &["Genesis", "Head"])?;
    exact(&key, false, &["history"])?;
    let history = entry(&key, "history", tx)?;
    exact(&history, true, &[])?;
    exact(&history, false, &["0000000000000001"])?;
    let first = entry(&history, &revision_name(1), tx)?;
    exact(&first, true, &["Record"])?;
    exact(&first, false, &[])?;
    if read(&key, "Genesis")? != raw || read(&first, "Record")? != raw {
        return Err(Error::StateCorrupt);
    }
    if read(&key, "Head")? != raw {
        return Err(Error::ScopeConflict);
    }
    Ok(())
}

#[allow(unsafe_code)]
pub(super) fn flush(product: &Key) -> Result<()> {
    let key = entry(product, NAMESPACE, None)?;
    let history = entry(&key, "history", None)?;
    let first = entry(&history, &revision_name(1), None)?;
    for item in [&key, &history, &first] {
        if unsafe { RegFlushKey(item.0) } != 0 {
            return Err(Error::StateCommitUnknown);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const ROOT: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/metadata-component-fixture/root.json"
    ));
    fn event() -> Event {
        Event {
            version: 1,
            kind: "lab".into(),
            revision: 1,
            previous_sha256: None,
            product: "pinvou".into(),
            original_root_sha256: json::sha256(ROOT),
            original_root_size: ROOT.len() as u64,
            profile_id: "metadata-component-test".into(),
            scope: "1".repeat(64),
            component: "app".into(),
            target_key: "windows-x86_64-nsis-perMachine".into(),
            seed_sha256: "2".repeat(64),
        }
    }
    #[test]
    fn original_genesis_binds_valid_signed_root_and_first_enrollment() {
        let enrollment = json::encode(&event(), false).unwrap();
        let first = ProductEvent::next(None, &enrollment).unwrap();
        let raw = json::encode(&first, false).unwrap();
        let actual = genesis(&raw, ROOT).unwrap();
        let decoded: serde_json::Value = json::decode(&actual, false).unwrap();
        assert_eq!(decoded["product"], "pinvou");
        assert_eq!(decoded["revision"], 1);
        assert_eq!(decoded["originalRootVersion"], 1);
        assert_eq!(decoded["originalRootSha256"], json::sha256(ROOT));
        assert_eq!(decoded["firstEnrollmentSha256"], json::sha256(&raw));
        let mut altered_root = ROOT.to_vec();
        altered_root.push(b' ');
        assert!(genesis(&raw, &altered_root).is_err());
        let mut wrong = event();
        wrong.product = "other".into();
        let wrong = json::encode(
            &ProductEvent::next(None, &json::encode(&wrong, false).unwrap()).unwrap(),
            false,
        )
        .unwrap();
        assert!(genesis(&wrong, ROOT).is_err());
        let mut changed_binding = event();
        changed_binding.scope = "3".repeat(64);
        let rebound = json::encode(
            &ProductEvent::next(None, &json::encode(&changed_binding, false).unwrap()).unwrap(),
            false,
        )
        .unwrap();
        assert_ne!(genesis(&rebound, ROOT).unwrap(), actual);
        let mut tampered: serde_json::Value = json::decode(ROOT, false).unwrap();
        let signature = tampered["signatures"][0]["signature"].as_str().unwrap();
        let first_byte = if signature.starts_with('A') { "B" } else { "A" };
        tampered["signatures"][0]["signature"] =
            format!("{}{}", first_byte, &signature[1..]).into();
        let tampered = json::encode(&tampered, false).unwrap();
        let mut changed_root = event();
        changed_root.original_root_sha256 = json::sha256(&tampered);
        changed_root.original_root_size = tampered.len() as u64;
        let changed_event = json::encode(
            &ProductEvent::next(None, &json::encode(&changed_root, false).unwrap()).unwrap(),
            false,
        )
        .unwrap();
        assert!(genesis(&changed_event, &tampered).is_err());
        let second = ProductEvent::next(Some((&first, &raw)), &enrollment).unwrap();
        assert!(genesis(&json::encode(&second, false).unwrap(), ROOT).is_err());
    }
}
