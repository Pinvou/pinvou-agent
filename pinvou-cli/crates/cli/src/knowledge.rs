//! `knowledge` family: knowledge-base surface mirroring the GUI commands in
//! `pinvou3-app/src-tauri/src/app/commands/knowledge.rs` (kb_* and the
//! session-mount commands) plus `remote_knowledge.rs` and
//! `shared_knowledge_host.rs`.
//!
//! Headless construction (resolved): `KnowledgeService::new(db_path: &Path)
//! -> rusqlite::Result<Self>` (`features/knowledge/mod.rs`) is a standalone
//! constructor — it only opens `~/.pinvou3/knowledge/index.db`, wraps that
//! connection in `L1Store` + `ImportJobStore`, and needs no Tauri state. The
//! GUI manages one instance as Tauri state. The windowless product host
//! (`run_windowless_host` behind `pinvou_product_backend::
//! run_with_product_backend`) would NOT help: its setup manages only
//! `SessionStore`, `build_pool` merely reads
//! `app.try_state::<KnowledgeService>()` (which is `None` headless), and the
//! work closure receives no app handle — so there is no host route to the
//! service either. The intended shape is per-invocation direct construction:
//! `KnowledgeService::new(&sandbox_home()?.join("knowledge").join("index.db"))`.
//!
//! However `features/mod.rs` in the app crate declares
//! `pub(crate) mod knowledge;` (likewise `remote_knowledge` and
//! `shared_knowledge_host`), so none of those types are reachable from the
//! CLI crate today. Every subcommand that needs them reports a stable
//! `knowledge_backend_unavailable` (or family-specific) error naming that
//! boundary instead of pretending success. Upstream unlock: expose the
//! modules as `pub` in `pinvou3-app/src-tauri/src/features/mod.rs`, then each
//! mapping below becomes a direct call:
//! - scan start/status/cancel → `KnowledgeService::{start_scan, status,
//!   cancel_scan}` behind `kb_start_scan`/`kb_scan_status`/`kb_cancel_scan`;
//!   `--root` omitted defaults to the user home like the GUI.
//! - stats / type-counts → `Store::{stats, type_counts}` behind `kb_stats` /
//!   `kb_type_counts` (L0 metadata index in the same `index.db`).
//! - collections list/create/update/delete/add-sources →
//!   `L1Store::{list_collections, create_collection, update_collection,
//!   delete_collection}` and `KnowledgeService::start_index` behind
//!   `kb_collection_*`; delete also cancels a running import for the
//!   collection and clears every session mount (GUI `kb_collection_delete`).
//! - documents / documents remove → `L1Store::{list_documents,
//!   remove_document}` behind `kb_documents`/`kb_remove_document`.
//! - index status/cancel/resume/retry/failed →
//!   `KnowledgeService::{index_status, cancel_index, resume_index,
//!   retry_index_item, failed_index_files}` behind `kb_index_*`; `--offset`
//!   defaults to 0 and `--limit` to 50 like the GUI page size.
//! - search → `kb_search`: explicit `--limit/--ext/--after/--before` map onto
//!   `SearchQueryDto` fields; the GUI's natural-language time/type parsing is
//!   internal to `kb_search` and yields to explicit filters, so the CLI only
//!   needs the explicit flags.
//! - model status/download/cancel →
//!   `features::knowledge::model_download::{kb_model_status,
//!   kb_model_download, kb_model_cancel}`. Download is network-bound and
//!   long-running (~570MB bge-m3 streams, then loads ONNX in-process) — an
//!   opt-in (`#[ignore]`) test path only.
//! - remote connections/probe/collections/search →
//!   `features::remote_knowledge` (client subset; LAN discovery/QR are
//!   GUI-bound and stay omitted). Probe is a TLS-pinned `/api/v1/identity`
//!   handshake inside the `pinvou-knowledge` client crate — not mirrorable
//!   over plain HTTP.
//! - host status → `features::shared_knowledge_host` status snapshot;
//!   install/upgrade/backup stay GUI/host-bound and are omitted.
//!
//! Reachable today (public `pinvou3_lib::features::sessions`): the session
//! mount trio, the exact `SessionStore` calls behind the GUI commands
//! (`mounted_collections_snapshot`, `add_mounted_collection`,
//! `remove_mounted_collection`). The GUI's mount gate
//! (`validate_collection_mountable` in app/commands/knowledge.rs) is mirrored
//! with its errors verbatim; in a CLI process the embedding model is never
//! loaded (its loader lives behind the same `pub(crate)` boundary), so
//! enabling a mount reports the GUI's not-ready error, while listing and
//! unmounting stay gate-free exactly like the GUI. Like the GUI store, mount
//! mutations live in process memory until another mode-state save persists
//! them, so each CLI invocation starts from the persisted state. Tests use a
//! temp `PINVOU3_HOME`; the service-backed paths are covered by `#[ignore]`d
//! documentation only until the upstream unlock (they cannot be constructed
//! here without the pub surface).

use std::path::PathBuf;

