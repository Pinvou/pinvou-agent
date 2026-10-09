//! `knowledge` family: knowledge-base surface mirroring the GUI commands in
//! `pinvou3-app/src-tauri/src/app/commands/knowledge.rs` (kb_* and the
//! session-mount commands) plus `remote_knowledge` and `shared_knowledge_host`.
//!
//! `features::knowledge`, `features::remote_knowledge` and
//! `features::shared_knowledge_host` are `pub` in the app crate, so the CLI
//! constructs the feature types directly (no Tauri host for the storage
//! paths):
//! - `KnowledgeService::new_without_recovery(db_path)` is a standalone
//!   constructor over `~/.pinvou3/knowledge/index.db` (`default_db_path()`
//!   honours `PINVOU3_HOME`). Recovery of interrupted imports is NEVER run
//!   by the CLI (see below) — the recovering `KnowledgeService::new` is
//!   deliberately not constructed here, so a one-shot invocation cannot
//!   wedge a live desktop app's import.
//! - scan start/status → `KnowledgeService::{start_scan, status}`;
//!   `--root` omitted defaults to the user home like `kb_start_scan`. Scan
//!   cancel is a stable exit-1 refusal (see `scan_cancel`): the in-memory
//!   scan state is process-local, so a one-shot invocation can never have a
//!   scan of its own to signal.
//! - collections list/create/update/delete → `KnowledgeService::l1()`
//!   (`L1Store` CRUD). Delete mirrors GUI `kb_collection_delete`:
//!   named-`cancel_index_job` of the collection's live job (read once, so
//!   a job flipping terminal mid-delete can never re-target another
//!   collection's import), `delete_collection` (which cascades the
//!   collection's import-job rows), and the mount sweep stays with the
//!   desktop app's own surface (`SessionStore` mounts live in per-process
//!   memory, so no CLI-side sweep exists — and no session store is booted).
//! - add-sources → `KnowledgeService::start_index`, then the invocation
//!   BLOCKS until the import job reaches a terminal phase (see "One-shot
//!   semantics" below): the started state, the wait and the final state all
//!   happen in one command, like `scan start`.
//! - documents → `L1Store::{list_documents, remove_document}` (`--limit`
//!   omitted maps to the GUI's 0 = default page of 500).
//! - index status/cancel/resume/retry/failed → `KnowledgeService::
//!   {index_status, cancel_index_job, resume_index, retry_index_item,
//!   failed_index_files}` (`cancel_index_job` is the CLI-facing id-taking
//!   cancel, the same shape `interrupt_index` uses); `--limit`
//!   for failed files defaults to the GUI page size 50. `index status`
//!   without an id reports the GUI's latest job (`kb_index_status`
//!   semantics); with an id it reports THAT job through
//!   `KnowledgeService::index_job_state`, because the latest-job ordering
//!   ranks `cancelled` last and would answer with an older `done_with_errors`
//!   job instead. `index cancel` reports the cancelled job through the same
//!   per-job read for exactly that reason.
//!   `index cancel <job-id>` cancels exactly the named job through the
//!   id-taking transition — never a re-derived "latest" job, which could
//!   race a desktop-app import — while `resume`/`retry <job-id>` validate the
//!   named id against the latest job and additionally refuse while the latest
//!   job is still `running`: the CLI never runs the GUI's boot recovery of
//!   interrupted jobs (see "One-shot semantics"), so every command must fail
//!   honestly on state it must not touch.
//! - stats/type-counts → the headless `KnowledgeService::{stats, type_counts}`
//!   (the same store calls as `kb_stats`/`kb_type_counts`, synchronously).
//! - search → the headless `KnowledgeService::search` (`kb_search` semantics:
//!   the free-text query goes through the same NL-rule merge, so
//!   "pdf from last week" becomes an ext + mtime filter plus residual text).
//!   `--after`/`--before` take UTC `YYYY-MM-DD` dates (the GUI frontend sends
//!   epoch seconds directly); `--limit` omitted keeps the store's default
//!   page of 200. `--before` is exclusive: it keeps files with mtime before
//!   the START of the named day, so `--before 2026-09-01` excludes
//!   2026-09-01 itself.
//! - mounts/mount/unmount → honest refusal (`knowledge_*_requires_product_host`):
//!   mounted collections live in the desktop app's per-process memory
//!   (`features::sessions::mode_state`, deliberately not persisted), so a
//!   one-shot CLI process can neither observe nor durably mutate them. The
//!   refusal is returned before any `SessionStore::boot()`, which enforces
//!   the 50-sessions-per-kind retention and would evict the user's oldest
//!   sessions on the way to an answer that can never be anything else.
//! - remote connections/collections/search → `RemoteKnowledgeService` over
//!   `~/.pinvou3/knowledge/remote-connections.json`. These are async network
//!   client calls, so they run on the bare async host
//!   (`pinvou3_lib::headless_bridge::run_bare_host`: rustls/env/runtime, no
//!   Tauri context, no session-store boot, no display — usable on headless
//!   Linux); zero configured connections answers offline without even that.
//! - remote probe → `features::remote_knowledge::probe_private_identity`
//!   (the TLS-pinned identity handshake behind the GUI's
//!   `remote_kb_probe_private_endpoint`, re-exported as a free function;
//!   `RemoteKnowledgeProbe` was already public via `request_join_confirmed`,
//!   so no new types entered the app crate's surface). Async network call →
//!   the same bare async host as the other remote lanes.
//! - host status → `features::shared_knowledge_host::status()` (async) via
//!   the same host runtime; install/upgrade/backup stay GUI/host-bound.
//!
//! Still behind an upstream boundary (stable `knowledge_backend_unavailable`
//! error, see `model_download_unavailable`):
//! - model download: the full download + verify + deploy orchestration is
//!   `kb_model_download` in features/knowledge/model_download.rs, built on
//!   private statics (`DOWNLOADING`/`CANCEL`/`MODEL_LOAD`) and private
//!   helpers (`configured_model_dir`, `uses_external_model_dir`,
//!   `deploy_validated_model`) that the parent `mod.rs` — the only place a
//!   headless wrapper could be added without touching that file — cannot
//!   reach (Rust visibility: parent modules cannot see child-module
//!   privates). A mod.rs re-implementation would fork the downloading/cancel
//!   state machine (`kb_model_cancel` could neither cancel nor observe a CLI
//!   download, `model status.downloading` could not see it) and would drop
//!   the contract-mandated candidate verification through a real ONNX
//!   inference session before the atomic deploy (the caller owns actually
//!   loading the candidate model after the deploy reports success).
//!   `model status` therefore reports what a one-shot CLI
//!   process can know (on-disk completeness mirror + `semantic_ready`), and
//!   `model cancel` refuses honestly: the cancel flag is process-local to
//!   the app's orchestration and a CLI process never has a download of its
//!   own in flight, so the flag it could set is read by nobody.
//!
//! One-shot semantics: imports run in-process; a one-shot process kills its
//! threads at exit, so `add-sources`/`index resume`/`index retry` cannot be
//! fire-and-forget — `main` exits right after printing, which reaped the
//! import thread mid-work in earlier rounds and stranded DB rows flagged
//! `running` with no owner (a live GUI polls that phantom "Indexing" forever,
//! and cannot even resume it: `resume` requires `interrupted`). Those three
//! commands therefore own their job through completion, exactly like
//! `scan start`: they print nothing until the job reaches a terminal phase
//! (done | done_with_errors | interrupted | cancelled) under a no-progress
//! liveness bound, then report the final state. A job that stops advancing
//! (wedge, stalled IO past the bound) is interrupted through the feature
//! layer's `interrupt_index` — `ImportJobStore::interrupt` returns every
//! claimed item to `pending` and flips the job to `interrupted` on disk —
//! so the next `index resume` continues it without a desktop-app boot in
//! between; the command exits 1 with a report that names the last known
//! state (and the honest remedy if even the interrupt could not land).
//! No signal handler exists on this lane (`main` is a synchronous
//! entry point; no windows/children are spawned here), so SIGINT kills both
//! the process and the import thread without a cleanup hook: the job is
//! likewise recoverable — it stays `running` on disk until the next
//! process's recovery run, which is the GUI's boot path, not a CLI one.
//!
//! Boot recovery (the GUI's startup reconciliation that re-runs a crashed
//! process's `interrupted` import) is NEVER run by the CLI. `main` exits
//! while the import thread is very much alive, so from the job store's
//! point of view recovery here is not crash-cleanup — it is an unprovoked
//! `UPDATE knowledge_import_jobs SET state='interrupted'` against a job that
//! might be driven by a live desktop-app process on the same DB. Recovery
//! stays with the processes that own the store's lifecycle
//! ([`open_service`] opens without it) and with `resume`/`retry`'s own
//! state transitions for the CLI's own stranded jobs.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::support::{render, require_yes, sandbox_home, success};
use crate::{CliError, CliOutcome, ExitCode, OutputMode};
use pinvou3_lib::features::knowledge::model_download::MODEL_VERSION;
use pinvou3_lib::features::knowledge::{
    IndexState, KnowledgeService, ScanState, SearchQueryDto, default_db_path, model_dir,
};
#[cfg(feature = "product-backend")]
use pinvou3_lib::features::remote_knowledge::RemoteKnowledgeService;

const USAGE: &str = "usage: pinvou knowledge <scan|stats|type-counts|collections|documents|index|search|model|mounts|mount|unmount|remote|host>";
const SCAN_USAGE: &str = "usage: pinvou knowledge scan <start [--root DIR]|status|cancel>";
const COLLECTIONS_USAGE: &str =
    "usage: pinvou knowledge collections <list|create|update|delete|add-sources>";
const DOCUMENTS_USAGE: &str = "usage: pinvou knowledge documents <collection-id> [--limit N] | pinvou knowledge documents remove <doc-id> --yes";
const INDEX_USAGE: &str = "usage: pinvou knowledge index <status [job-id]|cancel job-id|resume job-id|retry job-id item-id|failed job-id [--offset N --limit N]> (failed page: --offset defaults to 0, --limit to 50)";
const MODEL_USAGE: &str = "usage: pinvou knowledge model <status|download|cancel> (download: needs network and the desktop model host; long-running)";
const REMOTE_USAGE: &str = "usage: pinvou knowledge remote <connections|probe URL|collections|search <collection> <query>>";
const HOST_USAGE: &str = "usage: pinvou knowledge host status";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnowledgeCommand {
    ScanStart {
        root: Option<PathBuf>,
    },
    ScanStatus,
    ScanCancel,
    Stats,
    TypeCounts,
    CollectionsList,
    CollectionsCreate {
        name: String,
        category: Option<String>,
        description: Option<String>,
    },
    CollectionsUpdate {
        id: i64,
        name: Option<String>,
        category: Option<String>,
        description: Option<String>,
    },
    CollectionsDelete {
        id: i64,
        yes: bool,
    },
    CollectionsAddSources {
        id: i64,
        paths: Vec<PathBuf>,
    },
    Documents {
        collection_id: i64,
        limit: Option<usize>,
    },
    DocumentsRemove {
        doc_id: i64,
        yes: bool,
    },
    IndexStatus {
        job_id: Option<String>,
    },
    IndexCancel {
        job_id: String,
    },
    IndexResume {
        job_id: String,
    },
    IndexRetry {
        job_id: String,
        item_id: i64,
    },
    IndexFailed {
        job_id: String,
        offset: usize,
        limit: Option<usize>,
    },
    Search {
        query: String,
        limit: Option<usize>,
        ext: Option<String>,
        after: Option<String>,
        before: Option<String>,
    },
    ModelStatus,
    ModelDownload,
    ModelCancel,
    Mounts {
        session_id: String,
    },
    Mount {
        session_id: String,
        collection_id: i64,
    },
    Unmount {
        session_id: String,
        collection_id: i64,
    },
    RemoteConnections,
    RemoteProbe {
        url: String,
    },
    RemoteCollections,
    RemoteSearch {
        collection: String,
        query: String,
    },
    HostStatus,
}

/// Flags that carry a value, per subcommand.
const SCAN_START_OPTIONS: &[&str] = &["--root"];
const CREATE_OPTIONS: &[&str] = &["--name", "--category", "--description"];
const UPDATE_OPTIONS: &[&str] = &["--name", "--category", "--description"];
const DOCUMENT_OPTIONS: &[&str] = &["--limit"];
const INDEX_FAILED_OPTIONS: &[&str] = &["--offset", "--limit"];
const SEARCH_OPTIONS: &[&str] = &["--limit", "--ext", "--after", "--before"];

/// Boolean (valueless) flags, per subcommand.
const DELETE_FLAGS: &[&str] = &["--yes"];

