//! Ordinary QA SQLite storage. No protected directory, trusted clock, hostile
//! same-user namespace defense or whole-database rollback resistance is implied.
use crate::{Error, input::OBJECT_BYTES, sha};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{path::Path, time::Duration};
pub(crate) const SCHEMA: &str = include_str!("schema.sql");

pub(crate) fn connect(path: &Path, create: bool) -> Result<Connection, Error> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | if create {
            OpenFlags::SQLITE_OPEN_CREATE
        } else {
            OpenFlags::empty()
        };
    let connection = Connection::open_with_flags(path, flags)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    // Validated high-water/coverage inventories can span the entire legitimate
    // history. Keep those temporary B-trees file-backed, not whole-store RAM.
    connection.pragma_update(None, "temp_store", 1)?;
    if connection.query_row("PRAGMA temp_store", [], |r| r.get::<_, i64>(0))? != 1 {
        return Err(Error::Unavailable);
    }
    // DELETE uses SQLite's actual rollback journal and avoids retaining an
    // unauthenticated WAL side channel as a purported separate state owner.
    let journal: String = connection.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))?;
    if journal != "delete" {
        return Err(Error::Unavailable);
    }
    Ok(connection)
}
pub(crate) fn check_schema(connection: &Connection) -> Result<(), Error> {
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(SCHEMA)?;
    // Only the embedded, trusted schema is collected. Include implicit indexes
    // too: a NULL SQL definition must not hide an unexpected schema object.
    let mut statement = expected.prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema")?;
    let definitions = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let count: i64 = connection.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM sqlite_schema LIMIT ?)",
        [definitions.len() as i64 + 1],
        |r| r.get(0),
    )?;
    if count != definitions.len() as i64 {
        return Err(Error::InvalidStore);
    }
    for (kind, name, table, definition) in definitions {
        // SQLite's octet_length(column) uses stored byte-length metadata; unlike
        // length(TEXT) or a CAST this does not read the entire text payload.
        // Inspect lengths before materializing any untrusted definition.
        // This is a per-schema-object limit, not a database/history size limit.
        let lengths:Option<(i64,i64,i64,Option<i64>)>=connection.query_row("SELECT octet_length(type),octet_length(name),octet_length(tbl_name),octet_length(sql) FROM sqlite_schema WHERE type=? AND name=? AND tbl_name=?",rusqlite::params![kind,name,table],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        if lengths
            != Some((
                kind.len() as i64,
                name.len() as i64,
                table.len() as i64,
                definition.as_ref().map(|v| v.len() as i64),
            ))
        {
            return Err(Error::InvalidStore);
        }
        let actual: Option<String> = connection.query_row(
            "SELECT sql FROM sqlite_schema WHERE type=? AND name=? AND tbl_name=?",
            rusqlite::params![kind, name, table],
            |r| r.get(0),
        )?;
        if actual != definition {
            return Err(Error::InvalidStore);
        }
    }
    if connection.query_row("PRAGMA application_id", [], |r| r.get::<_, i64>(0))? != 1347834167
        || connection.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? != 1
    {
        return Err(Error::InvalidStore);
    }
    check_row_profiles(connection)?;
    let failed: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_quick_check WHERE quick_check<>'ok')",
        [],
        |r| r.get(0),
    )?;
    if failed {
        return Err(Error::InvalidStore);
    }
    let foreign: Option<i64> = connection
        .query_row("SELECT 1 FROM pragma_foreign_key_check LIMIT 1", [], |r| {
            r.get(0)
        })
        .optional()?;
    if foreign.is_some() {
        return Err(Error::InvalidStore);
    }
    Ok(())
}

