//! 跨会话交付物索引。
//!
//! 从 `app::commands::artifacts` 回流的纯域逻辑。Tauri 命令薄壳留在
//! `app::commands::artifacts`，本模块只根据会话磁盘真相装配交付物索引。

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::core::stamp_cache::{FileStamp, StampCache};

/// 「产出物」跨会话索引:遍历 `~/.pinvou3/sessions/*.json`,把每个会话跟踪的
/// artifacts 汇成一张扁平表(供「产出物」一级入口用)。只走磁盘真相:
/// 文件已被删则跳过;mtime/size 现取 fs。
#[derive(Debug, Clone, Deserialize)]
struct DvSessionView {
    metadata: DvMeta,
    #[serde(default)]
    artifacts: Vec<DvArtifact>,
}
#[derive(Debug, Clone, Deserialize)]
struct DvMeta {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
}
#[derive(Debug, Clone, Deserialize)]
struct DvArtifact {
    storage_path: std::path::PathBuf,
    #[serde(default)]
    byte_size: u64,
}

/// One deliverable row. Fields are `pub` because the row set is the shared
/// cross-surface contract: the headless CLI's `artifacts list` renders these
/// directly (JSON keys mirror the serde names, minus the camelCase rename).
#[derive(Debug, Clone, Serialize)]
pub struct DeliverableItem {
    pub name: String,
    pub path: String,
    pub ext: String,
    pub category: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub source: String,
    pub mtime: i64,
    pub size: u64,
}

/// What one index scan produced: the deliverable rows, plus the session
/// records the scan could not read (oversized, unreadable, or unparseable).
///
/// The skipped list is not cosmetic — an incomplete listing must stay
/// distinguishable from an empty one ("incomplete ≠ empty"); a consumer
/// reads it unconditionally rather than guessing from a truncated index.
/// The GUI command forwards the struct whole (round-4 review): mapping it to
/// `rows` at the shell made an oversized/unreadable record render as a
/// successful empty/partial panel with no notice.
#[derive(Debug, Clone, Serialize)]
pub struct DeliverableIndex {
    pub rows: Vec<DeliverableItem>,
    pub skipped: Vec<String>,
}

/// The deliverable extension whitelist, shared with the headless CLI's
/// `artifacts list` (which imports this table instead of a hand copy — a
/// copy silently dropped every extension added here later).
pub const DELIVERABLE_EXTS: &[&str] = &[
    "pptx", "ppt", "docx", "doc", "pdf", "html", "htm", "xlsx", "xls", "md", "csv", "png", "jpg",
    "jpeg", "svg", "gif", "webp", "zip",
];

/// Entry limit of the session JSON parse cache. One entry holds only
/// metadata+artifacts and is tiny; when full, a single entry is evicted (in
/// hash-map order) instead of clearing the whole table, avoiding a cache
/// avalanche on inventory refreshes.
const DV_VIEW_CACHE_LIMIT: usize = 512;

/// Cache of the index view (metadata+artifacts) parsed out of each session
/// JSON, keyed by (mtime, len). Session files are rewritten whole every
/// turn, so the mtime always changes; the cache only saves the redundant
/// full read+parse of unchanged files — exactly the common case during
/// high-frequency "deliverables" list refreshes. The view's remaining
/// derived data (artifact file mtime/size read live from the fs, extension
/// filtering, sorting) is recomputed on every call. Parsing is a pure read
/// with no side effects, so the cached value is safe to reuse.
static DV_VIEW_CACHE: StampCache<DvSessionView> = StampCache::new(DV_VIEW_CACHE_LIMIT);

/// Only extensions that survive the DELIVERABLE_EXTS filter (the sole
/// caller filters first) can reach this match; broader extensions such as
/// mhtml/odp/bmp never get here, so they are not listed.
pub fn deliverable_category(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" => "web",
        "ppt" | "pptx" => "ppt",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => "img",
        _ => "doc",
    }
}