pub fn parse(values: &[String]) -> Result<KnowledgeCommand, CliError> {
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(USAGE))?;
    let rest = &values[2..];
    match subcommand.as_str() {
        "scan" => {
            let action = rest.first().ok_or_else(|| CliError::usage(SCAN_USAGE))?;
            match action.as_str() {
                "start" => {
                    let (options, _) = parse_flags(&rest[1..], SCAN_START_OPTIONS, &[])?;
                    Ok(KnowledgeCommand::ScanStart {
                        root: option(&options, "--root").map(PathBuf::from),
                    })
                }
                "status" => {
                    no_options(&rest[1..], "scan status")?;
                    Ok(KnowledgeCommand::ScanStatus)
                }
                "cancel" => {
                    no_options(&rest[1..], "scan cancel")?;
                    Ok(KnowledgeCommand::ScanCancel)
                }
                _ => Err(CliError::usage(SCAN_USAGE)),
            }
        }
        "stats" => {
            no_options(rest, "stats")?;
            Ok(KnowledgeCommand::Stats)
        }
        "type-counts" => {
            no_options(rest, "type-counts")?;
            Ok(KnowledgeCommand::TypeCounts)
        }
        "collections" => {
            let action = rest
                .first()
                .ok_or_else(|| CliError::usage(COLLECTIONS_USAGE))?;
            match action.as_str() {
                "list" => {
                    no_options(&rest[1..], "collections list")?;
                    Ok(KnowledgeCommand::CollectionsList)
                }
                "create" => {
                    let (options, _) = parse_flags(&rest[1..], CREATE_OPTIONS, &[])?;
                    let name = option(&options, "--name")
                        .ok_or_else(|| {
                            CliError::usage("knowledge collections create requires --name")
                        })?
                        .to_owned();
                    Ok(KnowledgeCommand::CollectionsCreate {
                        name,
                        category: option(&options, "--category").map(str::to_owned),
                        description: option(&options, "--description").map(str::to_owned),
                    })
                }
                "update" => {
                    let (head, tail) = positionals(&rest[1..]);
                    let id = parse_id(exactly_one(head, "a collection id")?, "collection")?;
                    let (options, _) = parse_flags(tail, UPDATE_OPTIONS, &[])?;
                    let name = option(&options, "--name").map(str::to_owned);
                    let category = option(&options, "--category").map(str::to_owned);
                    let description = option(&options, "--description").map(str::to_owned);
                    if name.is_none() && category.is_none() && description.is_none() {
                        return Err(CliError::usage(
                            "knowledge collections update requires at least one of --name, --category or --description",
                        ));
                    }
                    Ok(KnowledgeCommand::CollectionsUpdate {
                        id,
                        name,
                        category,
                        description,
                    })
                }
                "delete" => {
                    let (head, tail) = positionals(&rest[1..]);
                    let id = parse_id(exactly_one(head, "a collection id")?, "collection")?;
                    let (_, flags) = parse_flags(tail, &[], DELETE_FLAGS)?;
                    Ok(KnowledgeCommand::CollectionsDelete {
                        id,
                        yes: flags.contains(&"--yes"),
                    })
                }
                "add-sources" => {
                    let (head, tail) = positionals(&rest[1..]);
                    if !tail.is_empty() {
                        return Err(CliError::usage(
                            "knowledge collections add-sources accepts no options",
                        ));
                    }
                    let id = parse_id(head.first().ok_or_else(|| {
                        CliError::usage(
                            "knowledge collections add-sources requires a collection id and at least one path",
                        )
                    })?, "collection")?;
                    let paths = head[1..]
                        .iter()
                        .map(|path| PathBuf::from(path))
                        .collect::<Vec<_>>();
                    if paths.is_empty() {
                        return Err(CliError::usage(
                            "knowledge collections add-sources requires at least one path",
                        ));
                    }
                    Ok(KnowledgeCommand::CollectionsAddSources { id, paths })
                }
                _ => Err(CliError::usage(COLLECTIONS_USAGE)),
            }
        }
        "documents" => {
            let action = rest
                .first()
                .ok_or_else(|| CliError::usage(DOCUMENTS_USAGE))?;
            if action == "remove" {
                let (head, tail) = positionals(&rest[1..]);
                let doc_id = parse_id(exactly_one(head, "a document id")?, "document")?;
                let (_, flags) = parse_flags(tail, &[], DELETE_FLAGS)?;
                Ok(KnowledgeCommand::DocumentsRemove {
                    doc_id,
                    yes: flags.contains(&"--yes"),
                })
            } else {
                let collection_id = parse_id(action, "collection")?;
                let (options, _) = parse_flags(&rest[1..], DOCUMENT_OPTIONS, &[])?;
                Ok(KnowledgeCommand::Documents {
                    collection_id,
                    limit: parse_positive(&options, "--limit")?,
                })
            }
        }
        "index" => {
            let action = rest.first().ok_or_else(|| CliError::usage(INDEX_USAGE))?;
            match action.as_str() {
                "status" => {
                    let (head, tail) = positionals(&rest[1..]);
                    no_options(tail, "index status")?;
                    Ok(KnowledgeCommand::IndexStatus {
                        job_id: optional_single(
                            head,
                            "an index job id (or none for the latest job)",
                        )?
                        .map(|value| value.to_owned()),
                    })
                }
                "cancel" | "resume" => {
                    let (head, tail) = positionals(&rest[1..]);
                    no_options(tail, &format!("index {action}"))?;
                    let job_id = exactly_one(head, "an index job id")?.to_owned();
                    if action == "cancel" {
                        Ok(KnowledgeCommand::IndexCancel { job_id })
                    } else {
                        Ok(KnowledgeCommand::IndexResume { job_id })
                    }
                }
                "retry" => {
                    let (head, tail) = positionals(&rest[1..]);
                    no_options(tail, "index retry")?;
                    let (job, item) = exactly_two(head, "an index job id and an item id")?;
                    Ok(KnowledgeCommand::IndexRetry {
                        job_id: job.to_owned(),
                        item_id: parse_id(item, "item")?,
                    })
                }
                "failed" => {
                    let (head, tail) = positionals(&rest[1..]);
                    let job_id = exactly_one(head, "an index job id")?.to_owned();
                    let (options, _) = parse_flags(tail, INDEX_FAILED_OPTIONS, &[])?;
                    let offset = match option(&options, "--offset") {
                        None => 0,
                        Some(value) => parse_non_negative(value, "--offset")?,
                    };
                    let limit = parse_positive(&options, "--limit")?;
                    Ok(KnowledgeCommand::IndexFailed {
                        job_id,
                        offset,
                        limit,
                    })
                }
                _ => Err(CliError::usage(INDEX_USAGE)),
            }
        }
        "search" => {
            let (head, tail) = positionals(rest);
            let query = head.join(" ");
            if query.trim().is_empty() {
                return Err(CliError::usage("knowledge search requires a query"));
            }
            let (options, _) = parse_flags(tail, SEARCH_OPTIONS, &[])?;
            Ok(KnowledgeCommand::Search {
                query,
                limit: parse_positive(&options, "--limit")?,
                ext: option(&options, "--ext").map(str::to_owned),
                after: option(&options, "--after").map(str::to_owned),
                before: option(&options, "--before").map(str::to_owned),
            })
        }
        "model" => {
            let action = rest.first().ok_or_else(|| CliError::usage(MODEL_USAGE))?;
            match action.as_str() {
                "status" => {
                    no_options(&rest[1..], "model status")?;
                    Ok(KnowledgeCommand::ModelStatus)
                }
                "download" => {
                    no_options(&rest[1..], "model download")?;
                    Ok(KnowledgeCommand::ModelDownload)
                }
                "cancel" => {
                    no_options(&rest[1..], "model cancel")?;
                    Ok(KnowledgeCommand::ModelCancel)
                }
                _ => Err(CliError::usage(MODEL_USAGE)),
            }
        }
        "mounts" => {
            let (head, tail) = positionals(rest);
            no_options(tail, "mounts")?;
            Ok(KnowledgeCommand::Mounts {
                session_id: require_session_id(exactly_one(head, "a session id")?)?,
            })
        }
        "mount" | "unmount" => {
            let (head, tail) = positionals(rest);
            no_options(tail, subcommand)?;
            let (session, collection) = exactly_two(head, "a session id and a collection id")?;
            let session_id = require_session_id(session)?;
            let collection_id = parse_id(collection, "collection")?;
            if subcommand == "mount" {
                Ok(KnowledgeCommand::Mount {
                    session_id,
                    collection_id,
                })
            } else {
                Ok(KnowledgeCommand::Unmount {
                    session_id,
                    collection_id,
                })
            }
        }
        "remote" => {
            let action = rest.first().ok_or_else(|| CliError::usage(REMOTE_USAGE))?;
            match action.as_str() {
                "connections" => {
                    no_options(&rest[1..], "remote connections")?;
                    Ok(KnowledgeCommand::RemoteConnections)
                }
                "probe" => {
                    let (head, tail) = positionals(&rest[1..]);
                    no_options(tail, "remote probe")?;
                    Ok(KnowledgeCommand::RemoteProbe {
                        url: exactly_one(head, "a URL")?.to_owned(),
                    })
                }
                "collections" => {
                    no_options(&rest[1..], "remote collections")?;
                    Ok(KnowledgeCommand::RemoteCollections)
                }
                "search" => {
                    let (head, tail) = positionals(&rest[1..]);
                    no_options(tail, "remote search")?;
                    let collection = head
                        .first()
                        .ok_or_else(|| {
                            CliError::usage(
                                "knowledge remote search requires a collection and a query",
                            )
                        })?
                        .to_owned();
                    let query = head[1..].join(" ");
                    if query.trim().is_empty() {
                        return Err(CliError::usage("knowledge remote search requires a query"));
                    }
                    Ok(KnowledgeCommand::RemoteSearch { collection, query })
                }
                _ => Err(CliError::usage(REMOTE_USAGE)),
            }
        }
        "host" => {
            let action = rest.first().ok_or_else(|| CliError::usage(HOST_USAGE))?;
            if action != "status" {
                return Err(CliError::usage(HOST_USAGE));
            }
            no_options(&rest[1..], "host status")?;
            Ok(KnowledgeCommand::HostStatus)
        }
        _ => Err(CliError::usage(USAGE)),
    }
}

/// Splits leading positional tokens off `values` before the first option
/// (a token starting with `--`). Negative numeric ids (`-3`) stay positional.
fn positionals(values: &[String]) -> (&[String], &[String]) {
    let stop = values
        .iter()
        .position(|token| token.starts_with("--"))
        .unwrap_or(values.len());
    values.split_at(stop)
}

fn no_options(values: &[String], context: &str) -> Result<(), CliError> {
    if values.is_empty() {
        Ok(())
    } else {
        Err(CliError::usage(format!(
            "knowledge {context} accepts no options (got {})",
            values.join(" ")
        )))
    }
}

fn exactly_one<'a>(head: &'a [String], what: &str) -> Result<&'a String, CliError> {
    match head {
        [only] => Ok(only),
        [] => Err(CliError::usage(format!("knowledge requires {what}"))),
        _ => Err(CliError::usage(format!(
            "knowledge {what} must be a single token (got {})",
            head.join(" ")
        ))),
    }
}

fn exactly_two<'a>(head: &'a [String], what: &str) -> Result<(&'a String, &'a String), CliError> {
    match head {
        [first, second] => Ok((first, second)),
        [] => Err(CliError::usage(format!("knowledge requires {what}"))),
        [one] => Err(CliError::usage(format!(
            "knowledge requires {what} (got only {one})"
        ))),
        _ => Err(CliError::usage(format!(
            "knowledge {what} must be two tokens (got {})",
            head.join(" ")
        ))),
    }
}

fn optional_single<'a>(head: &'a [String], what: &str) -> Result<Option<&'a String>, CliError> {
    match head {
        [] => Ok(None),
        [only] => Ok(Some(only)),
        _ => Err(CliError::usage(format!(
            "knowledge {what} must be a single token (got {})",
            head.join(" ")
        ))),
    }
}

fn require_session_id(value: &str) -> Result<String, CliError> {
    // Same charset gate as every other session-id family: an id the store
    // would reject is a usage error (exit 2), not a host failure, and a
    // traversal-shaped id is refused before any path is derived from it.
    if !crate::support::valid_session_id(value) {
        return Err(CliError::usage(
            "knowledge requires a valid session id ([A-Za-z0-9_-])",
        ));
    }
    Ok(value.to_owned())
}

fn parse_id(value: &str, label: &str) -> Result<i64, CliError> {
    let id = value.parse::<i64>().map_err(|_| {
        CliError::usage(format!(
            "knowledge {label} id must be an integer (got {value})"
        ))
    })?;
    // Non-positive ids are GUI-internal sentinels (id <= 0 lists across all
    // collections in some internal helpers); the CLI only addresses real
    // collections.
    if id <= 0 {
        return Err(CliError::usage(format!(
            "knowledge {label} id must be a positive integer (got {value})"
        )));
    }
    Ok(id)
}

/// Shared implementation in `support::parse_family_flags`; `family`
/// only names this family in error messages.
fn parse_flags<'a>(
    values: &'a [String],
    value_flags: &[&str],
    boolean_flags: &[&str],
) -> Result<(Vec<(&'a str, &'a str)>, Vec<&'a str>), CliError> {
    crate::support::parse_family_flags(values, value_flags, boolean_flags, "knowledge")
}

fn option<'a>(options: &'a [(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    crate::support::family_option(options, name)
}

fn parse_positive(options: &[(&str, &str)], name: &str) -> Result<Option<usize>, CliError> {
    crate::support::parse_family_positive::<usize>(options, name, "knowledge")
}

fn parse_non_negative(value: &str, name: &str) -> Result<usize, CliError> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| CliError::usage(format!("knowledge {name} must be a non-negative integer")))?;
    // The value lands upstream as a SQLite i64 (e.g. `index failed --offset`);
    // a larger usize would wrap negative in the cast and silently serve page
    // 0 again. Rejected as usage, like every other out-of-range numeric flag
    // in the family, rather than answered with a misleading empty page.
    if parsed > i64::MAX as usize {
        return Err(CliError::usage(format!(
            "knowledge {name} must be at most {max} (got {value})",
            max = i64::MAX
        )));
    }
    Ok(parsed)
}