fn check_row_profiles(db: &Connection) -> Result<(), Error> {
    // Every persistent field is checked inside the same SQLite snapshot before
    // meta/history/keyset readers allocate a String or Vec. These are original
    // object/identifier/hash limits; no limit on accepted history is introduced.
    for (table, fields) in [
        (
            "store",
            vec![
                ("singleton", "one"),
                ("generation", "positive"),
                ("product", "identifier"),
                ("anchor_hash", "hash"),
                ("root_hash", "hash"),
                ("initialized_at", "integer"),
            ],
        ),
        ("objects", vec![("hash", "hash"), ("bytes", "blob")]),
        (
            "waters",
            vec![
                ("context", "context"),
                ("version", "positive"),
                ("hash", "hash"),
            ],
        ),
        (
            "batches",
            vec![
                ("generation", "batch"),
                ("before_root", "hash"),
                ("after_root", "hash"),
                ("observed_at", "integer"),
                ("component", "identifier"),
                ("evidence", "blob"),
            ],
        ),
        (
            "root_steps",
            vec![
                ("generation", "batch"),
                ("ordinal", "ordinal"),
                ("hash", "hash"),
            ],
        ),
        (
            "sets",
            vec![
                ("id", "rowid"),
                ("component", "identifier"),
                ("generation", "batch"),
                ("timestamp_hash", "hash"),
                ("snapshot_hash", "hash"),
            ],
        ),
        (
            "members",
            vec![
                ("set_id", "rowid"),
                ("ordinal", "ordinal"),
                ("hash", "hash"),
            ],
        ),
        (
            "active",
            vec![("component", "identifier"), ("set_id", "rowid")],
        ),
    ] {
        for (column, profile) in fields {
            let valid = match profile {
                "hash" => format!(
                    "typeof({column})='text' AND octet_length({column})=64 AND {column} NOT GLOB '*[^0-9a-f]*'"
                ),
                "identifier" => format!(
                    "typeof({column})='text' AND octet_length({column}) BETWEEN 1 AND 128 AND {column} NOT GLOB '*[^A-Za-z0-9._:-]*' AND substr({column},1,1) GLOB '[A-Za-z0-9]'"
                ),
                "context" => format!(
                    "typeof({column})='text' AND octet_length({column}) BETWEEN 1 AND 1048576"
                ),
                "blob" => {
                    format!("typeof({column})='blob' AND length({column}) BETWEEN 1 AND 1048576")
                }
                "rowid" => format!("typeof({column})='integer' AND {column}>0"),
                "one" => format!("typeof({column})='integer' AND {column}=1"),
                "ordinal" => format!("typeof({column})='integer' AND {column} BETWEEN 0 AND 4095"),
                "batch" => format!(
                    "typeof({column})='integer' AND {column} BETWEEN 2 AND 9007199254740991"
                ),
                "positive" => format!(
                    "typeof({column})='integer' AND {column} BETWEEN 1 AND 9007199254740991"
                ),
                "integer" => format!(
                    "typeof({column})='integer' AND {column} BETWEEN 0 AND 9007199254740991"
                ),
                _ => return Err(Error::InvalidStore),
            };
            if db.query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM {table} WHERE NOT ({valid}) OR {column} IS NULL)"
                ),
                [],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(Error::InvalidStore);
            }
        }
    }
    Ok(())
}
pub(crate) fn original(db: &Connection, hash: &str) -> Result<Vec<u8>, Error> {
    // Bound before fetching a BLOB; never allocate from an unchecked database row.
    let len: i64 = db
        .query_row(
            "SELECT length(bytes) FROM objects WHERE hash=?",
            [hash],
            |r| r.get(0),
        )
        .map_err(|_| Error::InvalidStore)?;
    if !(1..=OBJECT_BYTES as i64).contains(&len) {
        return Err(Error::InvalidStore);
    }
    let bytes: Vec<u8> = db.query_row("SELECT bytes FROM objects WHERE hash=?", [hash], |r| {
        r.get(0)
    })?;
    if sha(&bytes) != hash {
        return Err(Error::InvalidStore);
    }
    Ok(bytes)
}
pub(crate) fn put(db: &Connection, bytes: &[u8]) -> Result<String, Error> {
    if bytes.is_empty() || bytes.len() > OBJECT_BYTES {
        return Err(Error::InvalidInput);
    }
    let hash = sha(bytes);
    write(
        db,
        "INSERT INTO objects(hash,bytes) VALUES(?,?) ON CONFLICT(hash) DO NOTHING",
        rusqlite::params![hash, bytes],
    )?;
    if original(db, &hash)? != bytes {
        return Err(Error::InvalidStore);
    }
    Ok(hash)
}

pub(crate) fn write(
    db: &Connection,
    statement: &str,
    params: impl rusqlite::Params,
) -> Result<usize, Error> {
    let changed = db.execute(statement, params)?;
    #[cfg(feature = "fault-tests")]
    {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static WRITES: AtomicUsize = AtomicUsize::new(0);
        let index = WRITES.fetch_add(1, Ordering::Relaxed) + 1;
        if std::env::var("PINVOU_METADATA_QA_TRACE_WRITES")
            .ok()
            .as_deref()
            == Some("1")
        {
            use std::io::Write;
            writeln!(std::io::stderr().lock(), "QA_SQL_WRITE {index}")
                .map_err(|_| Error::Unavailable)?;
        }
        crate::coordinator::checkpoint(&format!("sql-write-{index}"))?;
    }
    Ok(changed)
}

#[cfg(test)]
mod resource_tests {
    use super::*;
    #[test]
    fn bundled_sqlite_byte_length_gate_uses_column_metadata() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE sample(value TEXT)").unwrap();
        db.execute("INSERT INTO sample VALUES(?)", ["日".repeat(4096)])
            .unwrap();
        let lengths: (i64, i64) = db
            .query_row(
                "SELECT length(value),octet_length(value) FROM sample",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(lengths, (4096, 12288));
        let mut q = db
            .prepare("EXPLAIN SELECT octet_length(value) FROM sample")
            .unwrap();
        let columns = q
            .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(6)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        // SQLITE_FUNC_BYTELEN/OPFLAG_BYTELENARG in this pinned bundled engine.
        // A regression to an ordinary materializing column read is rejected.
        assert!(
            columns
                .iter()
                .any(|(op, flags)| op == "Column" && flags & 0xc0 == 0xc0)
        );
    }
}
