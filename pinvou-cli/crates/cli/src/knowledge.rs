//! `knowledge` family: knowledge-base surface mirroring the GUI commands in
//! `pinvou3-app/src-tauri/src/app/commands/knowledge.rs` (kb_* and the
//! session-mount commands) plus `remote_knowledge` and `shared_knowledge_host`.
//!
//! `features::knowledge`, `features::remote_knowledge` and
//! `features::shared_knowledge_host` are `pub` in the app crate, so the CLI
//! constructs the feature types directly (no Tauri host for the storage
//! paths):
//! - `KnowledgeService::new(db_path)` is a standalone constructor over
//!   `~/.pinvou3/knowledge/index.db` (`default_db_path()` honours
//!   `PINVOU3_HOME`); it also runs the GUI's startup recovery of interrupted
//!   imports.
//! - scan start/status/cancel → `KnowledgeService::{start_scan, status,
//!   cancel_scan}`; `--root` omitted defaults to the user home like
//!   `kb_start_scan`.
//! - collections list/create/update/delete → `KnowledgeService::l1()`
//!   (`L1Store` CRUD). Delete mirrors GUI `kb_collection_delete`:
//!   `cancel_index_for_collection`, `delete_collection`, then
//!   `SessionStore::remove_mounted_collection_from_all` (now only reached
//!   through the desktop app's own surface).
//! - add-sources → `KnowledgeService::start_index` (non-blocking; returns the
//!   DB-persisted job state that `index status` polls).
//! - documents → `L1Store::{list_documents, remove_document}` (`--limit`
//!   omitted maps to the GUI's 0 = default page of 500).
//! - index status/cancel/resume/retry/failed → `KnowledgeService::
//!   {index_status, cancel_index, resume_index, retry_index_item,
//!   failed_index_files}`; `--limit` for failed files defaults to the GUI
//!   page size 50. `index status` without an id reports the GUI's latest job
//!   (`kb_index_status` semantics); with an id it reports THAT job through
//!   `KnowledgeService::index_job_state`, because the latest-job ordering
//!   ranks `cancelled` last and would answer with an older `done_with_errors`
//!   job instead. `index cancel` reports the cancelled job through the same
//!   per-job read for exactly that reason.
//!   `index cancel/resume/retry <job-id>` validate the named id
//!   against that latest job BEFORE the recovering open: boot recovery would
//!   flip every preparing/running job to interrupted — including one a live
//!   desktop-app process is still importing — so a mistyped id must fail
//!   without ever running it.
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
//!   client calls, so they run through the windowless product host
//!   (`pinvou3_lib::headless_bridge::run_windowless_host`, needs a display on
//!   headless Linux like `agent run`); zero configured connections answers
//!   offline without booting the host.
//! - remote probe → `features::remote_knowledge::probe_private_identity`
//!   (the TLS-pinned identity handshake behind the GUI's
//!   `remote_kb_probe_private_endpoint`, re-exported as a free function;
//!   `RemoteKnowledgeProbe` was already public via `request_join_confirmed`,
//!   so no new types entered the app crate's surface). Async network call →
//!   the same windowless product host.
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
//!   `model cancel` calls the real `kb_model_cancel` (process-local by
//!   nature).
//!
//! One-shot semantics: import jobs run on in-process background threads.
//! The CLI prints the start state and exits; jobs are
//! DB-persisted and come back as interrupted/resumable (`index resume`).
//! `scan start` is the exception: it waits for the scan to finish inside
//! the invocation (a fire-and-forget scan would be killed by process exit
//! before doing any work), under a no-progress liveness bound, and
//! pre-flights the root so a typo fails loudly instead of reporting a
//! cheerful `done` over zero files. The pre-flight is usability only: index
//! safety is the sweep's own (a root the walk could not reach never
//! authorizes deletion — `features::knowledge::root_authorizes_deletion`),
//! because the pre-flight is inherently TOCTOU.
//! Every CLI invocation constructs the service fresh; read-only
//! commands open it WITHOUT the GUI's startup recovery, so inspecting the
//! store cannot degrade an import a live desktop-app process is still
//! running. The remaining recovery openers are the commands that act on the
//! import-job store itself (collection delete/add-sources,
//! resume/retry/cancel), which
//! converts `preparing`/`running` jobs to interrupted/resumable — including
//! a job that a live desktop-app process is executing RIGHT NOW (there is
//! no cross-process owner heartbeat in the job store). Recovery itself is
//! transaction-safe (per-item staged-chunk checkpoints, so no index
//! corruption and no duplicated work), but it wedges the app's live job
//! into interrupted and needs a manual `index resume`; do not run CLI
//! write commands while the desktop app is mid-import. Because a one-shot
//! process kills its background thread at exit, `add-sources`/`resume`/
//! `retry` only make progress while the process lives — completing a large
//! import needs the desktop app (or a future long-lived daemon); the CLI
//! honestly reports the persisted job state throughout.