use crate::support::{render, require_yes, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::sessions::{MountedCollectionsSnapshot, SessionStore};

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
    if value.is_empty() {
        return Err(CliError::usage("knowledge requires a session id"));
    }
    Ok(value.to_owned())
}

fn parse_id(value: &str, label: &str) -> Result<i64, CliError> {
    value.parse::<i64>().map_err(|_| {
        CliError::usage(format!(
            "knowledge {label} id must be an integer (got {value})"
        ))
    })
}

/// Mirrors the pair-based `named_options` helper in lib.rs (extended with
/// valueless boolean flags, as in `sessions.rs`): every token must be a known
/// value flag (followed by a non-empty value), a known boolean flag, or
/// nothing.
fn parse_flags<'a>(
    values: &'a [String],
    value_flags: &[&str],
    boolean_flags: &[&str],
) -> Result<(Vec<(&'a str, &'a str)>, Vec<&'a str>), CliError> {
    let mut options = Vec::new();
    let mut flags = Vec::new();
    let mut index = 0;
    while index < values.len() {
        let token = values[index].as_str();
        if boolean_flags.contains(&token) {
            if flags.contains(&token) {
                return Err(CliError::usage(format!(
                    "duplicate knowledge option {token}"
                )));
            }
            flags.push(token);
            index += 1;
            continue;
        }
        if !value_flags.contains(&token) {
            return Err(CliError::usage(format!(
                "unsupported knowledge option: {token}"
            )));
        }
        if options.iter().any(|(name, _)| *name == token) {
            return Err(CliError::usage(format!(
                "duplicate knowledge option {token}"
            )));
        }
        let value = values
            .get(index + 1)
            .ok_or_else(|| CliError::usage(format!("knowledge option {token} requires a value")))?;
        if value.is_empty() || value.starts_with("--") {
            return Err(CliError::usage(format!(
                "knowledge option {token} requires a value"
            )));
        }
        options.push((token, value.as_str()));
        index += 2;
    }
    Ok((options, flags))
}

fn option<'a>(options: &'a [(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    options
        .iter()
        .find_map(|(candidate, value)| (*candidate == name).then_some(*value))
}

fn parse_positive(options: &[(&str, &str)], name: &str) -> Result<Option<usize>, CliError> {
    match option(options, name) {
        None => Ok(None),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|count| *count > 0)
            .map(Some)
            .ok_or_else(|| CliError::usage(format!("knowledge {name} must be a positive integer"))),
    }
}

fn parse_non_negative(value: &str, name: &str) -> Result<usize, CliError> {
    value
        .parse::<usize>()
        .map_err(|_| CliError::usage(format!("knowledge {name} must be a non-negative integer")))
}

/// Stable error for the subcommands that need `pinvou3_lib::features::
/// knowledge`, which the app crate keeps `pub(crate)`. Exit code 1 (host /
/// runtime failure): the command is valid, the backend is out of reach.
fn backend_unavailable(operation: &str) -> CliError {
    CliError::failed(format!(
        "knowledge_backend_unavailable: {operation} needs \
         pinvou3_lib::features::knowledge::KnowledgeService, but the app crate keeps \
         features::knowledge pub(crate) (pinvou3-app/src-tauri/src/features/mod.rs); \
         expose the module upstream, then this subcommand constructs the service \
         directly at ~/.pinvou3/knowledge/index.db"
    ))
}

fn remote_unavailable(operation: &str) -> CliError {
    CliError::failed(format!(
        "remote_knowledge_backend_unavailable: {operation} needs \
         pinvou3_lib::features::remote_knowledge and the pinvou-knowledge client crate, \
         both unreachable from the CLI (features::remote_knowledge is pub(crate))"
    ))
}

