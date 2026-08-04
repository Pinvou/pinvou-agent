//! Remote Web host-file browsing and Session-owned artifact access.
//!
//! This module owns path authorization and bounded file reads. Relay lifecycle,
//! RPC dispatch, streaming, and upload state remain in [`super::manager`].

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::Serialize;

use super::platform;
use super::MAX_TRANSFER_CHUNK_BYTES;
use crate::features::sessions::SessionStore;
use crate::platform::paths;

const MAX_HOST_ENTRIES: usize = 5_000;

#[derive(Debug, Clone, Serialize)]
pub struct HostFileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostFileRoot {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostFileListing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<HostFileEntry>,
    pub roots: Vec<HostFileRoot>,
}

pub fn list_host_files(requested: Option<String>) -> Result<HostFileListing, String> {
    let requested = requested
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(paths::user_home_dir);
    let directory =
        crate::features::files::file_ingest::validate_browsable_path(&requested.to_string_lossy())?;
    if !directory.is_dir() {
        return Err(format!(
            "host path is not a directory: {}",
            directory.display()
        ));
    }
    let mut entries = Vec::new();
    let read_dir = std::fs::read_dir(&directory)
        .map_err(|error| format!("list host path {}: {error}", directory.display()))?;
    for entry in read_dir.take(MAX_HOST_ENTRIES) {
        let Ok(entry) = entry else { continue };
        let entry_path = entry.path();
        let Ok(authorized_path) = crate::features::files::file_ingest::validate_browsable_path(
            &entry_path.to_string_lossy(),
        ) else {
            continue;
        };
        let Ok(metadata) = std::fs::metadata(&authorized_path) else {
            continue;
        };
        let is_dir = metadata.is_dir();
        entries.push(HostFileEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: platform::display_path(&authorized_path),
            is_dir,
            size: (!is_dir).then_some(metadata.len()),
        });
    }
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(HostFileListing {
        path: platform::display_path(&directory),
        parent: directory.parent().and_then(|parent| {
            crate::features::files::file_ingest::validate_browsable_path(&parent.to_string_lossy())
                .ok()
                .map(|path| platform::display_path(&path))
        }),
        entries,
        roots: host_roots(),
    })
}

fn host_roots() -> Vec<HostFileRoot> {
    let home = paths::user_home_dir();
    vec![HostFileRoot {
        name: "Home".to_string(),
        path: platform::display_path(&home),
    }]
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactChunk {
    pub name: String,
    pub size: u64,
    pub offset: u64,
    pub data_base64: String,
    pub eof: bool,
}

pub fn read_artifact_chunk(
    store: &SessionStore,
    path: &str,
    session_id: &str,
    offset: u64,
    limit: Option<usize>,
) -> Result<ArtifactChunk, String> {
    let session_id = require_explicit_session_id(session_id)?;
    let limit = limit.unwrap_or(MAX_TRANSFER_CHUNK_BYTES);
    if limit == 0 || limit > MAX_TRANSFER_CHUNK_BYTES {
        return Err(format!(
            "artifact chunk limit must be between 1 and {MAX_TRANSFER_CHUNK_BYTES}"
        ));
    }
    let resolved = resolve_session_artifact_path(store, session_id, path)?;
    read_resolved_file_chunk_with_limit(&resolved, offset, limit)
}

pub(crate) fn read_resolved_file_chunk(
    resolved: &Path,
    offset: u64,
    limit: Option<usize>,
) -> Result<ArtifactChunk, String> {
    let limit = limit.unwrap_or(MAX_TRANSFER_CHUNK_BYTES);
    if limit == 0 || limit > MAX_TRANSFER_CHUNK_BYTES {
        return Err(format!(
            "artifact chunk limit must be between 1 and {MAX_TRANSFER_CHUNK_BYTES}"
        ));
    }
    read_resolved_file_chunk_with_limit(resolved, offset, limit)
}

fn read_resolved_file_chunk_with_limit(
    resolved: &Path,
    offset: u64,
    limit: usize,
) -> Result<ArtifactChunk, String> {
    let mut file = File::open(resolved)
        .map_err(|error| format!("open artifact {}: {error}", resolved.display()))?;
    let size = file
        .metadata()
        .map_err(|error| format!("stat artifact {}: {error}", resolved.display()))?
        .len();
    if offset > size {
        return Err(format!("artifact offset {offset} exceeds file size {size}"));
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| format!("seek artifact {}: {error}", resolved.display()))?;
    let mut data = vec![0_u8; limit];
    let read = file
        .read(&mut data)
        .map_err(|error| format!("read artifact {}: {error}", resolved.display()))?;
    data.truncate(read);
    Ok(ArtifactChunk {
        name: resolved
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("artifact")
            .to_string(),
        size,
        offset,
        data_base64: base64::engine::general_purpose::STANDARD.encode(data),
        eof: offset.saturating_add(read as u64) >= size,
    })
}

fn require_explicit_session_id(session_id: &str) -> Result<&str, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("sessionId is required to download an artifact".to_string());
    }
    Ok(session_id)
}