/// Scan-cap of one session record's bytes during the index scan: `list`
/// reads only the metadata header, but a session record is parsed whole, so
/// a huge transcript must not dominate (or stall) the listing — oversized
/// records are skipped with a `skipped_sessions` note instead of read.
pub const MAX_LIST_SCAN_BYTES: u64 = 32 * 1024 * 1024;

/// Assembles the cross-session deliverables index shared by the GUI
/// `list_deliverable_index` command and the headless CLI `artifacts list`
/// (the CLI previously carried a verbatim hand-copy of this implementation,
/// and was the more correct side on two divergences — a failed read_dir must
/// not render as "no deliverables", and per-record reads must be byte-capped
/// — so its semantics are now the single implementation). `only_session`
/// short-circuits by file name: sessions are stored as `<id>.json`, so a
/// single-session listing has no reason to read or parse the other records.
/// Reads disk truth only: deleted files are skipped; mtime/size come from fs
/// live; one physical path keeps only its newest row (same-mtime ties broken
/// by session_id, and row ordering carries the path leg, so the output is
/// independent of read_dir order).
pub fn deliverable_index(only_session: Option<&str>) -> Result<DeliverableIndex, String> {
    let sessions_dir = crate::platform::paths::sessions_root();
    let entries = match std::fs::read_dir(&sessions_dir) {
        Ok(entries) => entries,
        // A missing sessions root is a genuinely empty store, not a failure.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DeliverableIndex {
                rows: Vec::new(),
                skipped: Vec::new(),
            });
        }
        // Anything else (EACCES on a locked-down home, `sessions` shadowed
        // by a regular file) must not render as "no deliverables": the
        // `skipped_sessions` disclosure exists precisely so a consumer can
        // tell "empty" from "index incomplete" — an empty EVERYTHING is the
        // one shape that reads as a clean bill.
        Err(error) => {
            return Err(format!(
                "cannot read the sessions root {}: {error}",
                sessions_dir.display()
            ));
        }
    };
    let mut by_path: std::collections::HashMap<String, DeliverableItem> =
        std::collections::HashMap::new();
    let mut skipped: Vec<String> = Vec::new();
    // The over-cap skip disclosure, stated once: both the pre-read stat
    // probe below and the `RecordRead::TooLarge` arm (a record that grew
    // past the cap between the probe and the read) disclose the exact same
    // way. stderr notes for the human lane; the skipped list is the payload
    // a `--output json` consumer reads. The notes are best-effort writes —
    // Rust ignores SIGPIPE, so an `eprintln!` here would PANIC (exit 101)
    // when a human pipes stderr (`pinvou artifacts list 2>&1 | head`) and
    // the reader closes early; the CLI's `note!` documents this closed-pipe
    // contract, and this shared impl is library code on that call path
    // (round-9 review MAJOR), so the errors are ignored explicitly instead.
    let note_skip = |detail: std::fmt::Arguments<'_>, file: &Path| {
        use std::io::Write as _;
        let _ = std::io::stderr().write_fmt(format_args!(
            "[artifacts] list skips {} {detail}\n",
            file.display()
        ));
    };
    let skip_oversized = |file: &Path, stem: String, skipped: &mut Vec<String>| {
        note_skip(
            format_args!("(larger than the {MAX_LIST_SCAN_BYTES}-byte scan cap)"),
            file,
        );
        skipped.push(stem);
    };
    for entry in entries.flatten() {
        let file = entry.path();
        if !file.is_file() || file.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_owned();
        // Aux sessions carry no deliverables today, and the index is a
        // cross-session surface: an aux record parses identically here, so a
        // future artifact on an aux would surface its paths outside the aux
        // boundary (round-36 minor 4). Identity comes from the FILENAME, the
        // same derived-id rule as the retention orphan pass; the predicate
        // is the store's shared `is_aux_session_id`.
        if crate::features::sessions::is_aux_session_id(stem.as_str()) {
            continue;
        }
        if only_session.is_some_and(|session| session != stem.as_str()) {
            continue;
        }
        // A failed stat must NOT silently skip (round-9 review minor): the
        // CLI side this impl absorbed fell through to the capped read on a
        // stat failure, whose `RecordRead::Unreadable` arm discloses the
        // record in `skipped` — the "incomplete ≠ empty" contract covers
        // unreadable records, not just oversized ones. The over-cap probe
        // only fires on a good stat (a failed stat reads as len 0 there,
        // exactly like the CLI's `unwrap_or_default` probe), and a stamp
        // only exists for a good stat, so the no-stamp path reads
        // uncached — a stat that fails while the read succeeds (TOCTOU)
        // parses normally but is never cached under a fabricated stamp.
        let meta = std::fs::metadata(&file).ok();
        if meta
            .as_ref()
            .is_some_and(|m| m.is_file() && m.len() > MAX_LIST_SCAN_BYTES)
        {
            skip_oversized(&file, stem, &mut skipped);
            continue;
        }
        let stamp = meta.as_ref().map(FileStamp::of);
        let view = match stamp.and_then(|stamp| DV_VIEW_CACHE.get(&file, stamp)) {
            Some(view) => view,
            None => {
                let raw = match read_record_capped(&file, MAX_LIST_SCAN_BYTES) {
                    RecordRead::Text(raw) => raw,
                    RecordRead::TooLarge => {
                        // The record passed the stat probe but grew past
                        // the cap before this read landed; identical skip
                        // contract to the probe above.
                        skip_oversized(&file, stem, &mut skipped);
                        continue;
                    }
                    RecordRead::Unreadable => {
                        // Same "incomplete ≠ empty" disclosure as the
                        // oversize branch: an EACCES'd or mid-write record
                        // is dropped from the index, so an unreadable slice
                        // must surface in skipped_sessions instead of
                        // vanishing (round-36 review).
                        note_skip(format_args!("(unreadable)"), &file);
                        skipped.push(stem);
                        continue;
                    }
                };
                // A record without the literal `"artifacts"` key cannot
                // yield a row (a JSON object key must appear byte-for-byte),
                // so the common artifact-less well-formed record skips the
                // parse. A record carrying NEITHER key — or one truncated
                // after the metadata region — is corrupt in its early
                // region and must reach the parse so its not-a-record
                // disclosure lands (rounds 43/44/51); a well-formed JSON
                // document always ends with `}`.
                if !raw.contains("\"artifacts\"")
                    && raw.contains("\"metadata\"")
                    && raw.trim_end().ends_with('}')
                {
                    continue;
                }
                let Ok(view) = serde_json::from_str::<DvSessionView>(&raw) else {
                    // Corrupt JSON is the same undecidable slice:
                    // disclosed, then skipped.
                    note_skip(format_args!("(not a valid session record)"), &file);
                    skipped.push(stem);
                    continue;
                };
                if let Some(stamp) = stamp {
                    DV_VIEW_CACHE.insert(&file, stamp, view.clone());
                }
                view
            }
        };

        for art in view.artifacts {
            let p = &art.storage_path;
            // A per-artifact stat failure (file vanished between readdir and
            // metadata, or an unreadable path) skips just that row — unlike
            // the whole-record skips above, it is silent by design: the
            // deliverable's own row would only mislead while its siblings
            // stay listable.
            let Ok(meta) = std::fs::metadata(p) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let name = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !DELIVERABLE_EXTS.contains(&ext.as_str()) {
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let path = p.to_string_lossy().to_string();
            let item = DeliverableItem {
                name,
                path: path.clone(),
                ext: ext.clone(),
                category: deliverable_category(&ext).to_string(),
                session_id: view.metadata.id.clone(),
                source: view.metadata.title.clone(),
                mtime,
                size: if meta.len() > 0 {
                    meta.len()
                } else {
                    art.byte_size
                },
            };
            by_path
                .entry(path)
                .and_modify(|cur| {
                    // The equal-mtime tie breaks on the session id so
                    // `--session` filtering is deterministic — read_dir
                    // order is unspecified, and with a bare `>=` which row
                    // survived an equal-mtime collision flipped between
                    // runs (round-50 review).
                    if item.mtime > cur.mtime
                        || (item.mtime == cur.mtime && item.session_id >= cur.session_id)
                    {
                        *cur = item.clone();
                    }
                })
                .or_insert(item);
        }
    }

    let mut out: Vec<DeliverableItem> = by_path.into_values().collect();
    // (mtime desc, name) ties (same-second same-name deliverables in
    // different sessions) kept HashMap-random relative order; the final
    // path leg — unique per row, the dedup map's key — makes the order
    // deterministic (round-51 review).
    out.sort_by(|a, b| {
        b.mtime
            .cmp(&a.mtime)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
    skipped.sort();
    Ok(DeliverableIndex { rows: out, skipped })
}

/// Outcome of a capped session-record read for [`deliverable_index`].
#[derive(Debug)]
enum RecordRead {
    /// The record ends within the cap and decodes as UTF-8.
    Text(String),
    /// The record does not end within the cap; the read was cut off before
    /// EOF instead of slurping an unbounded file.
    TooLarge,
    /// The record is unreadable or not valid UTF-8 — the silent skip the
    /// previous uncapped `read_to_string` took on the same conditions.
    Unreadable,
}

/// Reads at most `cap + 1` bytes of `path` and decodes it as UTF-8.
///
/// The stat probe in [`deliverable_index`] only sees the size at stat time,
/// so a record under the cap at probe time that grows past it before the
/// read lands must not be slurped whole by an uncapped read. The take
/// allows one extra byte so a record of exactly `cap` bytes still reaches
/// EOF inside the read and counts as a full read; only a strictly larger
/// file reports [`RecordRead::TooLarge`].
fn read_record_capped(path: &Path, cap: u64) -> RecordRead {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else {
        return RecordRead::Unreadable;
    };
    let mut taken = file.take(cap.saturating_add(1));
    let mut raw = Vec::new();
    if taken.read_to_end(&mut raw).is_err() {
        return RecordRead::Unreadable;
    }
    if raw.len() as u64 > cap {
        return RecordRead::TooLarge;
    }
    match String::from_utf8(raw) {
        Ok(text) => RecordRead::Text(text),
        Err(_) => RecordRead::Unreadable,
    }
}

/// GUI command shell: whole-store index (no session filter). An undecidable
/// read_dir failure propagates as-is — rendering it as "no deliverables" is
/// exactly the false clean bill this contract fixes. Forwards the whole
/// [`DeliverableIndex`] (rows + skipped, round-4 review): truncating skipped
/// at the shell makes an over-cap/unreadable record's deliverables vanish
/// from the panel as a successful empty result; the view renders the
/// "incomplete" notice from skipped.
pub(crate) fn list_deliverable_index_impl() -> Result<DeliverableIndex, String> {
    deliverable_index(None)
}

#[cfg(test)]
mod capped_record_read_tests {
    use super::{RecordRead, read_record_capped};

    /// Boundary math of the capped record read: a record of exactly the cap
    /// still reaches EOF inside the read (the `take(cap + 1)` headroom), one
    /// byte past the cap reports `TooLarge` instead of returning truncated
    /// text, and non-UTF-8 content keeps the silent-skip shape
    /// (`Unreadable`) the uncapped `read_to_string` error path had.
    /// Round-3 review: the CLI mirror's boundary unit test was deleted with
    /// the mirror and the integration contract only exercises the stat-probe
    /// skip, so the math is pinned here beside the shared implementation.
    #[test]
    fn capped_record_read_cuts_off_past_the_cap() {
        let dir = tempfile::tempdir().expect("scratch dir");
        let record = dir.path().join("s-1.json");
        let body = r#"{"metadata":{"id":"s-1"}}"#;
        std::fs::write(&record, body).expect("seed record");
        let size = std::fs::metadata(&record).expect("stat record").len();

        match read_record_capped(&record, size) {
            RecordRead::Text(raw) => assert_eq!(raw, body),
            other => panic!("a record of exactly the cap must read whole, got {other:?}"),
        }
        match read_record_capped(&record, size - 1) {
            RecordRead::TooLarge => {}
            other => panic!("a record past the cap must read as TooLarge, got {other:?}"),
        }
        std::fs::write(&record, b"\xff\xfe not utf8").expect("seed binary record");
        assert!(matches!(
            read_record_capped(&record, size),
            RecordRead::Unreadable
        ));
    }
}

#[cfg(test)]
pub(crate) mod gui_index_contract_tests {
    // The skipped-disclosure GUI contract itself is asserted (over the same
    // seed_index_fixture shape) by the v2 command-shell tests in
    // app::commands::protocol_tests; this module now keeps only the fixture
    // helpers those shell tests consume (the deliverables-side GUI index
    // test was removed as contained by them).
    use super::MAX_LIST_SCAN_BYTES;
    use crate::platform::paths::tests::{ENV_LOCK, EnvVarGuard};

    /// Seeds one scannable session (one real deliverable on disk) plus one
    /// over-cap record under `<home>/sessions`, the fixture shape for the
    /// command-shell contract in app::commands::protocol_tests.
    pub(crate) fn seed_index_fixture(home: &std::path::Path) {
        let sessions = home.join("sessions");
        std::fs::create_dir_all(&sessions).expect("seed sessions root");

        // A scannable session tracking one real deliverable on disk.
        let deliverable = home.join("report.md");
        std::fs::write(&deliverable, "# Report\n").expect("seed deliverable");
        let ok_record = format!(
            r#"{{"metadata":{{"id":"s-ok","title":"OK chat"}},"artifacts":[{{"storage_path":{}}}]}}"#,
            serde_json::to_string(&deliverable.to_string_lossy()).expect("json path")
        );
        std::fs::write(sessions.join("s-ok.json"), ok_record).expect("seed ok record");

        // An over-cap record: the stat probe skips it before any read.
        // `set_len` keeps the fixture sparse — the probe only stats. Its
        // deliverables (if any) are the ones the GUI must disclose losing.
        let huge = std::fs::File::create(sessions.join("s-huge.json")).expect("seed huge record");
        huge.set_len(MAX_LIST_SCAN_BYTES + 1)
            .expect("spoil fixture");
        drop(huge);
    }

    /// Holds ENV_LOCK for setup AND for the caller's asserts (parallel
    /// env-writing tests must not repoint PINVOU3_HOME mid-scan), seeds the
    /// fixture under a scratch PINVOU3_HOME, and returns the lock guard, the
    /// tempdir, and the env guard whose Drop restores the host env.
    /// Destructure in that order: Rust drops bindings in reverse, so the
    /// env restore (including on assertion unwind) always runs while the
    /// lock is still held, and the tempdir is removed before the lock is
    /// released — the lock is the last guard dropped.
    pub(crate) fn run_in_scratch_home() -> (
        std::sync::MutexGuard<'static, ()>,
        tempfile::TempDir,
        EnvVarGuard,
    ) {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let home = tempfile::tempdir().expect("scratch home");
        let host = EnvVarGuard::capture(&["PINVOU3_HOME"]);
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env
        // writes are serialized against the other env-writing tests.
        unsafe { std::env::set_var("PINVOU3_HOME", home.path()) };
        seed_index_fixture(home.path());
        (lock, home, host)
    }
}