fn host_unavailable(operation: &str) -> CliError {
    CliError::failed(format!(
        "shared_knowledge_host_backend_unavailable: {operation} needs \
         pinvou3_lib::features::shared_knowledge_host, which the app crate keeps \
         pub(crate); install/upgrade/backup stay GUI-bound and are omitted"
    ))
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
        KnowledgeCommand::CollectionsDelete { id: _, yes } => {
            require_yes(yes)?;
            Err(backend_unavailable("collections delete"))
        }
        KnowledgeCommand::DocumentsRemove { doc_id: _, yes } => {
            require_yes(yes)?;
            Err(backend_unavailable("documents remove"))
        }
        KnowledgeCommand::ScanStart { .. } => Err(backend_unavailable("scan start")),
        KnowledgeCommand::ScanStatus => Err(backend_unavailable("scan status")),
        KnowledgeCommand::ScanCancel => Err(backend_unavailable("scan cancel")),
        KnowledgeCommand::Stats => Err(backend_unavailable("stats")),
        KnowledgeCommand::TypeCounts => Err(backend_unavailable("type-counts")),
        KnowledgeCommand::CollectionsList => Err(backend_unavailable("collections list")),
        KnowledgeCommand::CollectionsCreate { .. } => {
            Err(backend_unavailable("collections create"))
        }
        KnowledgeCommand::CollectionsUpdate { .. } => {
            Err(backend_unavailable("collections update"))
        }
        KnowledgeCommand::CollectionsAddSources { .. } => {
            Err(backend_unavailable("collections add-sources"))
        }
        KnowledgeCommand::Documents { .. } => Err(backend_unavailable("documents")),
        KnowledgeCommand::IndexStatus { .. } => Err(backend_unavailable("index status")),
        KnowledgeCommand::IndexCancel { .. } => Err(backend_unavailable("index cancel")),
        KnowledgeCommand::IndexResume { .. } => Err(backend_unavailable("index resume")),
        KnowledgeCommand::IndexRetry { .. } => Err(backend_unavailable("index retry")),
        KnowledgeCommand::IndexFailed { .. } => Err(backend_unavailable("index failed")),
        KnowledgeCommand::Search { .. } => Err(backend_unavailable("search")),
        KnowledgeCommand::ModelStatus => Err(backend_unavailable("model status")),
        KnowledgeCommand::ModelDownload => Err(backend_unavailable(
            "model download (network + desktop model host; downloads the ~570MB bge-m3 model)",
        )),
        KnowledgeCommand::ModelCancel => Err(backend_unavailable("model cancel")),
        KnowledgeCommand::RemoteConnections => Err(remote_unavailable("remote connections")),
        KnowledgeCommand::RemoteProbe { .. } => Err(remote_unavailable("remote probe")),
        KnowledgeCommand::RemoteCollections => Err(remote_unavailable("remote collections")),
        KnowledgeCommand::RemoteSearch { .. } => Err(remote_unavailable("remote search")),
        KnowledgeCommand::HostStatus => Err(host_unavailable("host status")),
    }
}

fn open_store() -> Result<SessionStore, CliError> {
    SessionStore::boot().map_err(|error| {
        CliError::failed(format!("knowledge session store unavailable: {error:#}"))
    })
}

/// The GUI mount commands operate on an open session, which by definition
/// exists; the CLI mirror requires the same so sidecar state is never written
/// for a stale id (same convention as the `sessions` family).
fn require_session(store: &SessionStore, session_id: &str, action: &str) -> Result<(), CliError> {
    store.load(session_id).map(|_| ()).map_err(|error| {
        CliError::failed(format!(
            "knowledge {action}({session_id}): session not found: {error:#}"
        ))
    })
}

/// Verbatim GUI mount gate (`validate_collection_mountable` in
/// app/commands/knowledge.rs): id validity, then embedding-model readiness,
/// then collection existence. A CLI process never has the embedding model
/// loaded (the loader lives behind the `pub(crate)` boundary described in the
/// module docs), so the gate always stops at the not-ready error with the
/// GUI's exact message; the existence half becomes reachable together with
/// the upstream module exposure.
fn ensure_collection_mountable(collection_id: i64) -> Result<(), CliError> {
    if collection_id <= 0 {
        return Err(CliError::failed("知识集 id 无效"));
    }
    Err(CliError::failed("embedding 模型未就绪,知识库暂不可用"))
}

/// GUI `session_mounted_collections_snapshot`: the revisioned source of truth
/// for one session's mounts. Unknown sessions are rejected first (CLI
/// convention); an existing session without mounts is an empty snapshot, not
/// an error.
fn mounts(session_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    let store = open_store()?;
    require_session(&store, session_id, "mounts")?;
    let snapshot = store.mounted_collections_snapshot(session_id);
    let value = snapshot_json(session_id, &snapshot);
    let human = if snapshot.collections.is_empty() {
        format!("no collections mounted for {session_id}")
    } else {
        snapshot
            .collections
            .iter()
            .map(|collection| {
                format!(
                    "{}\t{}",
                    collection.collection_id,
                    if collection.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(success(render(output, human, &value)))
}

/// GUI `session_mount_collection` / `session_add_mounted_collection` share
/// this gate; the atomic `store.add_mounted_collection` behind it is
/// unreachable until the knowledge service is (see module docs), so the gate
/// outcome is the CLI result for now.
fn mount(
    session_id: &str,
    collection_id: i64,
    _output: OutputMode,
) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    let store = open_store()?;
    require_session(&store, session_id, "mount")?;
    ensure_collection_mountable(collection_id)?;
    // The GUI gate passed; the atomic `store.add_mounted_collection` write it
    // guards stays behind the pub(crate) boundary, so report that instead of
    // pretending the mount succeeded.
    Err(backend_unavailable(
        "mount (store.add_mounted_collection needs the knowledge service)",
    ))
}

/// GUI `session_remove_mounted_collection`: no model or existence gate, so
/// stale mounts can always be cleaned up; removal of an absent id is a
/// no-op that still returns the fresh snapshot.
fn unmount(
    session_id: &str,
    collection_id: i64,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    let store = open_store()?;
    require_session(&store, session_id, "unmount")?;
    let snapshot = store.remove_mounted_collection(session_id, collection_id);
    let value = snapshot_json(session_id, &snapshot);
    let human = format!("unmounted {collection_id} from {session_id}");
    Ok(success(render(output, human, &value)))
}

fn snapshot_json(session_id: &str, snapshot: &MountedCollectionsSnapshot) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "revision": snapshot.revision,
        "collections": snapshot.collections,
    })
}
