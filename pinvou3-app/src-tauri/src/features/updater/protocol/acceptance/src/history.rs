//! Replays genuine committed batches at their retained historical supplied time.
//! No historical proof escapes as current authority; traversal is DB-keyset bounded.
use crate::{Bundle, Error, document, graph, platform::qa_sqlite as sql, state};
use pinvou_upgrade_verifier::{VerificationInputs, verify_with_supplied_root};
use rusqlite::{Connection, OptionalExtension};

pub(crate) fn validate(db: &Connection, pin: &str) -> Result<state::Meta, Error> {
    sql::check_schema(db)?;
    let meta = state::meta(db, pin)?;
    let anchor = sql::original(db, &meta.anchor)?;
    let root = graph::root(db, &meta.anchor)?;
    if root.product() != meta.product {
        return Err(Error::InvalidStore);
    }
    let now = document(meta.initialized.to_string().as_bytes())?
        .protocol_time_shape()
        .map_err(|_| Error::InvalidStore)?;
    let expected = graph::root_context(&meta.product)?;
    let checked =
        verify_with_supplied_root(&anchor, VerificationInputs::new(&root, &expected, &now))
            .map_err(|_| Error::InvalidStore)?;
    db.execute_batch(include_str!("platform/validation.sql"))?;
    state::water(db, checked.envelope(), "expected_waters")?;
    db.execute("INSERT INTO visited_objects VALUES(?)", [&meta.anchor])?;
    let mut generation = 1;
    let mut current = meta.anchor.clone();
    loop {
        let row:Option<(i64,String,String,i64,String,i64)>=db.query_row("SELECT generation,before_root,after_root,observed_at,component,length(evidence) FROM batches WHERE generation>? ORDER BY generation LIMIT 1",[generation],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
        let Some((next, before, after, observed, component, length)) = row else {
            break;
        };
        if next != generation + 1 || before != current || !(1..=1_048_576).contains(&length) {
            return Err(Error::InvalidStore);
        }
        let evidence: Vec<u8> = db.query_row(
            "SELECT evidence FROM batches WHERE generation=?",
            [next],
            |r| r.get(0),
        )?;
        let bundle = Bundle::parse(&evidence)?;
        if bundle.supplied_now_ms != observed
            || bundle.product != meta.product
            || bundle.component != component
        {
            return Err(Error::InvalidStore);
        }
        let actual = graph::transition(db, next, &current, &bundle, "validated_active")?;
        if actual != after {
            return Err(Error::InvalidStore);
        }
        let count: i64 = db.query_row(
            "SELECT count(*) FROM root_steps WHERE generation=?",
            [next],
            |r| r.get(0),
        )?;
        if count != bundle.roots.len() as i64 {
            return Err(Error::InvalidStore);
        }
        for index in 0..count {
            let hash: String = db.query_row(
                "SELECT hash FROM root_steps WHERE generation=? AND ordinal=?",
                rusqlite::params![next, index],
                |r| r.get(0),
            )?;
            state::water(
                db,
                &graph::shape(&sql::original(db, &hash)?)?,
                "expected_waters",
            )?;
            db.execute(
                "INSERT INTO visited_root_steps VALUES(?,?)",
                rusqlite::params![next, index],
            )?;
            db.execute("INSERT OR IGNORE INTO visited_objects VALUES(?)", [hash])?;
        }
        let set: i64 = db.query_row(
            "SELECT id FROM sets WHERE generation=? AND component=?",
            rusqlite::params![next, component],
            |r| r.get(0),
        )?;
        if db.query_row(
            "SELECT count(*) FROM sets WHERE generation=?",
            [next],
            |r| r.get::<_, i64>(0),
        )? != 1
        {
            return Err(Error::InvalidStore);
        }
        let member_count: i64 =
            db.query_row("SELECT count(*) FROM members WHERE set_id=?", [set], |r| {
                r.get(0)
            })?;
        if member_count != bundle.members.len() as i64 {
            return Err(Error::InvalidStore);
        }
        graph::check_set(
            db,
            set,
            &graph::root(db, &after)?,
            &bundle,
            Some("expected_waters"),
        )?;
        // Mark only after the entire real batch and exact graph have passed.
        // Merely being reachable from a row/FK never makes an object accepted.
        db.execute("INSERT INTO visited_batches VALUES(?)", [next])?;
        db.execute("INSERT INTO visited_sets VALUES(?)", [set])?;
        db.execute(
            "INSERT INTO visited_members SELECT set_id,ordinal FROM members WHERE set_id=?",
            [set],
        )?;
        db.execute(
            "INSERT OR IGNORE INTO visited_objects SELECT hash FROM members WHERE set_id=?",
            [set],
        )?;
        db.execute("INSERT OR IGNORE INTO visited_objects SELECT timestamp_hash FROM sets WHERE id=? UNION SELECT snapshot_hash FROM sets WHERE id=?",rusqlite::params![set,set])?;
        db.execute("INSERT INTO validated_active(component,set_id) VALUES(?,?) ON CONFLICT(component) DO UPDATE SET set_id=excluded.set_id",rusqlite::params![component,set])?;
        current = after;
        generation = next;
    }
    if generation != meta.generation || current != meta.root {
        return Err(Error::InvalidStore);
    }
    ensure_coverage(db)?;
    Ok(meta)
}

fn ensure_coverage(db: &Connection) -> Result<(), Error> {
    if db.query_row("SELECT count(*) FROM store", [], |r| r.get::<_, i64>(0))? != 1 {
        return Err(Error::InvalidStore);
    }
    for (left, right, cols) in [
        ("waters", "expected_waters", "context,version,hash"),
        ("active", "validated_active", "component,set_id"),
        ("objects", "visited_objects", "hash"),
        ("batches", "visited_batches", "generation"),
        ("root_steps", "visited_root_steps", "generation,ordinal"),
        ("sets", "visited_sets", "id"),
        ("members", "visited_members", "set_id,ordinal"),
    ] {
        for (a, b) in [(left, right), (right, left)] {
            let difference: bool = db.query_row(
                &format!("SELECT EXISTS(SELECT {cols} FROM {a} EXCEPT SELECT {cols} FROM {b})"),
                [],
                |r| r.get(0),
            )?;
            if difference {
                return Err(Error::InvalidStore);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    #[test]
    fn every_persistent_row_has_bidirectional_validated_key_coverage() {
        // This tests the SQL coverage predicate, not cryptographic acceptance.
        // Actual independent CLI signature/source rejection is tested separately.
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(sql::SCHEMA).unwrap();
        db.execute_batch("INSERT INTO objects VALUES('h',X'7B7D'); INSERT INTO store VALUES(1,2,'pinvou','h','h',0); INSERT INTO batches VALUES(2,'h','h',0,'app',X'7B7D'); INSERT INTO root_steps VALUES(2,0,'h'); INSERT INTO sets VALUES(1,'app',2,'h','h'); INSERT INTO members VALUES(1,0,'h'); INSERT INTO waters VALUES('ctx',1,'h'); INSERT INTO active VALUES('app',1);").unwrap();
        db.execute_batch(include_str!("platform/validation.sql"))
            .unwrap();
        let tables = [
            ("waters", "expected_waters", "context,version,hash"),
            ("active", "validated_active", "component,set_id"),
            ("objects", "visited_objects", "hash"),
            ("batches", "visited_batches", "generation"),
            ("root_steps", "visited_root_steps", "generation,ordinal"),
            ("sets", "visited_sets", "id"),
            ("members", "visited_members", "set_id,ordinal"),
        ];
        for (source, marker, cols) in tables {
            db.execute(
                &format!("INSERT INTO {marker} SELECT {cols} FROM {source}"),
                [],
            )
            .unwrap();
        }
        ensure_coverage(&db).unwrap();
        for (source, marker, cols) in tables {
            db.execute(&format!("DELETE FROM {marker}"), []).unwrap();
            assert_eq!(
                ensure_coverage(&db),
                Err(Error::InvalidStore),
                "unvisited persistent {source}"
            );
            db.execute(
                &format!("INSERT INTO {marker} SELECT {cols} FROM {source}"),
                [],
            )
            .unwrap();
            ensure_coverage(&db).unwrap();
        }
        db.execute("INSERT INTO visited_objects VALUES('invented')", [])
            .unwrap();
        assert_eq!(ensure_coverage(&db), Err(Error::InvalidStore));
        db.execute("DELETE FROM visited_objects WHERE hash='invented'", [])
            .unwrap();
        ensure_coverage(&db).unwrap();
    }
}