/// Stable error for `model download`: the feature orchestration and its
/// cancel/downloading state live behind model_download.rs privates that the
/// parent module cannot reach, and the download path requires the app
/// process's model loader (see module docs for the full blocker).
fn model_download_unavailable() -> CliError {
    CliError::failed(
        "knowledge_backend_unavailable: model download needs the download + verify + deploy \
         orchestration in features/knowledge/model_download.rs::kb_model_download, which is \
         built on private statics (DOWNLOADING/CANCEL/MODEL_LOAD) and private helpers \
         (configured_model_dir, uses_external_model_dir, deploy_validated_model) that the \
         parent mod.rs cannot reach, so a headless wrapper cannot reuse or share the cancel \
         state (kb_model_cancel could not cancel it, model status.downloading could not see \
         it) and would drop the candidate verification through a real ONNX inference session \
         required before the atomic deploy; downloads therefore run in the desktop app process",
    )
}

/// Constructs the per-invocation `KnowledgeService` over
/// `~/.pinvou3/knowledge/index.db` (temp-`PINVOU3_HOME` aware) WITHOUT the
/// GUI's boot-time recovery of interrupted imports.
///
/// No command in this family ever runs that recovery: it is the *desktop
/// app's* crash-handler (a dead process's `running` jobs cannot still be
/// executing, so it safely flips them to `interrupted`/resumable — plus
/// transaction-safe re-queueing of in-flight staged chunks — on every boot).
/// A one-shot CLI process exits while its import threads are still live, so
/// running it here is not crash cleanup but an unprovoked flip of jobs from
/// `running` to `interrupted` — including a job a live desktop-app process is
/// executing on the same database right now, for which there is no
/// cross-process owner heartbeat in the job store. Recovery belongs to the
/// processes that own the store's lifecycle, plus the jobs' own
/// `resume`/`retry` state transitions.
fn open_service() -> Result<KnowledgeService, CliError> {
    sandbox_home()?;
    let db = default_db_path();
    KnowledgeService::new_without_recovery(&db).map_err(|error| {
        CliError::failed(format!(
            "knowledge index store unavailable at {}: {error}",
            db.display()
        ))
    })
}

/// Maps an upstream `Result<_, String>` error onto the failed exit code with
/// the operation as context prefix.
fn feature_error(operation: &str, error: impl std::fmt::Display) -> CliError {
    CliError::failed(format!("knowledge {operation}: {error}"))
}

pub fn execute(command: KnowledgeCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    match command {
        KnowledgeCommand::Mounts { session_id } => mounts(&session_id, output),
        KnowledgeCommand::Mount {
            session_id,
            collection_id,
        } => mount(&session_id, collection_id, output),
        KnowledgeCommand::Unmount {
            session_id,
            collection_id,
        } => unmount(&session_id, collection_id, output),
        KnowledgeCommand::CollectionsDelete { id, yes } => {
            require_yes(yes)?;
            collections_delete(id, output)
        }
        KnowledgeCommand::DocumentsRemove { doc_id, yes } => {
            require_yes(yes)?;
            documents_remove(doc_id, output)
        }
        KnowledgeCommand::ScanStart { root } => scan_start(root, output),
        KnowledgeCommand::ScanStatus => scan_status(output),
        KnowledgeCommand::ScanCancel => scan_cancel(output),
        KnowledgeCommand::Stats => stats(output),
        KnowledgeCommand::TypeCounts => type_counts(output),
        KnowledgeCommand::CollectionsList => collections_list(output),
        KnowledgeCommand::CollectionsCreate {
            name,
            category,
            description,
        } => collections_create(&name, category.as_deref(), description.as_deref(), output),
        KnowledgeCommand::CollectionsUpdate {
            id,
            name,
            category,
            description,
        } => collections_update(id, name, category, description, output),
        KnowledgeCommand::CollectionsAddSources { id, paths } => {
            collections_add_sources(id, paths, output)
        }
        KnowledgeCommand::Documents {
            collection_id,
            limit,
        } => documents(collection_id, limit, output),
        KnowledgeCommand::IndexStatus { job_id } => index_status(job_id.as_deref(), output),
        KnowledgeCommand::IndexCancel { job_id } => index_cancel(&job_id, output),
        KnowledgeCommand::IndexResume { job_id } => index_resume(&job_id, output),
        KnowledgeCommand::IndexRetry { job_id, item_id } => index_retry(&job_id, item_id, output),
        KnowledgeCommand::IndexFailed {
            job_id,
            offset,
            limit,
        } => index_failed(&job_id, offset, limit, output),
        KnowledgeCommand::Search {
            query,
            limit,
            ext,
            after,
            before,
        } => search(
            &query,
            limit,
            ext.as_deref(),
            after.as_deref(),
            before.as_deref(),
            output,
        ),
        KnowledgeCommand::ModelStatus => model_status(output),
        KnowledgeCommand::ModelDownload => Err(model_download_unavailable()),
        KnowledgeCommand::ModelCancel => model_cancel(output),
        KnowledgeCommand::RemoteConnections => remote_connections(output),
        KnowledgeCommand::RemoteProbe { url } => remote_probe(&url, output),
        KnowledgeCommand::RemoteCollections => remote_collections(output),
        KnowledgeCommand::RemoteSearch { collection, query } => {
            remote_search(&collection, &query, output)
        }
        KnowledgeCommand::HostStatus => host_status(output),
    }
}

// ───────────────────────── scan ─────────────────────────