/// Resolve only files owned by the selected Session. For ordinary Sessions,
/// their isolated workspace and private artifacts directory are both owned by
/// that Session. Scheduled Sessions share a wider workspace, so only explicitly
/// recorded deliverables and their private artifacts directory are accepted.
pub(crate) fn resolve_session_artifact_path(
    store: &SessionStore,
    session_id: &str,
    requested: &str,
) -> Result<PathBuf, String> {
    crate::features::sessions::validate_session_id(session_id)
        .map_err(|error| format!("invalid Session id: {error:#}"))?;
    let saved = store
        .load(session_id)
        .map_err(|error| format!("load Session {session_id}: {error:#}"))?;
    let requested = requested.trim();
    if requested.is_empty() {
        return Err("missing artifact path".to_string());
    }
    let workspace = store
        .ledger_root(session_id)
        .map_err(|error| format!("resolve session workspace: {error:#}"))?;
    let scheduled = store.scheduled_profile(session_id).is_some();
    let artifacts = paths::session_artifacts_dir(session_id);
    let workspace_root = workspace
        .canonicalize()
        .map_err(|error| format!("resolve session workspace: {error}"))?;
    let artifacts_root = artifacts.canonicalize().ok();
    let scheduled_artifacts = if scheduled {
        saved
            .artifacts
            .into_iter()
            .filter_map(|artifact| artifact.storage_path.canonicalize().ok())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let mut candidates = Vec::new();
    let requested_path = Path::new(requested);
    if requested_path.is_absolute() {
        candidates.push(requested_path.to_path_buf());
    } else {
        candidates.push(workspace.join(requested));
        candidates.push(artifacts.join(requested));
        if let Some(tail) = path_after_segment(requested, "workspace") {
            candidates.push(workspace.join(tail));
        }
        if let Some(tail) = path_after_segment(requested, "artifacts") {
            candidates.push(artifacts.join(tail));
        }
    }
    for candidate in candidates {
        let Ok(canonical) = candidate.canonicalize() else {
            continue;
        };
        if !canonical.is_file() {
            continue;
        }
        let in_private_artifacts = artifacts_root
            .as_ref()
            .is_some_and(|root| canonical.starts_with(root));
        let authorized = if scheduled {
            in_private_artifacts || scheduled_artifacts.iter().any(|path| path == &canonical)
        } else {
            canonical.starts_with(&workspace_root) || in_private_artifacts
        };
        if authorized {
            return Ok(canonical);
        }
    }
    Err(format!(
        "artifact path not found in Session {session_id}: {requested}"
    ))
}

fn path_after_segment(path: &str, segment: &str) -> Option<PathBuf> {
    let normalized = path.replace('\\', "/");
    let mut parts = normalized.split('/').filter(|part| !part.is_empty());
    while let Some(part) = parts.next() {
        if part == segment {
            let tail = parts.collect::<Vec<_>>().join("/");
            return (!tail.is_empty()).then(|| PathBuf::from(tail));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_file_listing_accepts_the_default_home_directory() {
        let listing = list_host_files(None).expect("default home directory should be browsable");

        assert!(!listing.path.is_empty());
        assert!(
            listing.roots.iter().any(|root| root.path == listing.path),
            "home root should match the initial listing path"
        );
    }

    #[test]
    fn artifact_segment_helper_never_retains_parent_prefix() {
        assert_eq!(
            path_after_segment("ignored/workspace/reports/a.pdf", "workspace"),
            Some(PathBuf::from("reports/a.pdf"))
        );
        assert_eq!(path_after_segment("workspace", "workspace"), None);
    }

    #[test]
    fn artifact_chunk_limit_is_exactly_256_kib() {
        assert_eq!(MAX_TRANSFER_CHUNK_BYTES, 262_144);
    }

    #[test]
    fn artifact_chunk_requires_an_explicit_session_id() {
        assert_eq!(
            require_explicit_session_id("  ").unwrap_err(),
            "sessionId is required to download an artifact"
        );
        assert_eq!(
            require_explicit_session_id(" session-1 ").unwrap(),
            "session-1"
        );
    }

    #[test]
    fn resolved_file_chunks_preserve_offset_limit_and_eof() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-remote-file-access-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("artifact.bin");
        std::fs::write(&path, b"abcdef").unwrap();

        let first = read_resolved_file_chunk(&path, 2, Some(3)).unwrap();
        assert_eq!(first.name, "artifact.bin");
        assert_eq!(first.size, 6);
        assert_eq!(first.offset, 2);
        assert_eq!(first.data_base64, "Y2Rl");
        assert!(!first.eof);

        let last = read_resolved_file_chunk(&path, 5, Some(3)).unwrap();
        assert_eq!(last.data_base64, "Zg==");
        assert!(last.eof);

        std::fs::remove_dir_all(directory).unwrap();
    }
}
