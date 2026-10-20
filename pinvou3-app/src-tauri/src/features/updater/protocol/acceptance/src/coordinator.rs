use crate::{
    Bundle, Error, Inspection, Original, document, graph, history, input::read_bounded,
    platform::qa_sqlite as sql, sha, state,
};
use pinvou_upgrade_verifier::{VerificationInputs, verify_with_supplied_root};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use std::{fs, path::Path};

pub struct QaStore {
    db: Connection,
    pin: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Acceptance {
    pub generation: i64,
    pub root_sha256: String,
    pub protection: &'static str,
    pub current_qualification_checked: bool,
    pub verified_execution_object: bool,
}
fn receipt(generation: i64, root_sha256: String) -> Acceptance {
    Acceptance {
        generation,
        root_sha256,
        protection: "ordinary_isolated_qa",
        current_qualification_checked: false,
        verified_execution_object: false,
    }
}
impl QaStore {
    /// New explicit directory only. Initialization is never an implicit repair
    /// for missing/corrupt state and cannot adopt an incoming remote Root.
    pub fn create_new(
        directory: &Path,
        anchor: &[u8],
        external_pin: &str,
        supplied_now_ms: i64,
    ) -> Result<Self, Error> {
        if anchor.is_empty()
            || anchor.len() > crate::input::OBJECT_BYTES
            || sha(anchor) != external_pin
        {
            return Err(Error::InvalidInput);
        }
        let envelope = graph::shape(anchor)?;
        let root = envelope
            .envelope()
            .signed()
            .root_policy_shape()
            .map_err(|_| Error::InvalidInput)?;
        let expected = graph::root_context(root.product())?;
        let now = document(supplied_now_ms.to_string().as_bytes())?
            .protocol_time_shape()
            .map_err(|_| Error::InvalidInput)?;
        verify_with_supplied_root(anchor, VerificationInputs::new(&root, &expected, &now))
            .map_err(|_| Error::Verification)?;
        fs::create_dir(directory).map_err(|_| Error::Unavailable)?;
        let mut db = sql::connect(&directory.join("metadata-qa.sqlite"), true)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(sql::SCHEMA)?;
        let hash = sql::put(&tx, anchor)?;
        sql::write(
            &tx,
            "INSERT INTO store VALUES(1,1,?,?,?,?)",
            rusqlite::params![root.product(), hash, hash, supplied_now_ms],
        )?;
        // The explicit columns above intentionally have no caller-written water.
        state::water(&tx, &envelope, "waters")?;
        checkpoint("init-written")?;
        tx.commit().map_err(|_| Error::Uncertain)?;
        Ok(Self {
            db,
            pin: external_pin.to_owned(),
        })
    }
    pub fn open_existing(directory: &Path, external_pin: &str) -> Result<Self, Error> {
        let mut db = sql::connect(&directory.join("metadata-qa.sqlite"), false)?;
        let tx = db.transaction()?;
        history::validate(&tx, external_pin)?;
        tx.commit().map_err(|_| Error::Uncertain)?;
        Ok(Self {
            db,
            pin: external_pin.to_owned(),
        })
    }
    pub fn accept(&mut self, input_directory: &Path, bundle: &Bundle) -> Result<Acceptance, Error> {
        // Public Rust callers cannot bypass the same closed descriptor checks.
        let evidence = bundle.evidence()?;
        let bundle = Bundle::parse(&evidence)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let before = history::validate(&tx, &self.pin)?;
        if bundle.product != before.product {
            return Err(Error::InvalidInput);
        }
        // A post-COMMIT retry may include the already accepted Root batch.
        // Original metadata bytes, not unsigned filenames/order/time, identify
        // the set. Revalidate it with this request's actual time and deny inputs.
        if !bundle.roots.is_empty() && original_root_retry(&tx, &before, &bundle, input_directory)?
        {
            tx.rollback()?;
            return Ok(receipt(before.generation, before.root));
        }
        let next = before
            .generation
            .checked_add(1)
            .filter(|v| *v <= 9_007_199_254_740_991)
            .ok_or(Error::InvalidStore)?;
        sql::write(
            &tx,
            "INSERT INTO batches(generation,before_root,after_root,observed_at,component,evidence) VALUES(?,?,?,?,?,?)",
            rusqlite::params![
                next,
                before.root,
                before.root,
                bundle.supplied_now_ms,
                bundle.component,
                evidence
            ],
        )?;
        for (index, name) in bundle.roots.iter().enumerate() {
            let bytes = read_bounded(&bundle.path(input_directory, name)?)?;
            let hash = sql::put(&tx, &bytes)?;
            sql::write(
                &tx,
                "INSERT INTO root_steps VALUES(?,?,?)",
                rusqlite::params![next, index as i64, hash],
            )?;
        }
        let new_root = graph::transition(&tx, next, &before.root, &bundle, "active")?;
        sql::write(
            &tx,
            "UPDATE batches SET after_root=? WHERE generation=?",
            rusqlite::params![new_root, next],
        )?;
        for index in 0..bundle.roots.len() {
            let hash: String = tx.query_row(
                "SELECT hash FROM root_steps WHERE generation=? AND ordinal=?",
                rusqlite::params![next, index as i64],
                |r| r.get(0),
            )?;
            state::water(&tx, &graph::shape(&sql::original(&tx, &hash)?)?, "waters")?;
        }
        checkpoint("roots-written")?;
        let timestamp = sql::put(
            &tx,
            &read_bounded(&bundle.path(input_directory, &bundle.timestamp)?)?,
        )?;
        let snapshot = sql::put(
            &tx,
            &read_bounded(&bundle.path(input_directory, &bundle.snapshot)?)?,
        )?;
        sql::write(
            &tx,
            "INSERT INTO sets(component,generation,timestamp_hash,snapshot_hash) VALUES(?,?,?,?)",
            rusqlite::params![bundle.component, next, timestamp, snapshot],
        )?;
        let set = tx.last_insert_rowid();
        for (index, name) in bundle.members.iter().enumerate() {
            let hash = sql::put(&tx, &read_bounded(&bundle.path(input_directory, name)?)?)?;
            sql::write(
                &tx,
                "INSERT INTO members VALUES(?,?,?)",
                rusqlite::params![set, index as i64, hash],
            )?;
        }
        graph::check_set(
            &tx,
            set,
            &graph::root(&tx, &new_root)?,
            &bundle,
            Some("waters"),
        )?;
        checkpoint("objects-written")?;
        let old_set: Option<i64> = tx
            .query_row(
                "SELECT set_id FROM active WHERE component=?",
                [&bundle.component],
                |r| r.get(0),
            )
            .optional()?;
        if bundle.roots.is_empty()
            && let Some(old) = old_set
            && same_set(&tx, old, set)?
        {
            tx.rollback()?;
            return Ok(receipt(before.generation, before.root));
        }
        sql::write(
            &tx,
            "INSERT INTO active(component,set_id) VALUES(?,?) ON CONFLICT(component) DO UPDATE SET set_id=excluded.set_id",
            rusqlite::params![bundle.component, set],
        )?;
        let changed = sql::write(
            &tx,
            "UPDATE store SET generation=?,root_hash=? WHERE singleton=1 AND generation=? AND root_hash=?",
            rusqlite::params![next, new_root, before.generation, before.root],
        )?;
        if changed != 1 {
            return Err(Error::Conflict);
        }
        checkpoint("head-written")?;
        history::validate(&tx, &self.pin)?;
        checkpoint("before-commit")?;
        tx.commit().map_err(|_| Error::Uncertain)?;
        checkpoint("after-commit")?;
        Ok(receipt(next, new_root))
    }
    pub fn inspect(&mut self) -> Result<Inspection, Error> {
        let tx = self.db.transaction()?;
        let meta = history::validate(&tx, &self.pin)?;
        let components = tx.query_row("SELECT count(*) FROM active", [], |r| r.get(0))?;
        let objects = tx.query_row("SELECT count(*) FROM objects", [], |r| r.get(0))?;
        tx.commit().map_err(|_| Error::Uncertain)?;
        Ok(Inspection {
            generation: meta.generation,
            product: meta.product,
            root_sha256: meta.root,
            components,
            objects,
            protection: "ordinary_isolated_qa",
            current_qualification_checked: false,
            verified_execution_object: false,
        })
    }
    /// One bounded frame in canonical context key order. A caller owns the
    /// explicit cursor; this is historical QA data, never an execution token.
    pub fn original_after(
        &mut self,
        after: &str,
        expected_generation: i64,
    ) -> Result<Option<Original>, Error> {
        if after.len() > crate::input::OBJECT_BYTES {
            return Err(Error::InvalidInput);
        }
        let tx = self.db.transaction()?;
        let meta = history::validate(&tx, &self.pin)?;
        if meta.generation != expected_generation {
            return Err(Error::Conflict);
        }
        let row: Option<(String, i64, String)> = tx
            .query_row(
                "SELECT context,version,hash FROM waters WHERE context>? ORDER BY context LIMIT 1",
                [after],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let value = if let Some((context, version, hash)) = row {
            let bytes = sql::original(&tx, &hash)?;
            Some(Original {
                generation: meta.generation,
                context: document(context.as_bytes())?.value().clone(),
                version: Some(version),
                envelope_sha256: hash,
                original_utf8: String::from_utf8(bytes).map_err(|_| Error::InvalidStore)?,
            })
        } else {
            None
        };
        tx.commit().map_err(|_| Error::Uncertain)?;
        Ok(value)
    }
    /// Read an exact accepted historical object, including immutable Package
    /// envelopes which have no version high water. Hash is never a file path.
    pub fn original(&mut self, hash: &str, expected_generation: i64) -> Result<Original, Error> {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::InvalidInput);
        }
        let tx = self.db.transaction()?;
        let meta = history::validate(&tx, &self.pin)?;
        if meta.generation != expected_generation {
            return Err(Error::Conflict);
        }
        let bytes = sql::original(&tx, hash)?;
        let envelope = graph::shape(&bytes)?;
        let value = Original {
            generation: meta.generation,
            context: state::expected(&envelope)?.value().clone(),
            version: state::water_identity(&envelope)?.map(|(_, v)| v),
            envelope_sha256: hash.to_owned(),
            original_utf8: String::from_utf8(bytes).map_err(|_| Error::InvalidStore)?,
        };
        tx.commit().map_err(|_| Error::Uncertain)?;
        Ok(value)
    }
}
fn same_set(db: &Connection, a: i64, b: i64) -> Result<bool, Error> {
    let row = |id| {
        db.query_row(
            "SELECT timestamp_hash,snapshot_hash FROM sets WHERE id=?",
            [id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
    };
    if row(a)? != row(b)? {
        return Ok(false);
    }
    for (left, right) in [(a, b), (b, a)] {
        if db.query_row("SELECT EXISTS(SELECT hash FROM members WHERE set_id=? EXCEPT SELECT hash FROM members WHERE set_id=?)",rusqlite::params![left,right],|r|r.get::<_,bool>(0))? {return Ok(false);}
    }
    Ok(true)
}
fn original_root_retry(
    db: &Connection,
    meta: &state::Meta,
    bundle: &Bundle,
    directory: &Path,
) -> Result<bool, Error> {
    let row:Option<(i64,i64,String)>=db.query_row("SELECT b.generation,s.id,b.before_root FROM active a JOIN sets s ON s.id=a.set_id JOIN batches b ON b.generation=s.generation WHERE b.after_root=? AND a.component=?",rusqlite::params![meta.root,bundle.component],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let Some((generation, set, before_root)) = row else {
        return Ok(false);
    };
    let count: i64 = db.query_row(
        "SELECT count(*) FROM root_steps WHERE generation=?",
        [generation],
        |r| r.get(0),
    )?;
    if count != bundle.roots.len() as i64 {
        return Ok(false);
    }
    for (index, name) in bundle.roots.iter().enumerate() {
        let hash: String = db.query_row(
            "SELECT hash FROM root_steps WHERE generation=? AND ordinal=?",
            rusqlite::params![generation, index as i64],
            |r| r.get(0),
        )?;
        if sha(&read_bounded(&bundle.path(directory, name)?)?) != hash {
            return Ok(false);
        }
    }
    let (timestamp, snapshot): (String, String) = db.query_row(
        "SELECT timestamp_hash,snapshot_hash FROM sets WHERE id=?",
        [set],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    for (name, hash) in [(&bundle.timestamp, timestamp), (&bundle.snapshot, snapshot)] {
        if sha(&read_bounded(&bundle.path(directory, name)?)?) != hash {
            return Ok(false);
        }
    }
    let count: i64 = db.query_row("SELECT count(*) FROM members WHERE set_id=?", [set], |r| {
        r.get(0)
    })?;
    if count != bundle.members.len() as i64 {
        return Ok(false);
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in &bundle.members {
        let hash = sha(&read_bounded(&bundle.path(directory, name)?)?);
        if !seen.insert(hash.clone())
            || !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM members WHERE set_id=? AND hash=?)",
                rusqlite::params![set, hash],
                |r| r.get::<_, bool>(0),
            )?
        {
            return Ok(false);
        }
    }
    // Use the genuine old+new dual-threshold transition kernel, not ordinary
    // new-Root self verification (retired old signers are valid on successors).
    if graph::transition(db, generation, &before_root, bundle, "active")? != meta.root {
        return Err(Error::InvalidStore);
    }
    let root = graph::root(db, &meta.root)?;
    graph::check_set(db, set, &root, bundle, None)?;
    Ok(true)
}
// Test-only subprocess seam. No provider, owner, signature, clock or accepted
// state is synthesized. The parent waits a fixed actual SQL stage, then kills
// and joins this process or sends a newline to continue the real transaction.
pub(crate) fn checkpoint(_stage: &str) -> Result<(), Error> {
    #[cfg(feature = "fault-tests")]
    if std::env::var("PINVOU_METADATA_QA_CHECKPOINT")
        .ok()
        .as_deref()
        == Some(_stage)
    {
        use std::io::Write;
        let output = std::io::stdout();
        let mut output = output.lock();
        let failure = || {
            if _stage == "after-commit" {
                Error::Uncertain
            } else {
                Error::Unavailable
            }
        };
        writeln!(output, "QA_CHECKPOINT {_stage}")
            .and_then(|_| output.flush())
            .map_err(|_| failure())?;
        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .map_err(|_| failure())?;
        if _stage == "after-commit" && input.trim() == "lose-ack" {
            return Err(Error::Uncertain);
        }
    }
    Ok(())
}
