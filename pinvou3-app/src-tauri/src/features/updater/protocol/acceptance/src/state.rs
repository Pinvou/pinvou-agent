use crate::{Error, canonical, document, sha};
use pinvou_upgrade_codec::{HighWaterError, SchemaCheckedEnvelope, assert_supplied_high_water};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub generation: i64,
    pub product: String,
    pub root_sha256: String,
    pub components: i64,
    pub objects: i64,
    pub protection: &'static str,
    pub current_qualification_checked: bool,
    pub verified_execution_object: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Original {
    pub generation: i64,
    pub context: Value,
    pub version: Option<i64>,
    pub envelope_sha256: String,
    pub original_utf8: String,
}
pub(crate) struct Meta {
    pub generation: i64,
    pub product: String,
    pub anchor: String,
    pub root: String,
    pub initialized: i64,
}
pub(crate) fn meta(db: &Connection, pin: &str) -> Result<Meta, Error> {
    let value=db.query_row("SELECT generation,product,anchor_hash,root_hash,initialized_at FROM store WHERE singleton=1",[],|r|Ok(Meta{generation:r.get(0)?,product:r.get(1)?,anchor:r.get(2)?,root:r.get(3)?,initialized:r.get(4)?})).map_err(|_|Error::InvalidStore)?;
    if pin.len() != 64
        || !pin
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || value.anchor != pin
        || value.generation < 1
    {
        return Err(Error::InvalidStore);
    }
    Ok(value)
}
pub(crate) fn expected(
    envelope: &SchemaCheckedEnvelope,
) -> Result<pinvou_upgrade_codec::Document, Error> {
    let value = envelope.envelope().signed().value();
    document(&canonical(
        &json!({"role":value["role"],"product":value["product"],"component":value["component"],"scope":value["scope"]}),
    )?)
}
pub(crate) fn water_identity(
    envelope: &SchemaCheckedEnvelope,
) -> Result<Option<(String, i64)>, Error> {
    let v = envelope.envelope().signed().value();
    let version = match v["role"].as_str() {
        Some("root" | "timestamp" | "snapshot" | "target") => safe_version(&v["version"]),
        Some("release") => safe_version(&v["revision"]),
        Some("package") => return Ok(None),
        _ => None,
    }
    .ok_or(Error::InvalidStore)?;
    let ctx = expected(envelope)?
        .canonical_bytes()
        .map_err(|_| Error::InvalidStore)?;
    Ok(Some((
        String::from_utf8(ctx).map_err(|_| Error::InvalidStore)?,
        version,
    )))
}
fn safe_version(value: &Value) -> Option<i64> {
    // The original bounded Document parser uses finite binary64 numbers, even
    // when the canonical JSON spelling is an integer. Match its SAFE profile
    // before the lossless SQLite INTEGER conversion; as_i64() is not equivalent.
    value
        .as_f64()
        .filter(|v| v.is_finite() && v.fract() == 0.0 && *v >= 1.0 && *v <= 9_007_199_254_740_991.0)
        .map(|v| v as i64)
}
pub(crate) fn water(
    db: &Connection,
    envelope: &SchemaCheckedEnvelope,
    table: &str,
) -> Result<(), Error> {
    let Some((context, version)) = water_identity(envelope)? else {
        return Ok(());
    };
    // table is selected only by these two fixed internal call paths.
    if !["waters", "expected_waters"].contains(&table) {
        return Err(Error::InvalidStore);
    }
    let previous: Option<(i64, String)> = db
        .query_row(
            &format!("SELECT version,hash FROM {table} WHERE context=?"),
            [&context],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((old, hash)) = previous {
        let record = document(&canonical(
            &json!({"scope":expected(envelope)?.value(),"version":old,"envelopeSha256":hash}),
        )?)?;
        assert_supplied_high_water(envelope.envelope(), &record).map_err(|e| match e {
            HighWaterError::Rollback => Error::Rollback,
            HighWaterError::Equivocation => Error::Equivocation,
            _ => Error::InvalidStore,
        })?;
    }
    let statement = format!(
        "INSERT INTO {table}(context,version,hash) VALUES(?,?,?) ON CONFLICT(context) DO UPDATE SET version=excluded.version,hash=excluded.hash"
    );
    let hash = sha(envelope.envelope().raw_bytes());
    let params = rusqlite::params![context, version, hash];
    if table == "waters" {
        crate::platform::qa_sqlite::write(db, &statement, params)?;
    } else {
        db.execute(&statement, params)?;
    }
    Ok(())
}
