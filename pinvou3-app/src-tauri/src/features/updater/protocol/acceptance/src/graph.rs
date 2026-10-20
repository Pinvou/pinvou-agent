use crate::{Bundle, Error, document, platform::qa_sqlite as sql, state};
use pinvou_upgrade_codec::{
    Document, InputKind, RootPolicyShape, SchemaCheckedEnvelope, SuppliedProtocolTime,
    assert_supplied_public_graph_source, decode_canonical_envelope, derive_metadata_reference,
};
use pinvou_upgrade_verifier::{
    ActiveMetadataInput, VerificationError, VerificationInputs, verify_root_chain_source,
    verify_with_supplied_root,
};
use rusqlite::{Connection, OptionalExtension};

pub(crate) fn shape(bytes: &[u8]) -> Result<SchemaCheckedEnvelope, Error> {
    decode_canonical_envelope(bytes, InputKind::Metadata)
        .map_err(|_| Error::Verification)?
        .check_full_schema()
        .map_err(|_| Error::Verification)
}
pub(crate) fn root(db: &Connection, hash: &str) -> Result<RootPolicyShape, Error> {
    shape(&sql::original(db, hash)?)?
        .envelope()
        .signed()
        .root_policy_shape()
        .map_err(|_| Error::InvalidStore)
}
pub(crate) fn authenticate(
    db: &Connection,
    hash: &str,
    root: &RootPolicyShape,
    now: &SuppliedProtocolTime,
    bundle: &Bundle,
) -> Result<SchemaCheckedEnvelope, Error> {
    let bytes = sql::original(db, hash)?;
    let envelope = shape(&bytes)?;
    let expected = state::expected(&envelope)?;
    let role = envelope.envelope().signed().value()["role"]
        .as_str()
        .ok_or(Error::Verification)?;
    let denied: Vec<String> = bundle
        .denied_key_ids
        .iter()
        .chain(bundle.denied_by_role.get(role).into_iter().flatten())
        .cloned()
        .collect();
    verify_with_supplied_root(
        &bytes,
        VerificationInputs::new(root, &expected, now).with_denied_key_ids(&denied),
    )
    .map_err(|_| Error::Verification)?;
    Ok(envelope)
}
pub(crate) fn check_set(
    db: &Connection,
    set: i64,
    root: &RootPolicyShape,
    bundle: &Bundle,
    water_table: Option<&str>,
) -> Result<(), Error> {
    let (timestamp, snapshot, component): (String, String, String) = db.query_row(
        "SELECT timestamp_hash,snapshot_hash,component FROM sets WHERE id=?",
        [set],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if component != bundle.component || root.product() != bundle.product {
        return Err(Error::InvalidStore);
    }
    let now = bundle.time()?;
    let context = bundle.context()?;
    let timestamp = authenticate(db, &timestamp, root, &now, bundle)?;
    let snapshot = authenticate(db, &snapshot, root, &now, bundle)?;
    if let Some(table) = water_table {
        state::water(db, &timestamp, table)?;
        state::water(db, &snapshot, table)?;
    }
    let mut references = Vec::new();
    let mut hashes = Vec::new();
    let mut ordinal = 0;
    loop {
        let row:Option<(i64,String)>=db.query_row("SELECT ordinal,hash FROM members WHERE set_id=? AND ordinal>=? ORDER BY ordinal LIMIT 1",rusqlite::params![set,ordinal],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((index, hash)) = row else { break };
        if index != ordinal || index >= 4096 {
            return Err(Error::InvalidStore);
        }
        let envelope = authenticate(db, &hash, root, &now, bundle)?;
        if let Some(table) = water_table {
            state::water(db, &envelope, table)?;
        }
        references.push(
            derive_metadata_reference(envelope.envelope().raw_bytes())
                .map_err(|_| Error::Verification)?,
        );
        hashes.push(hash);
        ordinal += 1;
    }
    assert_supplied_public_graph_source(&timestamp, &snapshot, &references, &context, |index| {
        authenticate(db, &hashes[index], root, &now, bundle)
            .map_err(|_| pinvou_upgrade_codec::PublicGraphError::InvalidFields)
    })
    .map_err(|_| Error::Verification)
}
pub(crate) fn transition(
    db: &Connection,
    generation: i64,
    old_hash: &str,
    bundle: &Bundle,
    active_table: &str,
) -> Result<String, Error> {
    if !["active", "validated_active"].contains(&active_table) {
        return Err(Error::InvalidStore);
    }
    let old = root(db, old_hash)?;
    let expected = root_context(&bundle.product)?;
    let now = bundle.time()?;
    let count: i64 = db.query_row(
        "SELECT count(*) FROM root_steps WHERE generation=?",
        [generation],
        |r| r.get(0),
    )?;
    if count == 0 {
        let bytes = sql::original(db, old_hash)?;
        check_denied(&bytes, bundle, "root").map_err(|_| Error::Verification)?;
        old.assert_supplied_current_window(&now)
            .map_err(|_| Error::Verification)?;
        return Ok(old_hash.to_owned());
    }
    if !(1..=4096).contains(&count) {
        return Err(Error::InvalidStore);
    }
    let chain = verify_root_chain_source(
        count as usize,
        |index| {
            let hash: String = db
                .query_row(
                    "SELECT hash FROM root_steps WHERE generation=? AND ordinal=?",
                    rusqlite::params![generation, index as i64],
                    |r| r.get(0),
                )
                .map_err(|_| VerificationError::RootChainInvalid)?;
            let bytes =
                sql::original(db, &hash).map_err(|_| VerificationError::RootChainInvalid)?;
            check_denied(&bytes, bundle, "root")?;
            Ok(bytes)
        },
        &old,
        &expected,
        &now,
    )
    .map_err(|_| Error::Verification)?;
    // Complete source inventory comes exclusively from the DB snapshot. No
    // caller-provided empty active set can bypass another accepted component.
    let mut after = String::new();
    loop {
        let row:Option<(String,i64)>=db.query_row(&format!("SELECT component,set_id FROM {active_table} WHERE component>? ORDER BY component LIMIT 1"),[&after],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((component, set)) = row else { break };
        after = component;
        let (timestamp, snapshot): (String, String) = db.query_row(
            "SELECT timestamp_hash,snapshot_hash FROM sets WHERE id=?",
            [set],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        for hash in [timestamp, snapshot] {
            let bytes = sql::original(db, &hash)?;
            let envelope = shape(&bytes)?;
            let expected = state::expected(&envelope)?;
            chain
                .assert_active_binding(&ActiveMetadataInput::new(&bytes, &expected))
                .map_err(|_| Error::Verification)?;
        }
        let mut ordinal = 0;
        loop {
            let row:Option<(i64,String)>=db.query_row("SELECT ordinal,hash FROM members WHERE set_id=? AND ordinal>=? ORDER BY ordinal LIMIT 1",rusqlite::params![set,ordinal],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let Some((index, hash)) = row else { break };
            if index != ordinal {
                return Err(Error::InvalidStore);
            }
            ordinal += 1;
            let bytes = sql::original(db, &hash)?;
            let envelope = shape(&bytes)?;
            let expected = state::expected(&envelope)?;
            chain
                .assert_active_binding(&ActiveMetadataInput::new(&bytes, &expected))
                .map_err(|_| Error::Verification)?;
        }
    }
    Ok(crate::sha(chain.final_envelope().envelope().raw_bytes()))
}
fn check_denied(bytes: &[u8], bundle: &Bundle, role: &str) -> Result<(), VerificationError> {
    let envelope = shape(bytes).map_err(|_| VerificationError::RootChainInvalid)?;
    let denied = bundle
        .denied_key_ids
        .iter()
        .chain(bundle.denied_by_role.get(role).into_iter().flatten());
    for key in denied {
        if envelope
            .envelope()
            .signatures()
            .iter()
            .any(|signer| signer.key_id() == key)
        {
            return Err(VerificationError::SigningKeyDenied);
        }
    }
    Ok(())
}
pub(crate) fn root_context(product: &str) -> Result<Document, Error> {
    document(&crate::canonical(
        &serde_json::json!({"role":"root","product":product,"component":null,"scope":{}}),
    )?)
}