use std::path::{Path, PathBuf};

use crate::support::{render, require_yes, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::knowledge::model_download::{MODEL_VERSION, kb_model_cancel};
use pinvou3_lib::features::knowledge::{
    IndexState, KnowledgeService, ScanState, SearchQueryDto, default_db_path, model_dir,
};
use pinvou3_lib::features::remote_knowledge::RemoteKnowledgeService;
use pinvou3_lib::features::sessions::SessionStore;

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
    value
        .parse::<usize>()
        .map_err(|_| CliError::usage(format!("knowledge {name} must be a non-negative integer")))
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
/// `~/.pinvou3/knowledge/index.db` (temp-`PINVOU3_HOME` aware); the
/// constructor also performs the GUI's startup recovery of interrupted
/// imports.
/// Opens the store WITHOUT boot-time recovery. Read-only commands must not
/// degrade an import a live desktop-app process is still running: the
/// upstream recovery flips any preparing/running job to terminal state, and
/// `index resume` could then re-arm it under the still-alive owner. Recovery
/// stays with the write commands ([`open_service_recovering`]).
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

/// Opens the store WITH boot-time recovery of jobs orphaned by a crashed
/// process — the write/maintenance commands that legitimately own the
/// reconciliation.
fn open_service_recovering() -> Result<KnowledgeService, CliError> {
    sandbox_home()?;
    let db = default_db_path();
    KnowledgeService::new(&db).map_err(|error| {
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
    // This is the ONLY caller that blocks on `running`, so it also owns the
    // liveness bound. The scan thread now clears the flag even when it panics
    // (`features::knowledge::start_scan`), but a wait with no bound at all
    // turns any future way of losing that thread into a terminal that hangs
    // with no output and no exit code. The bound is no-progress, not
    // wall-clock: a full home scan legitimately runs for many minutes while
    // `scanned` keeps advancing (the walker reports every 5000 entries and
    // once per root), so "still running and not counting" is the honest
    // signature of a lost thread.
    let mut last_scanned = service.status().scanned;
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
                     the completion marker was not persisted and no stale entries were \
                     removed — re-run the command",
                    state.scanned
                )));
            }
            return scan_out("scan completed (process-local)", state, output);
        }
        if state.scanned != last_scanned {
            last_scanned = state.scanned;
            last_progress = std::time::Instant::now();
        } else if last_progress.elapsed() >= SCAN_NO_PROGRESS_TIMEOUT {
            return Err(CliError::failed(format!(
                "knowledge scan start: the scan is still flagged running but reported no \
                 progress for {}s (scanned: {}); the scan thread is gone or wedged — \
                 partial results may be indexed and the completion marker was not \
                 persisted",
                SCAN_NO_PROGRESS_TIMEOUT.as_secs(),
                state.scanned
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// How long `scan start` keeps waiting on a scan that is flagged running but
/// has stopped counting. Generous on purpose: the walker only reports every
/// 5000 entries, and a cold spinning disk or a slow network mount can spend
/// minutes between two reports without being stuck.
const SCAN_NO_PROGRESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

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

fn scan_cancel(output: OutputMode) -> Result<CliOutcome, CliError> {
    let service = open_service()?;
    service.cancel_scan();
    // The GUI polls scan status afterwards; a one-shot CLI reports the signal
    // only (the in-memory scan dies with the process, and the next
    // incremental `scan start` re-runs).
    Ok(success(render(
        output,
        "scan cancel signalled (process-local: the CLI can only signal cancels inside its own process; use the desktop app to cancel an app-side scan)"
            .to_owned(),
        &serde_json::json!({ "cancelled": true, "scope": "process-local" }),
    )))
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
                state.roots.join(", ")
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
            .map(|count| format!("{}\t{}", count.ext, count.count))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let value = serde_json::json!({
        "typeCounts": serde_json::to_value(&counts).unwrap_or_default(),
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
                format!(
                    "{}\t{}\t{}\t{}\t{}",
                    hit.name,
                    hit.ext.as_deref().unwrap_or("-"),
                    hit.size,
                    hit.mtime,
                    hit.path
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
/// `mtimeAfter`/`mtimeBefore` (the GUI frontend sends epoch seconds directly;
/// the civil-days conversion is inlined deliberately — chrono IS a
/// dependency, but this arithmetic must keep matching the foundation's
/// day-boundary computation exactly, the same rationale as the scheduled
/// family's copy). Invalid shapes are usage errors, like bad ids.
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
                format!(
                    "{}\t{}\t{}\tdocs={}\tchunks={}\tbytes={}",
                    collection.id,
                    collection.name,
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
    let Some(existing) = collections.iter().find(|collection| collection.id == id) else {
        return Err(CliError::failed(format!(
            "knowledge collections update: collection {id} not found"
        )));
    };
    let name = name.unwrap_or_else(|| existing.name.clone());
    let category = category.or_else(|| existing.category.clone());
    let description = description.or_else(|| existing.description.clone());
    service
        .l1()
        .update_collection(id, &name, category.as_deref(), description.as_deref())
        .map_err(|error| feature_error("collections update", error))?;
    Ok(success(render(
        output,
        format!("updated collection {id}"),
        &serde_json::json!({
            "id": id,
            "name": name,
            "category": category,
            "description": description,
        }),
    )))
}

/// The collection existence gate the destructive/recovering commands run
/// BEFORE the recovering open: boot recovery flips every preparing/running
/// job to interrupted — including one a live desktop-app process is still
/// importing — so a mistyped id must fail without ever running it (the same
/// rule `index cancel/resume/retry` implement on their latest-job read).
fn ensure_collection_exists(
    service: &KnowledgeService,
    id: i64,
    operation: &str,
) -> Result<(), CliError> {
    match service.l1().collection_name(id) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(CliError::failed(format!(
            "knowledge {operation}: collection {id} not found"
        ))),
        Err(error) => Err(feature_error(operation, error)),
    }
}

/// GUI `kb_collection_delete`: cancel a running import for the collection,
/// delete it, then clear every session mount.
fn collections_delete(id: i64, output: OutputMode) -> Result<CliOutcome, CliError> {
    // The existence check runs on a plain open first — recovery must not
    // fire for a mistyped id — and again on the mutating open, so a
    // collection deleted in between fails honestly instead of reporting a
    // silent no-op as "deleted".
    ensure_collection_exists(&open_service()?, id, "collections delete")?;
    let service = open_service_recovering()?;
    ensure_collection_exists(&service, id, "collections delete")?;
    service
        .cancel_index_for_collection(id)
        .map_err(|error| feature_error("collections delete", error))?;
    service
        .l1()
        .delete_collection(id)
        .map_err(|error| feature_error("collections delete", error))?;
    // The mount table lives in the app's in-process session memory, so this
    // store-level sweep is empty in a one-shot CLI process; mounted
    // collections in a running desktop app are cleaned up on its next mode
    // load, not here. Best-effort: the collection is already deleted, so a
    // session-store failure must not turn the outcome into a failure (an
    // exit 1 after the destructive step would claim the delete did not
    // happen).
    // The stderr warning stays static: the CodeQL cleartext-logging gate
    // flags interpolated store details in log writes, so the error text
    // travels in the JSON payload instead.
    let (unmounted, mount_sweep_error) = match open_store() {
        Ok(store) => (store.remove_mounted_collection_from_all(id), None),
        Err(error) => {
            note!(
                "warning: knowledge collections delete: could not sweep session mounts \
                 for the deleted collection; stale mounts may remain in a running \
                 desktop app session"
            );
            (Vec::new(), Some(error.to_string()))
        }
    };
    let mut human = format!("deleted collection {id}");
    if !unmounted.is_empty() {
        human.push_str(&format!("\nunmounted from {} session(s)", unmounted.len()));
    } else {
        human.push_str(
            "\nnote: mounts held in a running desktop app session are unaffected by this \
             process",
        );
    }
    Ok(success(render(
        output,
        human,
        &serde_json::json!({
            "id": id,
            "unmounted_sessions": unmounted.len(),
            "mount_sweep_error": mount_sweep_error,
        }),
    )))
}

/// GUI `kb_collection_add_sources`: persists the import job and returns its
/// state; the actual parsing/chunking/ingestion runs on a background thread
/// that `knowledge index status` polls. The GUI frontend only offers existing
/// collections, so the CLI adds the existence check the API silently assumes
/// (upstream returns an idle no-job state for unknown ids).
fn collections_add_sources(
    id: i64,
    paths: Vec<PathBuf>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    // Validate everything a mistyped invocation could get wrong BEFORE the
    // recovering open (see ensure_collection_exists): the path pre-flight is
    // pure filesystem, the existence check runs on a plain open, and the
    // re-check on the mutating open closes the disappear-in-between window.
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
    }
    ensure_collection_exists(&open_service()?, id, "collections add-sources")?;
    let service = open_service_recovering()?;
    ensure_collection_exists(&service, id, "collections add-sources")?;
    // `start_index` falls back to `index_status()` when the job create or
    // the follow-up state read fails; the reported job must then not be
    // passed off as the fresh import.
    let previous_job = service.index_status().job_id;
    let state = service.start_index(id, paths);
    // Upstream quirk: any resumable job short-circuits start_index and the
    // requested sources are silently dropped — a fresh job reports
    // preparing/running with resumable=false, so resumable here always
    // means a PRE-EXISTING job won. Reporting it as success would hide
    // files that were never enqueued, for a different collection and for
    // this one alike (the one-shot CLI process leaves its previous job
    // interrupted, so a plain second add-sources hits exactly this).
    if state.resumable || state.collection_id != id {
        return Err(CliError::failed(format!(
            "knowledge collections add-sources: an unfinished index job for collection {} \
             blocks collection {id} and the requested sources were NOT enqueued (check \
             `pinvou knowledge index status`, resume or cancel it first, then re-run this \
             command)",
            state.collection_id
        )));
    }
    // Success requires a genuinely new job (or one demonstrably in flight);
    // the pre-call latest job re-reported in a terminal phase means no
    // import was created and the requested sources were never enqueued.
    if state.job_id == previous_job && !state.running {
        return Err(CliError::failed(format!(
            "knowledge index start failed: collection {id} has no freshly created index \
             job (latest: {}, running: {}); the requested sources were not enqueued",
            state.job_id.as_deref().unwrap_or("none"),
            state.running
        )));
    }
    index_started("index job", Ok(state), output)
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
                format!(
                    "{}\t{}\t{}\tchunks={}\t{}",
                    document.id,
                    document.name,
                    document.parse_status,
                    document.n_chunks,
                    document.path
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
    service
        .l1()
        .remove_document(doc_id)
        .map_err(|error| feature_error("documents remove", error))?;
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

/// GUI `kb_index_cancel` targets the active/latest job; refuse when the
/// caller named a different one so the CLI never cancels the wrong job.
fn index_cancel(job_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Validate the named id BEFORE the recovering open: boot recovery flips
    // every preparing/running job to interrupted — including one a live
    // desktop-app process is still importing — so a wrong id must fail
    // without ever running it.
    let latest = open_service()?.index_status();
    match &latest.job_id {
        Some(active) if active == job_id => {}
        Some(active) => {
            return Err(CliError::failed(format!(
                "knowledge index cancel({job_id}): job is not the active/latest index \
                 job (latest: {active})"
            )));
        }
        // No job exists at all: claiming a cancel was signalled would be a
        // false success.
        None => {
            return Err(CliError::failed(format!(
                "knowledge_index_job_not_found: no index job exists (nothing to cancel for \
                 {job_id})"
            )));
        }
    }
    // `ImportJobStore::cancel` is synchronous: a running or interrupted
    // (resumable) job is flipped to `cancelled` inside the call, so the
    // post-cancel status never reports running — deciding the message from
    // it printed "nothing was signalled" on every effective cancel. Decide
    // from the pre-cancel state instead: a running or resumable job gets a
    // real signal, a finished job (done/cancelled) takes the same call
    // without anything to signal.
    let was_active = latest.running || latest.resumable;
    // A finished job has nothing to signal, so the recovering open buys
    // nothing here and would still flip a job a live desktop process is
    // importing to interrupted.
    let service = if was_active {
        open_service_recovering()?
    } else {
        open_service()?
    };
    // Re-check on the open that will actually cancel — in BOTH branches:
    // `cancel_index` targets the latest job at call time even when the named
    // job is finished, so a job created between the validation open and this
    // one must fail here instead of being cancelled unvalidated — the whole
    // point of the pre-check above.
    match service.index_status().job_id.as_deref() {
        Some(active) if active == job_id => {}
        Some(active) => {
            return Err(CliError::failed(format!(
                "knowledge index cancel({job_id}): job is no longer the active/latest index job (latest: {active}); re-run to act on {active}"
            )));
        }
        None => {
            return Err(CliError::failed(format!(
                "knowledge_index_job_not_found: no index job exists anymore (nothing to cancel for {job_id})"
            )));
        }
    }
    service
        .cancel_index()
        .map_err(|error| feature_error("index cancel", error))?;
    // Report the job that was actually cancelled, NOT `index_status()`. The
    // latest-job read ranks `cancelled` in its last bucket, so the successful
    // cancel hands the "latest" crown to any older `done_with_errors` /
    // `interrupted` job: the human line said "cancel signalled for job B"
    // while the JSON body carried A's state under A's jobId. `resume`/`retry`
    // already report the named job; this is the same read.
    let state = named_job_state(&service, job_id, "cancel")?;
    let header = if was_active {
        format!("index cancel signalled for job {job_id}")
    } else {
        format!(
            "index cancel: job {job_id} was not active (running: {}); nothing was signalled",
            latest.running
        )
    };
    let human = format!("{header}\n{}", render_index_state(&state));
    let mut value = serde_json::to_value(&state).unwrap_or_default();
    value["phase"] = serde_json::json!(display_phase(&state));
    Ok(success(render(output, human, &value)))
}

/// Validates the named job id against the latest job BEFORE any recovering
/// open (the same rule `index cancel` applies): boot recovery flips every
/// preparing/running job to interrupted — including one a live desktop-app
/// process is still importing — so a mistyped id must fail without ever
/// running it.
fn require_latest_job_id(job_id: &str, operation: &str) -> Result<(), CliError> {
    let latest = open_service()?.index_status();
    match &latest.job_id {
        Some(active) if active == job_id => Ok(()),
        Some(active) => Err(CliError::failed(format!(
            "knowledge index {operation}({job_id}): job is not the active/latest index \
             job (latest: {active})"
        ))),
        // No job exists at all: claiming a {operation} landed would be a
        // false success.
        None => Err(CliError::failed(format!(
            "knowledge_index_job_not_found: no index job exists (nothing to {operation} for \
             {job_id})"
        ))),
    }
}

/// GUI `kb_index_resume` re-arms an interrupted job; the named id is
/// validated against the latest job before the recovering open (see
/// [`require_latest_job_id`]).
fn index_resume(job_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_latest_job_id(job_id, "resume")?;
    index_started(
        "index resumed",
        open_service_recovering()?.resume_index(job_id.to_owned()),
        output,
    )
}

/// GUI `kb_index_retry` re-queues one failed item; same pre-validation as
/// [`index_resume`].
fn index_retry(job_id: &str, item_id: i64, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_latest_job_id(job_id, "retry")?;
    index_started(
        "index retry queued",
        open_service_recovering()?.retry_index_item(job_id.to_owned(), item_id),
        output,
    )
}

/// Shared shape for the start/status-style job operations (`add-sources`,
/// `resume`, `retry`): the feature call returns immediately with the
/// DB-persisted job state that `index status` polls.
fn index_started(
    header: &str,
    result: Result<IndexState, String>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let state = result.map_err(|error| {
        // Upstream `ImportJobStore::{resume, retry_item}` answer an unknown
        // or non-resumable job id with rusqlite's `QueryReturnedNoRows`;
        // name the real cause instead of leaking the raw driver message (the
        // same code `index failed` uses).
        if error.contains("Query returned no rows") {
            CliError::failed(
                "knowledge_index_job_not_found: no resumable index job for the requested id"
                    .to_owned(),
            )
        } else {
            CliError::failed(format!("knowledge index: {error}"))
        }
    })?;
    index_out(header, state, output)
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
        lines.push(format!("current: {path}"));
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
            .map(|file| format!("{}\t{}\t{}", file.item_id, file.name, file.error))
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
/// The whole payload is process-local and JSON says so with the same
/// `"scope": "process-local"` marker `scan cancel` and `model cancel` carry.
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
    let installed = model_directory_complete(&dir);
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

/// CLI-side mirror of `pinvou_knowledge::model_download::
/// model_directory_is_complete` (pinvou-knowledge/src/model_download.rs, the
/// single source of truth): the directory is canonicalized first, then one of
/// the ONNX variants plus the four tokenizer/config files must be present.
///
/// The upstream helper is `pub`, but `pinvou-knowledge` is only a TRANSITIVE
/// dependency here (it enters the graph through `pinvou3-tauri`), and
/// `pinvou3_lib` does not re-export it — a Rust crate cannot name a
/// transitive dependency, so calling it would mean adding a direct
/// `pinvou-knowledge` path dependency to this crate's Cargo.toml. Until that
/// happens this copy must stay behaviourally identical, canonicalization
/// included: without it a symlinked model directory (`current -> models/v3`,
/// a supported override shape) answers differently here than in the app.
/// Keep both halves in sync when the model manifest changes.
fn model_directory_complete(dir: &Path) -> bool {
    let Ok(dir) = std::fs::canonicalize(dir) else {
        return false;
    };
    let onnx = dir.join("model.onnx").is_file()
        || dir.join("onnx").join("model_int8.onnx").is_file()
        || dir.join("onnx").join("model.onnx").is_file();
    onnx && [
        "tokenizer.json",
        "config.json",
        "special_tokens_map.json",
        "tokenizer_config.json",
    ]
    .iter()
    .all(|name| dir.join(name).is_file())
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

/// Real `kb_model_cancel` (sets the upstream cancel flag). Its effect is
/// process-local by nature: downloads run inside the desktop app process,
/// never in a one-shot CLI invocation.
fn model_cancel(output: OutputMode) -> Result<CliOutcome, CliError> {
    kb_model_cancel();
    Ok(success(render(
        output,
        "model download cancel signalled (downloads run in the desktop app process)".to_owned(),
        &serde_json::json!({ "cancelled": true, "scope": "process-local" }),
    )))
}

// ───────────────────────── remote / host ─────────────────────────

/// Loads the persisted remote connections (offline file read; a missing
/// file means no connections, like the GUI's fresh state).
fn open_remote_service() -> Result<RemoteKnowledgeService, CliError> {
    sandbox_home()?;
    RemoteKnowledgeService::load(RemoteKnowledgeService::default_path())
        .map_err(|error| CliError::failed(format!("knowledge remote connections: {error}")))
}

/// GUI `remote_kb_connections`: probe every configured connection (network).
/// With zero configured connections this answers offline without booting the
/// windowless host.
fn remote_connections(output: OutputMode) -> Result<CliOutcome, CliError> {
    let service = open_remote_service()?;
    if service.configured_connections().is_empty() {
        return empty_remote_connections(output);
    }
    let statuses =
        pinvou3_lib::headless_bridge::run_windowless_host(move |_pool, _store| async move {
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
                    status.connection.name,
                    status.connection.endpoint,
                    status.online,
                    status.ready
                );
                match &status.error {
                    Some(error) => format!("{base}\t{error}"),
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
/// windowless product host, like the other remote surfaces. The human output
/// shows the confirmable identity fields; JSON carries the full probe
/// (public CA identity material — the GUI requires out-of-band confirmation
/// of the identity code before any join).
fn remote_probe(url: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    let source = url.to_owned();
    let probe =
        pinvou3_lib::headless_bridge::run_windowless_host(move |_pool, _store| async move {
            pinvou3_lib::features::remote_knowledge::probe_private_identity(&source)
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))
        })
        .map_err(|error| host_error("remote probe", error))?;
    let human = format!(
        "endpoint: {}\nserver: {}\nserver_id: {}\nidentity_code: {}\nnetwork: {:?}\nready: {}\n\
         note: confirm the identity code out-of-band before joining",
        probe.endpoint,
        probe.server_name,
        probe.server_id,
        probe.identity_code,
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
    let pages =
        pinvou3_lib::headless_bridge::run_windowless_host(move |_pool, _store| async move {
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
                        server_id, collection.id, collection.name, collection.doc_count
                    ));
                }
                if collections.is_empty() {
                    lines.push(format!("{server_id}\tno remote collections"));
                }
                results.push(serde_json::json!({
                    "server_id": server_id,
                    "name": name,
                    "collections": serde_json::to_value(&collections).unwrap_or_default(),
                }));
            }
            Err(error) => {
                lines.push(format!("{server_id}\terror: {error}"));
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
    let outcomes =
        pinvou3_lib::headless_bridge::run_windowless_host(move |_pool, _store| async move {
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
                        hit.document_name,
                        hit.score,
                        hit.text.chars().take(120).collect::<String>()
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
                lines.push(format!("{server_id}\terror: {error}"));
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
/// probe + optional local health check), so it runs through the windowless
/// product host (needs a display on headless Linux).
fn host_status(output: OutputMode) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    let status =
        pinvou3_lib::headless_bridge::run_windowless_host(move |_pool, _store| async move {
            Ok::<_, anyhow::Error>(pinvou3_lib::features::shared_knowledge_host::status().await)
        })
        .map_err(|error| host_error("host status", error))?;
    let human = format!(
        "supported: {}\ninstalled: {}\nrunning: {}\nendpoint: {}\nservice_version: {}\n\
         app_version: {}\nupgrade_available: {}\nclient_outdated: {}",
        status.supported,
        status.installed,
        status.running,
        status.endpoint,
        status.service_version.as_deref().unwrap_or("none"),
        status.app_version,
        status.upgrade_available,
        status.client_outdated
    );
    let value = serde_json::to_value(&status).unwrap_or_default();
    Ok(success(render(output, human, &value)))
}

/// The remote/host surfaces are async network client calls; run them through
/// the windowless product host — the same bootstrap as `agent run` (needs a
/// display on headless Linux; documented in the module docs and the
/// `#[ignore]`d tests). The closure parameter types are the host's
/// `EnginePool`/`SessionStore` (supplied by inference, as in memory.rs —
/// `engine_pool` is `pub(crate)` and not nameable from the CLI).
fn host_error(operation: &str, error: anyhow::Error) -> CliError {
    CliError::failed(format!("knowledge {operation}: {error:#}"))
}

fn open_store() -> Result<SessionStore, CliError> {
    SessionStore::boot().map_err(|error| {
        CliError::failed(format!("knowledge session store unavailable: {error:#}"))
    })
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