/// GUI `kb_start_scan`: starts a background incremental scan (in-process
/// thread) and waits for it to finish inside this invocation; `--root`
/// omitted defaults to the user home like the GUI.
fn scan_start(root: Option<PathBuf>, output: OutputMode) -> Result<CliOutcome, CliError> {
    // The root is pre-flighted BEFORE the scan starts so a typo'd path or a
    // plain file fails loudly here instead of walking to nothing and
    // reporting a `done` scan that indexed zero files; it mirrors the
    // add-sources path pre-flight. It guards usability, NOT the index, and it
    // cannot guard the index: the root can still vanish between this check
    // and the walk (an unmount, a revoked permission). Index safety is the
    // sweep's own — it authorizes deletion only inside roots the walk
    // actually reached (`features::knowledge::root_authorizes_deletion`), so
    // both an unrelated root and a root lost mid-flight leave the rest of the
    // index alone.
    let root = root.unwrap_or_else(pinvou3_lib::platform::paths::user_home_dir);
    match std::fs::metadata(&root) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err(CliError::failed(format!(
                "knowledge scan start: root {} is not a directory",
                root.display()
            )));
        }
        Err(error) => {
            return Err(CliError::failed(format!(
                "knowledge scan start: root {} does not exist: {error}",
                root.display()
            )));
        }
    }
    // Index keys are the paths the walker emits, i.e. they inherit the exact
    // form of the root it was handed, and the walker does not follow links.
    // Canonicalizing here means a relative path or a symlink to an already
    // indexed directory re-visits the keys the index already holds instead of
    // minting a second set of keys for the same files beside them.
    let root = std::fs::canonicalize(&root).map_err(|error| {
        CliError::failed(format!(
            "knowledge scan start: root {} cannot be resolved: {error}",
            root.display()
        ))
    })?;
    // Scan never touches the import-job store, so like the other pure L1
    // CRUD lanes it must not run the boot recovery: that would flip a job a
    // live desktop process is still importing to interrupted.
    let service = open_service()?;
    let roots = vec![root];
    service.start_scan(roots);
    // The scan runs on a service thread; a one-shot process that returned
    // immediately would kill it before it did any work (nothing would be
    // indexed and no completion marker persisted), so the invocation waits
    // for the scan to finish and reports the final state.
    //
    // This is the only caller that blocks on the SCAN's `running` flag (the
    // import lanes own their jobs the same way through
    // `wait_for_terminal_job`), so it also owns this liveness bound. The
    // scan thread now clears the flag even when it panics
    // (`features::knowledge::start_scan`), but a wait with no bound at all
    // turns any future way of losing that thread into a terminal that hangs
    // with no output and no exit code. The bound is no-progress, not
    // wall-clock: a full home scan legitimately runs for many minutes while
    // `scanned` keeps advancing (the walker reports every 5000 entries and
    // once per root), so "still running and not counting" is the honest
    // signature of a lost thread.
    // The stall bound watches BOTH counters: `scanned` (countable entries,
    // reported every 5000) and `raw_seen` (a heartbeat ticked every 5000
    // RAW enumerations, pre-prune). A pruned-heavy tree — millions of
    // disallowed files under a few countable ones — walks healthily for a
    // long time without `scanned` moving, and keying on it alone would
    // misclassify that walk as a wedged thread and kill it mid-tree.
    let mut last_scanned = service.status().scanned;
    let mut last_raw_seen = service.status().raw_seen;
    let mut last_progress = std::time::Instant::now();
    loop {
        let state = service.status();
        if !state.running {
            // The panic guard's own phase: the walk aborted mid-way, so the
            // completion marker was deliberately not persisted and the stale
            // sweep never ran. Reporting "scan completed" would be a lie and
            // exit 0 would hide it from a script.
            if state.phase == "interrupted" {
                return Err(CliError::failed(format!(
                    "knowledge scan start: the scan thread aborted before finishing \
                     (phase: interrupted, scanned: {}); partial results may be indexed, \
                     the completion marker was not persisted and the stale sweep's \
                     outcome is unknown — re-run the command",
                    state.scanned
                )));
            }
            return scan_out("scan completed (process-local)", state, output);
        }
        if state.scanned != last_scanned || state.raw_seen != last_raw_seen {
            last_scanned = state.scanned;
            last_raw_seen = state.raw_seen;
            last_progress = std::time::Instant::now();
        } else if last_progress.elapsed() >= scan_no_progress_timeout() {
            return Err(CliError::failed(format!(
                // Round-40 review: the import twin's timeout text names the
                // legitimate quiet phases; this one called a healthy scan on
                // a slow mount "gone or wedged" without that disclosure.
                "knowledge scan start: the scan is still flagged running but reported no \
                 progress for {:?} (scanned: {}, raw: {}); the scan thread is gone or wedged, \
                 or a quiet phase (a cold disk or slow network mount between reports) \
                 outran this bound — raise PINVOU_KB_SCAN_STALL_MILLIS and rerun; partial \
                 results may be indexed and the completion marker was not \
                 persisted",
                last_progress.elapsed(),
                state.scanned,
                state.raw_seen
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// How long `scan start` keeps waiting on a scan that is flagged running but
/// has stopped counting. Generous on purpose: the walker only reports every
/// 5000 entries, and a cold spinning disk or a slow network mount can spend
/// minutes between two reports without being stuck.
/// `PINVOU_KB_SCAN_STALL_MILLIS` (milliseconds, positive) overrides the
/// default — the same override shape the import lane's
/// `PINVOU_KB_IMPORT_STALL_MILLIS` has, so the timeout path stays
/// automatable; an unusable value keeps the default.
fn scan_no_progress_timeout() -> std::time::Duration {
    std::env::var("PINVOU_KB_SCAN_STALL_MILLIS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map(std::time::Duration::from_millis)
        .unwrap_or(std::time::Duration::from_secs(300))
}

fn scan_status(output: OutputMode) -> Result<CliOutcome, CliError> {
    let service = open_service()?;
    let state = service.status();
    // Same process-local scope as scan start/cancel: the in-memory scan
    // state is process-local, so this reports what a one-shot process can
    // know (the last finished scan), never a live desktop-app scan.
    scan_out(
        "scan status (process-local: a desktop-app scan in flight is not visible here)",
        state,
        output,
    )
}

/// GUI `kb_scan_status`/the GUI scan surface has no cancel entry; the CLI's
/// `scan cancel` used to call `cancel_scan()` — a signal into its OWN
/// process's scan — and report `{"cancelled":true}` with exit 0. But no scan
/// can ever be running inside that one-shot process when a later invocation
/// executes `scan cancel` (`scan start` blocks until its scan finished, and
/// every other lane never starts one), and a scan a live desktop app is
/// running lives in that app's process, out of this signal's reach. A
/// command reporting success that cannot cancel anything is dishonest; this
/// is therefore a stable refusal, same pattern as `model cancel` and
/// `model download`.
fn scan_cancel(_output: OutputMode) -> Result<CliOutcome, CliError> {
    Err(CliError::failed(
        "knowledge_scan_cancel_requires_product_host: the scan this process could signal is \
         always already over — `scan start` blocks until its scan finishes and no other lane \
         starts one, so a later one-shot invocation can never have a scan to cancel; a scan a \
         live desktop app is running lives in that app's process and is out of reach here",
    ))
}

fn scan_out(header: &str, state: ScanState, output: OutputMode) -> Result<CliOutcome, CliError> {
    let human = format!("{header}\n{}", render_scan_state(&state));
    let value = serde_json::to_value(&state).unwrap_or_default();
    Ok(success(render(output, human, &value)))
}

fn render_scan_state(state: &ScanState) -> String {
    let mut lines = vec![
        format!("phase: {}", state.phase),
        format!("running: {}", state.running),
        format!("scanned: {}", state.scanned),
        format!(
            "roots: {}",
            if state.roots.is_empty() {
                "-".to_owned()
            } else {
                state
                    .roots
                    .iter()
                    .map(|root| crate::support::collapse_control_characters(root))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ),
    ];
    if state.finished_at > 0 {
        lines.push(format!("finished_at: {}", state.finished_at));
    }
    lines.join("\n")
}

// ───────────────────────── stats / type-counts / search ─────────────────────────

/// GUI `kb_stats` (headless `KnowledgeService::stats`): L0 index overview; a
/// fresh store answers with the all-zero state.
fn stats(output: OutputMode) -> Result<CliOutcome, CliError> {
    let service = open_service()?;
    let stats = service
        .stats()
        .map_err(|error| feature_error("stats", error))?;
    let human = format!("total_files: {}", stats.total_files);
    Ok(success(render(
        output,
        human,
        &serde_json::to_value(&stats).unwrap_or_default(),
    )))
}

/// GUI `kb_type_counts` (headless `KnowledgeService::type_counts`).
fn type_counts(output: OutputMode) -> Result<CliOutcome, CliError> {
    let service = open_service()?;
    let counts = service
        .type_counts()
        .map_err(|error| feature_error("type-counts", error))?;
    let human = if counts.is_empty() {
        "no indexed files".to_owned()
    } else {
        counts
            .iter()
            // `ext` derives from filenames, and Unix filenames may carry
            // tabs/C0 controls: collapse like every other store-derived cell
            // so the row cannot forge columns (JSON keeps the original).
            .map(|count| {
                format!(
                    "{}\t{}",
                    crate::support::collapse_control_characters(&count.ext),
                    count.count
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let value = serde_json::json!({
        "type_counts": serde_json::to_value(&counts).unwrap_or_default(),
    });
    Ok(success(render(output, human, &value)))
}

/// GUI `kb_search` (headless `KnowledgeService::search`): the free-text query
/// goes through the same NL-rule merge as the GUI ("pdf from last week" →
/// ext + mtime filter + residual text). `--after`/`--before` are UTC
/// `YYYY-MM-DD` dates; `--limit` omitted keeps the store's default page (0 =
/// 200 upstream). `--before` is exclusive (mtime before the start of the
/// named day; the named day itself never matches).
fn search(
    query: &str,
    limit: Option<usize>,
    ext: Option<&str>,
    after: Option<&str>,
    before: Option<&str>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let service = open_service()?;
    let dto = SearchQueryDto {
        text: Some(query.to_owned()),
        exts: ext.map(|value| vec![value.to_owned()]).unwrap_or_default(),
        mtime_after: after
            .map(|value| parse_date_epoch(value, "--after"))
            .transpose()?,
        // The store filters `mtime <= ?`, so the raw day start would let a
        // file stamped exactly at 00:00:00Z of the named day through; pass
        // the last second of the previous day to keep the boundary truly
        // exclusive at the store's second granularity.
        mtime_before: before
            .map(|value| parse_date_epoch(value, "--before").map(|epoch| epoch - 1))
            .transpose()?,
        id_before: None,
        min_size: None,
        max_size: None,
        limit: limit.unwrap_or(0),
    };
    let hits = service
        .search(dto)
        .map_err(|error| feature_error("search", error))?;
    let human = if hits.is_empty() {
        format!("no matches for {query}")
    } else {
        hits.iter()
            .map(|hit| {
                // Store/user-sourced cells: collapsed like every other
                // family's tab rows, so a name or path carrying a control
                // character cannot forge extra rows or columns in the human
                // block (JSON keeps the originals).
                format!(
                    "{}\t{}\t{}\t{}\t{}",
                    crate::support::collapse_control_characters(&hit.name),
                    hit.ext
                        .as_deref()
                        .map(|ext| crate::support::collapse_control_characters(ext))
                        .unwrap_or_else(|| "-".to_owned()),
                    hit.size,
                    hit.mtime,
                    crate::support::collapse_control_characters(&hit.path)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let value = serde_json::json!({
        "query": query,
        "hits": serde_json::to_value(&hits).unwrap_or_default(),
    });
    Ok(success(render(output, human, &value)))
}

/// Parses a UTC `YYYY-MM-DD` date into UNIX seconds for the search DTO's
/// `mtimeAfter`/`mtimeBefore` (the GUI frontend sends epoch seconds
/// directly; the civil-days conversion is inlined and pinned by hand-computed
/// fixtures so the UTC day boundary is stable regardless of the platform's
/// chrono version). Invalid shapes are usage errors, like bad ids.
fn parse_date_epoch(value: &str, flag: &str) -> Result<i64, CliError> {
    let invalid = || {
        CliError::usage(format!(
            "knowledge {flag} must be a UTC YYYY-MM-DD date (got {value})"
        ))
    };
    let parts: Vec<&str> = value.split('-').collect();
    let [year, month, day] = parts.as_slice() else {
        return Err(invalid());
    };
    let year: i64 = year.parse().map_err(|_| invalid())?;
    let month: u32 = month.parse().map_err(|_| invalid())?;
    let day: u32 = day.parse().map_err(|_| invalid())?;
    if !(1..=9999).contains(&year)
        || month == 0
        || month > 12
        || day == 0
        || day > days_in_month(year, month)
    {
        return Err(invalid());
    }
    Ok(days_from_civil(year, month, day) * 86_400)
}

#[cfg(test)]
mod date_math_tests {
    use super::{days_from_civil, days_in_month, parse_date_epoch};

    /// The calendar math has no value-level pin anywhere in the contract
    /// suites (they only pin the invalid shapes): these are hand-computed
    /// anchors — epoch day 0, a leap day, a modern date, and the seconds
    /// conversion.
    #[test]
    fn civil_date_math_pins_known_epoch_values() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 2, 29), 11_016);
        assert_eq!(days_from_civil(2026, 9, 1), 20_697);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(parse_date_epoch("1970-01-01", "--before").unwrap(), 0);
        assert_eq!(parse_date_epoch("1970-01-02", "--after").unwrap(), 86_400);
        assert_eq!(
            parse_date_epoch("2000-02-29", "--before").unwrap(),
            11_016 * 86_400
        );
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * i64::from(mp) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

// ───────────────────────── collections ─────────────────────────

fn collections_list(output: OutputMode) -> Result<CliOutcome, CliError> {
    let service = open_service()?;
    let collections = service
        .l1()
        .list_collections()
        .map_err(|error| feature_error("collections list", error))?;
    let human = if collections.is_empty() {
        "no collections".to_owned()
    } else {
        collections
            .iter()
            .map(|collection| {
                // User-named cell: collapsed like every other family's tab
                // rows (JSON keeps the original name).
                format!(
                    "{}\t{}\t{}\tdocs={}\tchunks={}\tbytes={}",
                    collection.id,
                    crate::support::collapse_control_characters(&collection.name),
                    collection.status,
                    collection.doc_count,
                    collection.chunk_count,
                    collection.total_bytes
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let value = serde_json::json!({
        "collections": serde_json::to_value(&collections).unwrap_or_default(),
    });
    Ok(success(render(output, human, &value)))
}

fn collections_create(
    name: &str,
    category: Option<&str>,
    description: Option<&str>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    // Pure L1 CRUD never touches import jobs, so it must not run the boot
    // recovery that wedges a live desktop-app import to interrupted.
    let service = open_service()?;
    let id = service
        .l1()
        .create_collection(name, category, description)
        .map_err(|error| feature_error("collections create", error))?;
    Ok(success(render(
        output,
        format!("created collection {id}"),
        &serde_json::json!({ "id": id }),
    )))
}

/// GUI `kb_collection_update` always receives the complete field set from the
/// frontend and replaces the row; the CLI's optional flags are merged onto
/// the stored values first so the GUI's replace semantics keep the rest.
fn collections_update(
    id: i64,
    name: Option<String>,
    category: Option<String>,
    description: Option<String>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    // Same as create: pure L1 CRUD, no job reconciliation, no recovery.
    let service = open_service()?;
    let collections = service
        .l1()
        .list_collections()
        .map_err(|error| feature_error("collections update", error))?;
    if !collections.iter().any(|collection| collection.id == id) {
        // One stable code for the family's not-found class: the concurrent
        // race arm below reports the same condition as
        // `knowledge_collection_not_found`, so the pre-check must not split
        // the class across two machine prefixes (round-39 review).
        return Err(CliError::failed(format!(
            "knowledge_collection_not_found: collection {id} not found"
        )));
    }
    // Round-45 review: the write is partial — only the provided columns join
    // the SET list (`update_collection_fields`), so a concurrent rename or
    // category edit between this process's read and write is no longer
    // reverted by stale merged values. The rows-affected check below still
    // catches a concurrent delete. The pre-check read stays only to answer
    // the not-found class with one stable code (round-39).
    let changed = service
        .l1()
        .update_collection_fields(
            id,
            name.as_deref(),
            category.as_deref(),
            description.as_deref(),
        )
        .map_err(|error| feature_error("collections update", error))?;
    if !changed {
        // Round-37 review: the pre-check can lose a race with a concurrent
        // delete; the store's rows-affected is the authority. Reporting
        // success for a row that no longer exists would be a no-op lie.
        return Err(CliError::failed(format!(
            "knowledge_collection_not_found: collection {id} no longer exists"
        )));
    }
    // Round-45 review: the payload reports exactly what the caller changed
    // (the write is partial now), not a read-merge of the row.
    let mut payload = serde_json::json!({ "id": id });
    if let Some(name) = &name {
        payload["name"] = serde_json::json!(name);
    }
    if let Some(category) = &category {
        payload["category"] = serde_json::json!(category);
    }
    if let Some(description) = &description {
        payload["description"] = serde_json::json!(description);
    }
    Ok(success(render(
        output,
        format!("updated collection {id}"),
        &payload,
    )))
}

/// The collection existence check shared by the collection commands: the
/// upstream create (`INSERT ... WHERE EXISTS`) is a silent no-op for an
/// unknown id and `start_index` would fall back to reporting a stale job,
/// so the CLI names the id instead.
fn ensure_collection_exists(
    service: &KnowledgeService,
    id: i64,
    operation: &str,
) -> Result<(), CliError> {
    match service.l1().collection_name(id) {
        Ok(Some(_)) => Ok(()),
        // The stable family code, not a per-operation prose prefix, so a
        // script keying on not-found matches every command in the family
        // (round-39 review); the operation stays in the message for humans.
        Ok(None) => Err(CliError::failed(format!(
            "knowledge_collection_not_found: collection {id} not found ({operation})"
        ))),
        Err(error) => Err(feature_error(operation, error)),
    }
}

/// GUI `kb_collection_delete`: cancel a running import for the collection,
/// delete it (which cascades the collection's import-job rows), then clear
/// every session mount.
fn collections_delete(id: i64, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Deleting a collection — one of its own import jobs or not — is a
    // destructive by intent. It deliberately never runs the GUI's boot
    // recovery of interrupted jobs: `delete_collection` already cascades
    // every import-job row this collection owns (cancel-then-delete via
    // the named cancel below for a live one, the DELETE for the
    // rest), so recovery would serve no purpose here and would wedge an
    // unrelated job a live desktop-app process is still importing to
    // `interrupted` on its way to a delete that does not concern it.
    let service = open_service()?;
    ensure_collection_exists(&service, id, "collections delete")?;
    // Round-40 review: cancel by NAME. `cancel_index_for_collection`
    // re-derives the target from a second `index_status()` read, so a job
    // leaving the live tier inside that window (it finished, or the user
    // cancelled it in the desktop app) made this delete cancel whichever
    // job had become latest — an import the caller never named — and park
    // its collection mid-import. Read the status once and cancel the named
    // id while it still belongs to this collection and is live. The
    // sub-second residual (the job flipping terminal between the read and
    // the named cancel) is the window `cancel_index_job` already carries:
    // a terminal job is an idempotent no-op there, never another
    // collection's job.
    let status = service.index_status();
    let live_job = status
        .job_id
        .filter(|_| status.collection_id == id && (status.running || status.resumable));
    if let Some(job_id) = live_job.as_deref() {
        service
            .cancel_index_job(job_id)
            .map_err(|error| feature_error("collections delete", error))?;
    }
    service
        .l1()
        .delete_collection(id)
        .map_err(|error| feature_error("collections delete", error))?;
    // Session mounts live ONLY in the desktop app's per-process memory
    // (`features::sessions::mode_state` — deliberately not persisted), so
    // there is no CLI-side mount to sweep: `remove_mounted_collection_from_all`
    // operates on the calling process's in-memory mode states and would
    // return an empty list here by construction. Earlier rounds booted the
    // whole session store just to prove that emptiness — `SessionStore::boot`
    // is not a read (it enforces the 50-sessions-per-kind retention and
    // irreversibly evicts the oldest non-pinned sessions), a destructive
    // side effect no knowledge command should trigger. The running app's
    // own mount bookkeeping is its concern; the desktop app's kb delete
    // sweep covers it there.
    Ok(success(render(
        output,
        format!(
            "deleted collection {id}\nnote: mounts held in a running desktop app session are \
             unaffected by this process"
        ),
        &serde_json::json!({ "id": id }),
    )))
}

/// GUI `kb_collection_add_sources`: persists the import job, then the
/// invocation BLOCKS until the job reaches a terminal phase — a one-shot
/// process exits right after printing, which killed the fire-and-forget
/// variant's import thread mid-work and stranded the job flagged `running`
/// with no owner on disk (see the module docs). The GUI frontend only
/// offers existing collections, so the CLI adds the existence check the API
/// silently assumes.
fn collections_add_sources(
    id: i64,
    paths: Vec<PathBuf>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    // Validate everything a mistyped invocation could get wrong before the
    // job is created: the path pre-flight is pure filesystem, and the
    // existence check names the id (upstream answers an unknown id with the
    // previous job's state, which would masquerade as a start).
    //
    // Round-46 review: a credential-path gate on each enqueued source. The
    // store these sources land in is the one `kb_search` reads model-side,
    // so `add-sources ~/.ssh` would ingest private-key material into the
    // model's search corpus. The GUI's folder picker has no such gate
    // (disclosed CLI-stricter-than-GUI, the same posture `files ingest`
    // takes); canonicalize first so the component check cannot be escaped
    // by a symlink or a `..` hop, and hand the canonical path downstream
    // (the pre-existing canonicalization below becomes the same path).
    for path in &paths {
        let Ok(meta) = std::fs::metadata(path) else {
            return Err(CliError::failed(format!(
                "knowledge collections add-sources: source path {} does not exist",
                path.display()
            )));
        };
        if !meta.is_file() && !meta.is_dir() {
            return Err(CliError::failed(format!(
                "knowledge collections add-sources: source path {} is not a regular file \
                 or directory",
                path.display()
            )));
        }
        // Round-47 review: the gate fails CLOSED on a canonicalize failure,
        // like every sibling lane (memory add, personas --file, agent run
        // --prompt-file/--attach, feedback). The old `unwrap_or_else`
        // fallback gated the raw spelling when the path vanished between
        // the metadata pre-flight and this stat — a symlink swap in that
        // window would sail through unresolved. The re-canonicalize for
        // the stored name below keeps its own documented fallback: there
        // the walk itself re-reports missing paths.
        let resolved = path.canonicalize().map_err(|error| {
            CliError::failed(format!(
                "knowledge collections add-sources: cannot resolve source path {}: {error}",
                path.display()
            ))
        })?;
        crate::artifacts::check_sensitive_path(&resolved).map_err(|reason| {
            CliError::failed(format!(
                "knowledge collections add-sources: refusing source path: {reason}"
            ))
        })?;
    }
    let service = open_service()?;
    ensure_collection_exists(&service, id, "collections add-sources")?;
    // A running latest job has an owner — the only candidate inside a
    // fresh one-shot process is THIS command's own job, which does not
    // exist yet, so any running job predates this invocation: another
    // process's import (the desktop app, or an earlier one-shot invocation
    // whose job a hard kill stranded). Starting a second import against
    // the same store would run items concurrently; refuse instead (the
    // same rule `index resume`/`index retry` enforce).
    let preexisting = service.index_status();
    if preexisting.running {
        return Err(CliError::failed(format!(
            "knowledge collections add-sources: the latest index job {} is still running and \
             owns the import store — its owner is another process (a desktop-app import, or \
             an earlier one-shot invocation's job stranded by a hard kill); the requested \
             sources were NOT enqueued (`pinvou knowledge index status` to watch it, \
             `pinvou knowledge index cancel {}` to drop it)",
            preexisting.job_id.as_deref().unwrap_or("none"),
            preexisting.job_id.as_deref().unwrap_or(""),
        )));
    }
    // `start_index` falls back to `index_status()` when the job create or
    // the follow-up state read fails; the reported job must then not be
    // passed off as the fresh import.
    let previous_job = preexisting.job_id;
    // Round-38 review: canonicalize the enqueued roots exactly like the scan
    // lane (this lane used to persist them verbatim). A relative source was
    // stored as typed and `index resume` from another CWD re-resolved it
    // against the new CWD — a different file, or a read failure landing the
    // item in `done_with_errors`; a symlinked source keyed its documents
    // under the link-prefixed path, so re-adding the same content via the
    // canonical path created a second document. A canonicalize failure here
    // is a vanish race (the metadata pre-flight passed on the same name):
    // keep the verbatim name and let the import's own walk report it.
    // Round-48 review: when this second resolution lands somewhere OTHER
    // than the path the credential gate checked (a symlink swap in the
    // gate→enqueue window), re-run the sensitive-path gate on the resolved
    // target — the gate is cheap and the whole point is that the ingested
    // path is the policy-checked one. A vanishing path keeps the documented
    // fallback (verbatim name; the import's own walk reports it).
    let mut resolved_paths: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for path in paths {
        let resolved = path.canonicalize().unwrap_or_else(|_| path.clone());
        if resolved != path {
            crate::artifacts::check_sensitive_path(&resolved).map_err(|reason| {
                CliError::failed(format!(
                    "knowledge collections add-sources: refusing source path: {reason}"
                ))
            })?;
        }
        resolved_paths.push(resolved);
    }
    let paths = resolved_paths;
    // Disclosed residual (same sub-second race class the id-taking
    // `cancel_index_job` documents): a job a desktop app starts on this
    // collection between the pre-check above and `start_index` carries a
    // fresh job id, passes the guards below, and this command then blocks
    // on and reports the APP's import while its own sources were never
    // enqueued. Re-running the command is the remedy. A further residual
    // of the same race: if that APP-owned import then stays quiet past this
    // invocation's stall timeout, the timeout interrupts the app's healthy
    // job (it lands `interrupted`, immediately resumable — no data loss,
    // but a live GUI import was flipped as a side effect).
    let state = service.start_index(id, paths);
    // Upstream quirk: any resumable job short-circuits start_index and the
    // requested sources are silently dropped — a fresh job reports
    // preparing/running with resumable=false, so resumable here always
    // means a PRE-EXISTING job won. Reporting it as success would hide
    // files that were never enqueued, for a different collection and for
    // this one alike.
    if state.resumable || state.collection_id != id {
        // A terminal `done` job of another collection can land in this arm
        // through `start_index`'s status fallback (nothing about it is
        // "unfinished", and there is nothing to resume or cancel), so the
        // remedy is keyed on the actual phase instead of asserting one.
        let phase = display_phase(&state);
        let remedy = if phase == "done" {
            "check `pinvou knowledge index status`; if it shows this terminal job, \
             re-run this command (the finished import needs no attention)"
        } else {
            "check `pinvou knowledge index status`, resume or cancel it first, \
             then re-run this command"
        };
        return Err(CliError::failed(format!(
            "knowledge_add_sources_blocked: index job {} (collection {}, phase {phase}) \
             blocks collection {id} and the requested sources were NOT enqueued ({remedy})",
            state.job_id.as_deref().unwrap_or("none"),
            state.collection_id,
        )));
    }
    // Success requires a genuinely new job (or one demonstrably in flight);
    // the pre-call latest job re-reported in a terminal phase means no
    // import was created and the requested sources were never enqueued.
    if state.job_id == previous_job && !state.running {
        return Err(CliError::failed(format!(
            "knowledge_index_start_failed: collection {id} has no freshly created index \
             job (latest: {}, running: {}); the requested sources were not enqueued",
            state.job_id.as_deref().unwrap_or("none"),
            state.running
        )));
    }
    let Some(job_id) = state.job_id.clone() else {
        return Err(CliError::failed(format!(
            "knowledge_index_start_failed: collection {id} reported no index job id; the \
             requested sources were not enqueued"
        )));
    };
    let final_state =
        wait_for_terminal_job(&service, &job_id, "knowledge collections add-sources")?;
    index_finished(&final_state, output)
}

// ───────────────────────── documents ─────────────────────────

/// GUI `kb_documents`: `limit` omitted maps to the GUI's 0 = default page of
/// 500 (`L1Store::list_documents`).
fn documents(
    collection_id: i64,
    limit: Option<usize>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let service = open_service()?;
    // An unknown collection id answers an empty list from `list_documents`
    // (exit 0, indistinguishable from a genuinely empty collection) — the
    // one collection-addressing lane that skipped the family's existence
    // gate (`documents remove` has it; `update`/`delete`/`add-sources` have
    // it). Name the id instead.
    ensure_collection_exists(&service, collection_id, "documents")?;
    let documents = service
        .l1()
        // No CLI-side page cap: `L1Store::list_documents` already clamps to
        // `store::SEARCH_LIMIT_CAP` for exactly this reason (a raw `as i64`
        // would wrap usize::MAX into SQLite's LIMIT -1 = unlimited and
        // materialize the whole table). That constant is `pub(crate)` in the
        // app crate, so the CLI cannot import it; a second local copy of the
        // same number would silently desync the day upstream changes it, and
        // the guarantee is upstream's to keep.
        .list_documents(collection_id, limit.unwrap_or(0))
        .map_err(|error| feature_error("documents", error))?;
    let human = if documents.is_empty() {
        format!("no documents in collection {collection_id}")
    } else {
        documents
            .iter()
            .map(|document| {
                // Store/filesystem-sourced cells: collapsed like every other
                // family's tab rows (JSON keeps the originals).
                format!(
                    "{}\t{}\t{}\tchunks={}\t{}",
                    document.id,
                    crate::support::collapse_control_characters(&document.name),
                    document.parse_status,
                    document.n_chunks,
                    crate::support::collapse_control_characters(&document.path)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let value = serde_json::json!({
        "collection_id": collection_id,
        "documents": serde_json::to_value(&documents).unwrap_or_default(),
    });
    Ok(success(render(output, human, &value)))
}

/// GUI `kb_remove_document` with the existence check the rest of this family
/// adds: the upstream delete is a silent no-op for unknown ids, which would
/// report success for a document that was never there. Pure L1 CRUD (an
/// existence check plus a delete), so it opens WITHOUT boot recovery — the
/// recovering open belongs to commands that legitimately reconcile the job
/// store, and running it here would wedge a live desktop-app import for a
/// delete that never touches the job store.
fn documents_remove(doc_id: i64, output: OutputMode) -> Result<CliOutcome, CliError> {
    let service = open_service()?;
    let exists = service
        .l1()
        .document_exists(doc_id)
        .map_err(|error| feature_error("documents remove", error))?;
    if !exists {
        return Err(CliError::failed(format!(
            "knowledge_document_not_found: document {doc_id} does not exist"
        )));
    }
    let deleted = service
        .l1()
        .remove_document(doc_id)
        .map_err(|error| feature_error("documents remove", error))?;
    if !deleted {
        // Round-37 review: same concurrent-delete race as `collections
        // update` — the store's rows-affected decides, not the pre-check.
        return Err(CliError::failed(format!(
            "knowledge_document_not_found: document {doc_id} does not exist"
        )));
    }
    Ok(success(render(
        output,
        format!("removed document {doc_id}"),
        &serde_json::json!({ "id": doc_id }),
    )))
}

// ───────────────────────── index jobs ─────────────────────────

/// GUI `kb_index_status` polls the latest job; the CLI reports that same
/// latest job when no id is given, and the NAMED job when one is.
///
/// The named job must not be answered from the latest-job read: upstream's
/// ordering ranks `preparing|running` first, then `interrupted`, then
/// `done_with_errors`, and everything else (including `cancelled`) last, so a
/// job the caller just cancelled is routinely outranked by an older terminal
/// one. Reporting that other job's state under the requested id — or refusing
/// because "only the latest job is reachable" — were both wrong answers to a
/// question the store can answer exactly ([`KnowledgeService::index_job_state`]).
fn index_status(job_id: Option<&str>, output: OutputMode) -> Result<CliOutcome, CliError> {
    let service = open_service()?;
    let state = match job_id {
        None => service.index_status(),
        Some(job_id) => named_job_state(&service, job_id, "status")?,
    };
    index_out("index status", state, output)
}

/// Reads one named job's state, mapping the store's "unknown id" answer
/// (rusqlite's `QueryReturnedNoRows`) onto the family's stable
/// `knowledge_index_job_not_found` code instead of leaking the driver
/// message (the same mapping `index failed`/`index resume` use).
fn named_job_state(
    service: &KnowledgeService,
    job_id: &str,
    operation: &str,
) -> Result<IndexState, CliError> {
    service.index_job_state(job_id).map_err(|error| {
        if error.contains("Query returned no rows") {
            CliError::failed(format!(
                "knowledge_index_job_not_found: no index job {job_id} exists"
            ))
        } else {
            feature_error(&format!("index {operation}({job_id})"), error)
        }
    })
}

/// GUI `kb_index_cancel` targets the active/latest job. The CLI takes the
/// job id explicitly: the cancel goes through `cancel_index_job` — the
/// id-taking transition `interrupt_index` already uses as its shape — which
/// looks up and cancels exactly the named job inside one store call. The
/// pre-PR flow verified `active == job_id` from one `index_status()` read
/// and then called `cancel_index()`, which re-derived the "latest" job
/// server-side: a sub-second swap between the two reads could cancel a
/// desktop-app import the caller never named.
///
/// Semantics: a running or resumable named job is really cancelled (the
/// store's `cancel` is synchronous and transactional) and reported as
/// signalled; a finished named job (done or already cancelled) is an
/// idempotent no-op that still succeeds and honestly reports that nothing
/// was signalled; an unknown id is the family's stable
/// `knowledge_index_job_not_found` code.
fn index_cancel(job_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let service = open_service()?;
    // The pre-transition read and the transition are the same store call, so
    // the `was_active` decision below cannot describe a different job than
    // the one the store cancelled.
    let pre = service.cancel_index_job(job_id).map_err(|error| {
        if error.contains("Query returned no rows") {
            CliError::failed(format!(
                "knowledge_index_job_not_found: no index job {job_id} exists"
            ))
        } else {
            feature_error(&format!("index cancel({job_id})"), error)
        }
    })?;
    // A running or resumable job got a real cancel signal; a finished job
    // (done/cancelled) took the same call without anything to signal —
    // `ImportJobStore::cancel`'s WHERE clause is the idempotent no-op here.
    let was_active = pre.running || pre.resumable;
    // Report the job that was actually cancelled, NOT `index_status()`. The
    // latest-job read ranks `cancelled` in its last bucket, so the successful
    // cancel hands the "latest" crown to any older `done_with_errors` /
    // `interrupted` job: the human line said "cancel signalled for job B"
    // while the JSON body carried A's state under A's jobId.
    // Round-48 review: the cancel landed transactionally above; a transient
    // read failure in this REPORT read must not turn into an exit-1 store
    // error that never says the signal landed (the cancel is idempotent,
    // but the first answer would mislead). Fold the report back to the
    // pre-transition state already in hand.
    let state = named_job_state(&service, job_id, "cancel").unwrap_or_else(|_| pre.clone());
    let header = if was_active {
        format!("index cancel signalled for job {job_id}")
    } else {
        format!(
            "index cancel: job {job_id} was not active (running: {}); nothing was signalled",
            pre.running
        )
    };
    // Round-45 review: the cancel is deliberately not gated behind `--yes`
    // and drops the job's staged chunks, so the output itself carries the
    // consequence (docs/pinvou-cli.md's knowledge row promises exactly
    // this): a cancelled job can never `resume` or `retry`, and recovery
    // means re-running `add-sources` from scratch. A no-op cancel on a
    // finished job discards nothing and stays silent.
    let note = if was_active {
        Some(
            "the cancel discarded the job's staged progress; a cancelled job \
             cannot resume or retry — recovery means re-running add-sources \
             from scratch",
        )
    } else {
        None
    };
    let mut human = format!("{header}\n{}", render_index_state(&state));
    let mut value = serde_json::to_value(&state).unwrap_or_default();
    value["phase"] = serde_json::json!(display_phase(&state));
    if let Some(note) = note {
        human.push_str(&format!("\nnote: {note}"));
        value["note"] = serde_json::json!(note);
    }
    Ok(success(render(output, human, &value)))
}

/// Validates the named job id against the latest job on the caller's own
/// service handle: `resume`/`retry` re-arm a job only if it is `interrupted`
/// (retry also accepts `done_with_errors`), and both refuse while the latest
/// job is still `running`.
fn require_job_id(
    service: &KnowledgeService,
    job_id: &str,
    operation: &str,
) -> Result<IndexState, CliError> {
    let latest = service.index_status();
    let Some(active) = latest.job_id.as_deref() else {
        return Err(CliError::failed(format!(
            "knowledge_index_job_not_found: no index job exists (nothing to {operation} for \
             {job_id})"
        )));
    };
    if active != job_id {
        return Err(CliError::failed(format!(
            "knowledge index {operation}({job_id}): job is not the active/latest index \
             job (latest: {active})"
        )));
    }
    // A running job has an owner: it is either THIS process's import thread
    // (still inside its own blocking wait — no second command can run in a
    // one-shot process) or another process's, most plausibly the desktop
    // app's. Re-arming it here would run the same items twice against the
    // same job row. The CLI never runs the GUI's boot recovery, so a job
    // stranded `running` by a killed one-shot process (SIGINT-mid-import,
    // power loss) stays that way from the CLI until the desktop app's next
    // boot reconciles it to `interrupted` — resume becomes available then.
    if latest.running {
        return Err(CliError::failed(format!(
            "knowledge index {operation}({job_id}): the latest index job is still running and \
             cannot be re-armed from here — its owner is another process (a desktop-app \
             import, or an earlier one-shot invocation's job stranded by a hard kill); \
             `pinvou knowledge index cancel {job_id}` can drop it, or the desktop app's \
             next start relabels it interrupted/resumable",
        )));
    }
    Ok(latest)
}

/// GUI `kb_index_resume` re-arms an interrupted job; the invocation then
/// blocks until the re-armed job reaches a terminal phase (one-shot
/// semantics — see the module docs), reporting the final state. A latest
/// job that is finished rather than interrupted answers the store's own
/// refusal, mapped to the family's stable code by [`index_state_result`].
fn index_resume(job_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let service = open_service()?;
    require_job_id(&service, job_id, "resume")?;
    // Round-48 review: this comment used to disclose "the store's guarded
    // re-arm then runs both importers on one collection" — but
    // `refuse_fresh_foreign_running_import` (app-side `knowledge/mod.rs`,
    // pinned by its tests and by the contract lane below) now refuses a
    // fresh foreign RUNNING import with `knowledge_index_busy`, so the
    // double-importer outcome is no longer reachable; the remaining
    // disclosed window is only the job-id vs latest race class the
    // add-sources lane documents. The store guard still prevents re-arming
    // a job that is not `interrupted`, and the invocation blocks on and
    // reports `job_id`'s own state; rerunning after interrupting is the
    // remedy.
    index_state_result(service.resume_index(job_id.to_owned()))?;
    let final_state = wait_for_terminal_job(&service, job_id, "knowledge index resume")?;
    index_finished(&final_state, output)
}

/// GUI `kb_index_retry` re-queues one failed item; same pre-validation and
/// one-shot wait as [`index_resume`].
fn index_retry(job_id: &str, item_id: i64, output: OutputMode) -> Result<CliOutcome, CliError> {
    let service = open_service()?;
    require_job_id(&service, job_id, "retry")?;
    index_state_result(service.retry_index_item(job_id.to_owned(), item_id))?;
    let final_state = wait_for_terminal_job(&service, job_id, "knowledge index retry")?;
    index_finished(&final_state, output)
}

/// Maps an upstream `Result<_, String>` error from the job-continuing
/// operations (`resume`/`retry`): an unknown or non-resumable job id answers
/// with rusqlite's `QueryReturnedNoRows`; name the real cause instead of
/// leaking the raw driver message (the same code `index failed` uses).
/// Round-45 review: an item-level miss on a perfectly resumable job (a bad
/// item id) carries the store's "is not a failed item" marker and must map
/// to its own code, not the job-not-found one.
fn index_state_result(result: Result<IndexState, String>) -> Result<IndexState, CliError> {
    result.map_err(|error| {
        if error.contains("is not a failed item") {
            CliError::failed(format!("knowledge_index_item_not_found: {error}"))
        } else if error.contains("Query returned no rows") {
            CliError::failed(
                "knowledge_index_job_not_found: no resumable index job for the requested id"
                    .to_owned(),
            )
        } else if error.contains(pinvou3_lib::features::knowledge::FOREIGN_IMPORT_RUNNING_MARKER) {
            // Round-46 review: the fresh-foreign-running refusal was the one
            // zh-CN-only, codeless error class reachable through this lane
            // (a surface starting an import inside the
            // `require_job_id`→command race). Match the single-sourced
            // marker and give scripts a stable code.
            CliError::failed(format!(
                "knowledge_index_busy: {error}; another surface started this import \
                 moments ago — watch it with `pinvou knowledge index status` or drop it \
                 with `pinvou knowledge index cancel <job-id>`"
            ))
        } else {
            CliError::failed(format!("knowledge index: {error}"))
        }
    })
}

/// How long an import-owning command keeps waiting on a job that is still
/// flagged running but reports no observable progress. Generous on purpose:
/// the importer can legitimately go quiet while it loads the embedding model
/// (~570 MiB of ONNX/tokenizer reads on model-installed machines) before the
/// first item's counters start moving. Same order as the scan lane's bound.
const IMPORT_NO_PROGRESS_TIMEOUT: Duration = Duration::from_secs(300);

/// The no-progress bound actually applied. `PINVOU_KB_IMPORT_STALL_MILLIS`
/// (milliseconds, positive) overrides the default for automation and tests
/// that exercise the timeout path; an unusable value keeps the default.
fn import_no_progress_timeout() -> Duration {
    std::env::var("PINVOU_KB_IMPORT_STALL_MILLIS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map(Duration::from_millis)
        .unwrap_or(IMPORT_NO_PROGRESS_TIMEOUT)
}

/// Waits until the named job leaves the running phase, polling the per-job
/// state surface ([`KnowledgeService::index_job_state`]) — the same read
/// `index status <job-id>` uses. Returns the terminal state, or fails with a
/// liveness error when the job reports no progress for
/// [`import_no_progress_timeout`] (the import thread is gone or wedged).
///
/// Terminal states are exactly the phases with `running: false`
/// (`interrupted`/`cancelled`/`done`/`done_with_errors` in
/// [`display_phase`] vocabulary).
///
/// A timeout interrupts the job through the feature layer's
/// [`KnowledgeService::interrupt_index`] first — the round-18 review's
/// prescription — so it is left `interrupted`/resumable rather than
/// `running`-with-no-owner, and the next `index resume` can continue it
/// without waiting for a desktop-app boot. The interrupt is best-effort: it
/// cannot mask the timeout report, and the remedy text states the job's real
/// on-disk state (interrupted-and-resumable, or still running when the
/// interrupt could not land).
fn wait_for_terminal_job(
    service: &KnowledgeService,
    job_id: &str,
    operation: &str,
) -> Result<IndexState, CliError> {
    let stall_bound = import_no_progress_timeout();
    // Round-48 review: the FIRST poll sits inside the same read-error
    // tolerance as the loop below. It used to propagate immediately, so a
    // transient store-busy at the one poll this command makes before its
    // own import thread starts (the comment at the tolerance below grants
    // that a sustained cross-process SQLite write burst can outrun the 5s
    // busy_timeout) killed the command — reaping the very thread that was
    // about to run the import and stranding the job flagged `running`, the
    // exact shape the round-41 tolerance exists to prevent.
    let mut last = {
        const READ_ERROR_TOLERANCE: u32 = 10;
        const READ_ERROR_BACKOFF: std::time::Duration = std::time::Duration::from_millis(250);
        let mut attempt = 0;
        loop {
            match named_job_state(service, job_id, "status") {
                Ok(state) => break state,
                Err(error) if attempt < READ_ERROR_TOLERANCE => {
                    attempt += 1;
                    eprintln!(
                        "knowledge {operation}: status read failed ({error}); retry {attempt}/{READ_ERROR_TOLERANCE}"
                    );
                    std::thread::sleep(READ_ERROR_BACKOFF);
                }
                Err(error) => return Err(error),
            }
        }
    };
    // Consecutive failed status polls; reset on every good poll (round-41
    // review, see the poll tail below).
    let mut read_errors: u32 = 0;
    // Observable-progress signature: items completed, items total, the file
    // currently being parsed and its chunk counter. Any of them moving
    // resets the liveness clock.
    let mut last_signature = job_signature(&last);
    let mut last_progress = Instant::now();
    loop {
        if !last.running {
            return Ok(last);
        }
        if last_signature != job_signature(&last) {
            last_signature = job_signature(&last);
            last_progress = Instant::now();
        } else if last_progress.elapsed() >= stall_bound {
            // Leave the job resumable: interrupt flips a preparing/running
            // job to `interrupted` and returns its claimed item to pending
            // on disk, so the next `index resume` continues it without a
            // recovery-owning boot in between. If the interrupt cannot land
            // (the job vanished, the store broke), the report still names
            // the stall — and the remedy text then honestly says the job is
            // left running, with the app-boot path as the resume route.
            if let Err(error) = service.interrupt_index(job_id) {
                note!(
                    "warning: knowledge {operation}: could not interrupt job {job_id} after \
                     the stall timeout: {error}"
                );
            }
            let resumable_now = matches!(
                named_job_state(service, job_id, "status"),
                Ok(state) if state.resumable
            );
            // The state can also have reached a terminal phase in the
            // milliseconds between the last poll and the interrupt (which is
            // a no-op for a non-running job); name the actual phase in the
            // remedy instead of promising a relabel that never applies.
            let actual_phase = named_job_state(service, job_id, "status")
                .ok()
                .map(|state| display_phase(&state));
            let remedy = if resumable_now {
                format!(
                    "the job was interrupted and is resumable now (`pinvou knowledge index \
                     resume {job_id}` continues it)"
                )
            } else if matches!(
                actual_phase.as_deref(),
                Some("done") | Some("done_with_errors") | Some("cancelled")
            ) {
                // Round-40 review: a job landing TERMINAL in the moment
                // between the last poll and this interrupt is not "left
                // as-is and resumable" — the remedy must match the on-disk
                // phase, and only a genuinely still-running (or
                // recovery-owned) job gets the relabel/cancel remedy.
                format!(
                    "the job reached `{}` right at the deadline — it is terminal \
                     (`pinvou knowledge index status` shows it); nothing needs resuming or \
                     cancelling",
                    actual_phase.as_deref().unwrap_or("done")
                )
            } else {
                format!(
                    "the job is left as-is on disk and stays resumable once a recovery-owning \
                     process (the desktop app's next start) relabels it, or `pinvou \
                     knowledge index cancel {job_id}` drops it"
                )
            };
            return Err(CliError::failed(format!(
                "{operation}: index job {job_id} is still flagged running but reported no \
                 progress for {}s (done: {}/{}, failed: {}, in flight: {}); either the import \
                 thread is gone or wedged, or a quiet phase (the embedding-model load, a source \
                 tree whose walk outlasts this bound, or one file whose parse/OCR alone — its \
                 staging only moves the counters after it finishes — outlasts the bound; raise \
                 PINVOU_KB_IMPORT_STALL_MILLIS for huge trees) outran it — {remedy}",
                stall_bound.as_secs(),
                last.done,
                last.total,
                last.failed,
                last.current_path.as_deref().unwrap_or("(none)")
            )));
        }
        // The poll step stays at 50 ms for real imports, but a sub-second
        // stall bound (the hermetic tests drive the trip with `1`) must not
        // have its whole window swallowed by one sleep — that made the trip
        // depend on the import losing a race against the FIRST poll, which
        // is the flake the frozen-import test's margin rests on (round-39
        // review).
        let step =
            std::cmp::min(Duration::from_millis(50), stall_bound / 2).max(Duration::from_millis(1));
        std::thread::sleep(step);
        // Round-41 review: a transient store-read error used to abort the
        // whole wait — and the import thread lives in this same one-shot
        // process, so the exit killed it mid-item and left exactly the
        // "job flagged running with no owner" state this command's contract
        // excludes. Tolerate a short burst of failed polls (a sustained
        // cross-process SQLite write burst can outrun the 5s busy_timeout);
        // if the failures persist, interrupt the job — the stall arm's
        // route, landing it `interrupted` + resumable on disk — and only
        // then fail.
        const READ_ERROR_TOLERANCE: u32 = 10;
        match named_job_state(service, job_id, "status") {
            Ok(state) => {
                read_errors = 0;
                last = state;
            }
            Err(error) => {
                read_errors += 1;
                if read_errors < READ_ERROR_TOLERANCE {
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                let interrupted = service.interrupt_index(job_id).is_ok();
                let remedy = if interrupted {
                    format!(
                        "the job was interrupted and is resumable (`pinvou knowledge index \
                         resume {job_id}` continues it)"
                    )
                } else {
                    format!(
                        "the interrupt could not land either; the job is left as-is on disk \
                         and stays resumable once a recovery-owning process (the desktop \
                         app's next start) relabels it, or `pinvou knowledge index cancel \
                         {job_id}` drops it"
                    )
                };
                return Err(CliError::failed(format!(
                    "{operation}: index job {job_id} status stayed unreadable across \
                     {READ_ERROR_TOLERANCE} consecutive polls ({error}); the wait cannot own \
                     the job to completion — {remedy}",
                )));
            }
        }
    }
}

/// The observable-progress signature of an in-flight job (see
/// [`wait_for_terminal_job`]). Owned so the polling loop can hold it across
/// the re-assignment of the state it was computed from. The job-row
/// `updated_at` leads the tuple on purpose (round-37 review MAJOR): the
/// source-root walk and the embedder-model load run before any item exists,
/// so the per-item counters cannot move — the importer ticks the job row
/// during the walk (a pre-prune raw heartbeat every 5000 enumerated entries)
/// and at each item claim. The model load itself
/// has NO tick (nothing can run beside it in-process): a load outlasting
/// the bound is interrupted and stays resumable — the remedy text names
/// that quiet phase, and round-40 review scoped the docstrings claiming
/// full coverage down to this reality. A single file whose parse alone
/// outlasts the bound is the third quiet phase (the staged counters only
/// move once the parse finishes); the stall report names it and carries the
/// in-flight path.
fn job_signature(state: &IndexState) -> (Option<i64>, u64, u64, u64, u64, Option<String>) {
    (
        state.updated_at,
        state.done,
        state.total,
        state.failed,
        state.current_chunks_done,
        state.current_path.clone(),
    )
}

/// Reports a terminal import state with a phase-honest header and exit code:
/// `done` is success; `interrupted`, `cancelled` and `done_with_errors` mean
/// the import did NOT complete, so they exit 1 while still printing the full
/// state (human and JSON) the way `index status` would.
fn index_finished(state: &IndexState, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Round-37 review: an empty harvest must not read as silent success —
    // scan pre-flights guard this for the scan lane; imports had no
    // equivalent. Stderr only, so JSON consumers are unaffected.
    if display_phase(state) == "done" && state.total == 0 {
        note!(
            "note: knowledge index: the sources yielded 0 importable files (nothing was \
             added; check the exclude rules if this is unexpected)"
        );
    }
    let (header, failed) = match display_phase(state).as_str() {
        "interrupted" => (
            "index interrupted (staged progress survives; re-run `pinvou knowledge index \
             resume <job-id>` to continue)",
            true,
        ),
        "cancelled" => ("index cancelled", true),
        "done_with_errors" => (
            "index completed with errors (see `pinvou knowledge index failed <job-id>` \
             or `index retry <job-id> <item-id>`)",
            true,
        ),
        _ => ("index completed", false),
    };
    let human = format!("{header}\n{}", render_index_state(state));
    let mut value = serde_json::to_value(state).unwrap_or_default();
    value["phase"] = serde_json::json!(display_phase(state));
    Ok(CliOutcome {
        exit_code: if failed {
            ExitCode::Failed
        } else {
            ExitCode::Success
        },
        stdout: render(output, human, &value),
    })
}

fn index_out(header: &str, state: IndexState, output: OutputMode) -> Result<CliOutcome, CliError> {
    let human = format!("{header}\n{}", render_index_state(&state));
    let mut value = serde_json::to_value(&state).unwrap_or_default();
    value["phase"] = serde_json::json!(display_phase(&state));
    Ok(success(render(output, human, &value)))
}

/// The app's job state carries no phase string; derive the CLI's display
/// phase from the surviving flags so the command's output contract (human
/// and JSON) stays stable across the app's state redesign. `cancelled` is
/// part of that contract: without it a cancelled job (running=false with a
/// job_id) is indistinguishable from a finished one, and a client polling
/// until `done` would treat a cancelled import as completed.
fn display_phase(state: &IndexState) -> String {
    if state.running {
        "running".into()
    } else if state.resumable {
        "interrupted".into()
    } else if state.cancelled {
        "cancelled".into()
    } else if state.job_id.is_some() {
        if state.failed > 0 {
            "done_with_errors".into()
        } else {
            "done".into()
        }
    } else {
        "idle".into()
    }
}

#[cfg(test)]
mod index_state_result_tests {
    use super::index_state_result;
    use crate::ExitCode;

    /// Round-46 review: the fresh-foreign-running refusal (a surface
    /// starting an import inside the `require_job_id`→command race) was the
    /// one codeless error class this lane could surface; the mapper must
    /// pin its stable code to the single-sourced app marker.
    #[test]
    fn the_foreign_running_refusal_maps_to_a_stable_code() {
        let error = index_state_result(Err(
            pinvou3_lib::features::knowledge::FOREIGN_IMPORT_RUNNING_MARKER.to_owned(),
        ))
        .unwrap_err();
        assert_eq!(error.exit_code(), ExitCode::Failed);
        let rendered = error.to_string();
        assert!(rendered.starts_with("knowledge_index_busy"), "{rendered}");
        // The marker stays visible so the locale it came from is not hidden.
        assert!(
            rendered.contains(pinvou3_lib::features::knowledge::FOREIGN_IMPORT_RUNNING_MARKER),
            "{rendered}"
        );
        // Unrelated store errors keep the generic family prefix.
        let generic = index_state_result(Err("no such table: jobs".to_owned())).unwrap_err();
        assert!(
            generic.to_string().starts_with("knowledge index:"),
            "{generic}"
        );
    }
}

#[cfg(test)]
mod phase_tests {
    use super::{IndexState, display_phase};

    fn state(
        running: bool,
        resumable: bool,
        cancelled: bool,
        job_id: Option<&str>,
        failed: u64,
    ) -> IndexState {
        IndexState {
            job_id: job_id.map(str::to_owned),
            running,
            resumable,
            cancelled,
            collection_id: 1,
            done: 2,
            total: 3,
            failed,
            current_path: None,
            current_chunks_done: 0,
            current_chunks_total: 0,
            updated_at: None,
            failed_files: Vec::new(),
        }
    }

    #[test]
    fn display_phase_covers_every_state_vocabulary() {
        assert_eq!(
            display_phase(&state(true, false, false, Some("j"), 0)),
            "running"
        );
        assert_eq!(
            display_phase(&state(false, true, false, Some("j"), 1)),
            "interrupted"
        );
        assert_eq!(
            display_phase(&state(false, false, true, Some("j"), 1)),
            "cancelled",
            "a cancelled job must not masquerade as done (or done_with_errors)"
        );
        assert_eq!(
            display_phase(&state(false, false, false, Some("j"), 2)),
            "done_with_errors"
        );
        assert_eq!(
            display_phase(&state(false, false, false, Some("j"), 0)),
            "done"
        );
        assert_eq!(display_phase(&state(false, false, false, None, 0)), "idle");
    }
}

fn render_index_state(state: &IndexState) -> String {
    let phase = display_phase(state);
    let mut lines = vec![
        format!("job: {}", state.job_id.as_deref().unwrap_or("none")),
        format!("phase: {phase}"),
        format!("running: {}", state.running),
        format!("resumable: {}", state.resumable),
        format!("progress: {}/{}", state.done, state.total),
        format!("failed: {}", state.failed),
    ];
    if let Some(path) = &state.current_path {
        // The path originates from DB import rows (a Unix filename may
        // legally carry tab/newline/ESC): collapse it like every other
        // human cell in this CLI. JSON keeps the verbatim path.
        lines.push(format!(
            "current: {}",
            crate::support::collapse_control_characters(path)
        ));
    }
    lines.join("\n")
}

/// GUI `kb_index_failed_files`; `--limit` omitted keeps the GUI page size 50.
fn index_failed(
    job_id: &str,
    offset: usize,
    limit: Option<usize>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let service = open_service()?;
    let page = service
        .failed_index_files(job_id, offset, limit.unwrap_or(50))
        .map_err(|error| {
            // Upstream `ImportJobStore::failed_files_page` answers an unknown
            // job id with rusqlite's `QueryReturnedNoRows`; name the real
            // cause instead of leaking the raw driver message (the same
            // mapping `index resume`/`index retry` use).
            if error.contains("Query returned no rows") {
                CliError::failed(format!(
                    "knowledge_index_job_not_found: no index job {job_id} exists"
                ))
            } else {
                feature_error("index failed", error)
            }
        })?;
    let mut lines = if page.files.is_empty() {
        vec![format!("no failed files for job {job_id}")]
    } else {
        page.files
            .iter()
            .map(|file| {
                // Remote/user-sourced cells: collapsed like every other
                // family's tab rows, so a name carrying a control character
                // cannot forge extra rows or columns in the human block.
                format!(
                    "{}\t{}\t{}",
                    file.item_id,
                    crate::support::collapse_control_characters(&file.name),
                    crate::support::collapse_control_characters(&file.error)
                )
            })
            .collect::<Vec<_>>()
    };
    if let Some(next) = page.next_offset {
        lines.push(format!("next_offset: {next}"));
    }
    let value = serde_json::to_value(&page).unwrap_or_default();
    Ok(success(render(output, lines.join("\n"), &value)))
}

// ───────────────────────── model ─────────────────────────

/// The GUI's `kb_model_status` needs `tauri::State`, so the CLI reports what
/// a one-shot process can know: the on-disk completeness mirror, the
/// service's real readiness, and the version.
///
/// The whole payload is process-local and JSON says so with a
/// `"scope": "process-local"` marker.
/// `ready` is the reason the marker is not optional: it is
/// `semantic_ready()`, i.e. "is the ~570 MB ONNX model loaded IN THIS
/// PROCESS", and a one-shot CLI never loads it — so it reads `false` even
/// when the model is fully deployed and resident in the desktop app. Without
/// the marker a script gating on `.ready` would never proceed and could not
/// tell why; `.installed` is the field to gate deployment on. The field is
/// kept rather than dropped because it is the honest answer for this process
/// (and becomes meaningful the day a headless loader exists), and dropping a
/// published field would break consumers for no gain. `model_dir`/`installed`
/// are process-local too: `configured_model_dir` reads this process's
/// `PINVOU3_KB_EMBED_MODEL_DIR`.
fn model_status(output: OutputMode) -> Result<CliOutcome, CliError> {
    let service = open_service()?;
    let dir = configured_model_dir();
    let installed = pinvou3_lib::features::knowledge::model_directory_is_complete(&dir);
    let ready = service.semantic_ready();
    let human = format!(
        "version: {MODEL_VERSION}\nmodel_dir: {}\ninstalled: {installed}\nready: {ready}\n\
         note: ready is process-local (a one-shot CLI never loads the model, so it reads \
         false even when the app has it resident); gate on installed\n\
         note: model download and load run in the desktop app process",
        dir.display()
    );
    Ok(success(render(
        output,
        human,
        &serde_json::json!({
            "version": MODEL_VERSION,
            "model_dir": dir.display().to_string(),
            "installed": installed,
            "ready": ready,
            "scope": "process-local",
        }),
    )))
}

/// Same resolution order as the GUI (`configured_model_dir` in
/// model_download.rs): the `PINVOU3_KB_EMBED_MODEL_DIR` override first, then
/// the managed `model_dir()`.
fn configured_model_dir() -> PathBuf {
    std::env::var("PINVOU3_KB_EMBED_MODEL_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(model_dir)
}

/// The upstream `kb_model_cancel` sets a process-local flag inside the
/// desktop app's download orchestration (`features/knowledge/
/// model_download.rs`). A one-shot CLI process can never have a download of
/// its own in flight (`model download` is honestly unavailable headless),
/// so the flag it sets is read by nobody and the `cancelled: true` the
/// command used to print was a no-op reported as success. Same honest
/// refusal pattern as `model download` itself.
fn model_cancel(_output: OutputMode) -> Result<CliOutcome, CliError> {
    Err(CliError::failed(
        "knowledge_model_cancel_requires_product_host: the download cancel flag is \
         process-local to the desktop app's model_download orchestration, and a one-shot \
         CLI process never has a download of its own in flight (`model download` is \
         unavailable headless — see `model status`) — nothing in this process can be \
         cancelled; cancel a download in the desktop app",
    ))
}

// ───────────────────────── remote / host ─────────────────────────
//
// The remote/host surfaces are async network client calls behind the bare
// async host (`run_bare_host`, whose error type is `anyhow`); both are
// product-backend-only, so the whole cluster is cfg-gated with honest
// featureless refusals below (the `agent_task` family's stub precedent).

/// Loads the persisted remote connections (offline file read; a missing
/// file means no connections, like the GUI's fresh state).
#[cfg(feature = "product-backend")]
fn open_remote_service() -> Result<RemoteKnowledgeService, CliError> {
    sandbox_home()?;
    RemoteKnowledgeService::load(RemoteKnowledgeService::default_path())
        .map_err(|error| CliError::failed(format!("knowledge remote connections: {error}")))
}

/// GUI `remote_kb_connections`: probe every configured connection (network).
/// With zero configured connections this answers offline without booting the
/// windowless host.
#[cfg(feature = "product-backend")]
fn remote_connections(output: OutputMode) -> Result<CliOutcome, CliError> {
    let service = open_remote_service()?;
    if service.configured_connections().is_empty() {
        return empty_remote_connections(output);
    }
    let statuses = pinvou3_lib::headless_bridge::run_bare_host(move || async move {
        service
            .statuses()
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))
    })
    .map_err(|error| host_error("remote connections", error))?;
    let human = if statuses.is_empty() {
        "no remote knowledge connections".to_owned()
    } else {
        statuses
            .iter()
            .map(|status| {
                let base = format!(
                    "{}\t{}\t{}\tonline={}\tready={}",
                    status.connection.server_id,
                    crate::support::collapse_control_characters(&status.connection.name),
                    crate::support::collapse_control_characters(&status.connection.endpoint),
                    status.online,
                    status.ready
                );
                match &status.error {
                    // Server-originated error text: collapse like every other
                    // remote-derived cell so it cannot forge rows/columns.
                    Some(error) => format!(
                        "{base}\t{}",
                        crate::support::collapse_control_characters(error)
                    ),
                    None => base,
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let value = serde_json::json!({
        "connections": serde_json::to_value(&statuses).unwrap_or_default(),
    });
    Ok(success(render(output, human, &value)))
}

#[cfg(feature = "product-backend")]
fn empty_remote_connections(output: OutputMode) -> Result<CliOutcome, CliError> {
    Ok(success(render(
        output,
        "no remote knowledge connections".to_owned(),
        &serde_json::json!({ "connections": [] }),
    )))
}

/// GUI `remote_kb_probe_private_endpoint`: the unauthenticated TLS-pinned
/// identity handshake, re-exported by the app crate as
/// `features::remote_knowledge::probe_private_identity`. Async network call →
/// the bare async host, like the other remote surfaces. The human output
/// shows the confirmable identity fields; JSON carries the full probe
/// (public CA identity material — the GUI requires out-of-band confirmation
/// of the identity code before any join).
#[cfg(feature = "product-backend")]
fn remote_probe(url: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    let source = url.to_owned();
    let probe = pinvou3_lib::headless_bridge::run_bare_host(move || async move {
        pinvou3_lib::features::remote_knowledge::probe_private_identity(&source)
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))
    })
    .map_err(|error| host_error("remote probe", error))?;
    // Collapsed like every other remote-derived cell in this family: the
    // probed endpoint controls these strings, and this is exactly the
    // output the user is told to confirm out-of-band, so a forged label
    // line (`ready: true`, a fake identity code) must not be renderable
    // with control characters. JSON keeps the verbatim values.
    let human = format!(
        "endpoint: {}\nserver: {}\nserver_id: {}\nidentity_code: {}\nnetwork: {:?}\nready: {}\n\
         note: confirm the identity code out-of-band before joining",
        crate::support::collapse_control_characters(&probe.endpoint),
        crate::support::collapse_control_characters(&probe.server_name),
        crate::support::collapse_control_characters(&probe.server_id),
        crate::support::collapse_control_characters(&probe.identity_code),
        probe.network_kind,
        probe.ready
    );
    let value = serde_json::to_value(&probe).unwrap_or_default();
    Ok(success(render(output, human, &value)))
}

/// GUI `remote_kb_collections(server_id, include_deleted)`; the CLI has no
/// server argument in its fixed surface, so it queries every configured
/// connection. Per-connection failures are reported inline; the command only
/// fails when no connection answered.
#[cfg(feature = "product-backend")]
fn remote_collections(output: OutputMode) -> Result<CliOutcome, CliError> {
    let service = open_remote_service()?;
    let connections = service.configured_connections();
    if connections.is_empty() {
        return Err(CliError::failed(
            "knowledge remote collections: no remote knowledge connections configured \
             (pair with a server from the desktop app)",
        ));
    }
    let mut lines = Vec::new();
    let mut results = Vec::new();
    let mut errors = Vec::new();
    let pages = pinvou3_lib::headless_bridge::run_bare_host(move || async move {
        let mut pages = Vec::new();
        for connection in connections {
            match service.collections(&connection.server_id, false).await {
                Ok(collections) => {
                    pages.push((connection.server_id, connection.name, Ok(collections)))
                }
                Err(error) => pages.push((connection.server_id, connection.name, Err(error))),
            }
        }
        Ok(pages)
    })
    .map_err(|error| host_error("remote collections", error))?;
    for (server_id, name, page) in pages {
        match page {
            Ok(collections) => {
                for collection in &collections {
                    lines.push(format!(
                        "{}\t{}\t{}\tdocs={}",
                        // Round-44 review: server_id takes the same collapse
                        // discipline as the name cell — a misbehaving server
                        // must not be able to forge rows/columns through the
                        // connection's stored id. (The collection id is an
                        // i64 from the wire schema; JSON keeps the raw
                        // values.)
                        crate::support::collapse_control_characters(&server_id),
                        collection.id,
                        crate::support::collapse_control_characters(&collection.name),
                        collection.doc_count
                    ));
                }
                if collections.is_empty() {
                    lines.push(format!(
                        "{}\tno remote collections",
                        crate::support::collapse_control_characters(&server_id)
                    ));
                }
                results.push(serde_json::json!({
                    "server_id": server_id,
                    "name": name,
                    "collections": serde_json::to_value(&collections).unwrap_or_default(),
                }));
            }
            Err(error) => {
                lines.push(format!(
                    "{}\terror: {}",
                    crate::support::collapse_control_characters(&server_id),
                    crate::support::collapse_control_characters(&error)
                ));
                errors.push(serde_json::json!({ "server_id": server_id, "error": error }));
            }
        }
    }
    if results.is_empty() {
        return Err(CliError::failed(format!(
            "knowledge remote collections: {}",
            errors
                .iter()
                .filter_map(|error| error["error"].as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    let human = if lines.is_empty() {
        "no remote collections".to_owned()
    } else {
        lines.join("\n")
    };
    Ok(success(render(
        output,
        human,
        &serde_json::json!({ "results": results, "errors": errors }),
    )))
}

/// GUI `remote_kb_search(server_id, collection_ids, query, limit)` with the
/// GUI's default limit of 8; the CLI's `collection` argument is a name, so
/// it is resolved against each server's collections first.
#[cfg(feature = "product-backend")]
fn remote_search(
    collection: &str,
    query: &str,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let service = open_remote_service()?;
    let connections = service.configured_connections();
    if connections.is_empty() {
        return Err(CliError::failed(
            "knowledge remote search: no remote knowledge connections configured \
             (pair with a server from the desktop app)",
        ));
    }
    let wanted = collection.to_owned();
    let remote_query = query.to_owned();
    let mut lines = Vec::new();
    let mut results = Vec::new();
    let mut errors = Vec::new();
    let outcomes = pinvou3_lib::headless_bridge::run_bare_host(move || async move {
        let mut outcomes = Vec::new();
        for connection in connections {
            let server_id = connection.server_id.clone();
            let outcome = match service.collections(&server_id, false).await {
                Ok(collections) => match collections.iter().find(|item| item.name == wanted) {
                    Some(matched) => {
                        let collection_id = matched.id;
                        service
                            .search(&server_id, vec![collection_id], remote_query.clone(), 8)
                            .await
                            .map(|hits| (Some(collection_id), Some(hits), None))
                            .unwrap_or_else(|error| (Some(collection_id), None, Some(error)))
                    }
                    None => (None, None, Some(format!("collection {wanted} not found"))),
                },
                Err(error) => (None, None, Some(error)),
            };
            outcomes.push((server_id, outcome));
        }
        Ok(outcomes)
    })
    .map_err(|error| host_error("remote search", error))?;
    for (server_id, (collection_id, hits, error)) in outcomes {
        match (hits, error) {
            (Some(hits), None) => {
                for hit in &hits {
                    lines.push(format!(
                        "{}\t{}\tscore={:.3}\t{}",
                        server_id,
                        crate::support::collapse_control_characters(&hit.document_name),
                        hit.score,
                        crate::support::collapse_control_characters(
                            &hit.text.chars().take(120).collect::<String>()
                        )
                    ));
                }
                if hits.is_empty() {
                    lines.push(format!("{server_id}\tno hits"));
                }
                results.push(serde_json::json!({
                    "server_id": server_id,
                    "collection_id": collection_id,
                    "hits": serde_json::to_value(&hits).unwrap_or_default(),
                }));
            }
            (_, Some(error)) => {
                lines.push(format!(
                    "{server_id}\terror: {}",
                    crate::support::collapse_control_characters(&error)
                ));
                errors.push(serde_json::json!({ "server_id": server_id, "error": error }));
            }
            _ => {}
        }
    }
    if results.is_empty() {
        return Err(CliError::failed(format!(
            "knowledge remote search: {}",
            errors
                .iter()
                .filter_map(|error| error["error"].as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    let human = if lines.is_empty() {
        "no hits".to_owned()
    } else {
        lines.join("\n")
    };
    Ok(success(render(
        output,
        human,
        &serde_json::json!({ "query": query, "results": results, "errors": errors }),
    )))
}

/// GUI `shared_kb_host_status`; the snapshot function is async (systemd
/// probe + optional local health check), so it runs on the bare async host
/// (`run_bare_host`: no Tauri context, no session-store boot, no display —
/// usable on headless Linux).
#[cfg(feature = "product-backend")]
fn host_status(output: OutputMode) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    let status = pinvou3_lib::headless_bridge::run_bare_host(move || async move {
        Ok::<_, anyhow::Error>(pinvou3_lib::features::shared_knowledge_host::status().await)
    })
    .map_err(|error| host_error("host status", error))?;
    // Same collapse discipline as the probe block: the endpoint is
    // server-controlled and lands in a label the user reads; JSON keeps
    // the verbatim value.
    let human = format!(
        "supported: {}\ninstalled: {}\nendpoint: {}\nservice_version: {}\n\
         app_version: {}\nupgrade_available: {}\nclient_outdated: {}",
        status.supported,
        status.installed,
        crate::support::collapse_control_characters(&status.endpoint),
        status
            .service_version
            .as_deref()
            .map(crate::support::collapse_control_characters)
            .unwrap_or_else(|| "none".to_owned()),
        status.app_version,
        status.upgrade_available,
        status.client_outdated
    );
    let value = serde_json::to_value(&status).unwrap_or_default();
    Ok(success(render(output, human, &value)))
}

/// The remote/host surfaces are async network client calls; run them on the
/// bare async host (`run_bare_host`: rustls/env/runtime only — no Tauri
/// context, no session-store boot, no display; documented in the module
/// docs and the `#[ignore]`d tests). The closures are zero-argument: the
/// lanes need no engine pool or session store.
#[cfg(feature = "product-backend")]
fn host_error(operation: &str, error: anyhow::Error) -> CliError {
    CliError::failed(format!("knowledge {operation}: {error:#}"))
}

// Featureless refusals for the remote/host cluster (the `agent_task`
// family's stub precedent): the bare async host these commands bootstrap
// is a product-backend capability, and a featureless build links neither
// it nor `anyhow`.
#[cfg(not(feature = "product-backend"))]
fn remote_connections(_output: OutputMode) -> Result<CliOutcome, CliError> {
    Err(CliError::failed("product_backend_not_enabled"))
}

#[cfg(not(feature = "product-backend"))]
fn remote_probe(_url: &str, _output: OutputMode) -> Result<CliOutcome, CliError> {
    Err(CliError::failed("product_backend_not_enabled"))
}

#[cfg(not(feature = "product-backend"))]
fn remote_collections(_output: OutputMode) -> Result<CliOutcome, CliError> {
    Err(CliError::failed("product_backend_not_enabled"))
}

#[cfg(not(feature = "product-backend"))]
fn remote_search(
    _collection: &str,
    _query: &str,
    _output: OutputMode,
) -> Result<CliOutcome, CliError> {
    Err(CliError::failed("product_backend_not_enabled"))
}

#[cfg(not(feature = "product-backend"))]
fn host_status(_output: OutputMode) -> Result<CliOutcome, CliError> {
    Err(CliError::failed("product_backend_not_enabled"))
}

/// The mount surface refuses honestly, same pattern as `model download`:
/// mounted collections live in the desktop app's per-process memory
/// (`features::sessions::mode_state` — deliberately not persisted), so a
/// one-shot CLI process can neither observe nor durably mutate them. A
/// command that printed success here would change nothing.
fn mount_requires_product_host(action: &str, session_id: &str) -> CliError {
    CliError::failed(format!(
        "knowledge_{action}_requires_product_host: mounted collections live in the running \
         desktop app's process memory and are deliberately not persisted, so a CLI process \
         can neither read nor change session {session_id}'s mounts; mount collections in the \
         app's session knowledge panel"
    ))
}

/// GUI `session_mounted_collections_snapshot`: the revisioned source of truth
/// for one session's mounts. Every invocation is refused honestly — mounts
/// live in the desktop app's process memory and are deliberately not
/// persisted, so the CLI can neither read nor change them.
///
/// The refusal is returned BEFORE any store is opened. It does not depend on
/// the session at all, and `SessionStore::boot()` is not a read: it enforces
/// the 50-sessions-per-kind retention policy, which irreversibly deletes the
/// user's oldest non-pinned sessions. Booting it to decorate an unavoidable
/// refusal with "session not found" would destroy chat history as a side
/// effect of a command that cannot succeed. The session-id charset gate still
/// runs first, in [`require_session_id`] at parse time (exit 2), so a
/// traversal-shaped id never reaches here.
fn mounts(session_id: &str, _output: OutputMode) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    Err(mount_requires_product_host("mounts", session_id))
}

/// Same pre-boot refusal as [`mounts`]: a mutation the CLI cannot perform
/// must not evict sessions on its way to saying so.
fn mount(
    session_id: &str,
    _collection_id: i64,
    _output: OutputMode,
) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    Err(mount_requires_product_host("mount", session_id))
}

/// Same pre-boot refusal as [`mounts`].
fn unmount(
    session_id: &str,
    _collection_id: i64,
    _output: OutputMode,
) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    Err(mount_requires_product_host("unmount", session_id))
}
