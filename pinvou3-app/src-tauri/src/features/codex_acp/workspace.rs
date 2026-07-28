use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::{DirEntry, WalkDir};

use super::events::AcpEventEnvelope;

const LIST_LIMIT: usize = 500;
const SEARCH_LIMIT: usize = 300;
const WALK_LIMIT: usize = 20_000;
const PREVIEW_LIMIT: usize = 512 * 1024;
const IMAGE_PREVIEW_LIMIT: u64 = 10 * 1024 * 1024;
const DIFF_LIMIT: usize = 1024 * 1024;

const DELIVERABLE_EXTENSIONS: &[&str] = &[
    "pptx", "ppt", "docx", "doc", "pdf", "html", "htm", "xlsx", "xls", "md", "csv", "png", "jpg",
    "jpeg", "svg", "gif", "webp", "zip",
];

const IGNORED_DIRECTORIES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".cache",
    "__pycache__",
    ".venv",
    "venv",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEntry {
    pub name: String,
    pub relative_path: String,
    pub kind: String,
    pub size: u64,
    pub modified: i64,
    pub has_children: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListing {
    pub relative_path: String,
    pub entries: Vec<WorkspaceEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePreview {
    pub name: String,
    pub relative_path: String,
    pub kind: String,
    pub size: u64,
    pub modified: i64,
    pub text: Option<String>,
    pub data_url: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChange {
    pub relative_path: String,
    pub status: String,
    pub staged: bool,
    /// `session` / `preexisting` / `preexisting_modified` / `unknown`
    pub origin: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChanges {
    pub git: bool,
    pub branch: Option<String>,
    pub baseline_available: bool,
    pub changes: Vec<WorkspaceChange>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDiff {
    pub relative_path: String,
    pub text: String,
    pub truncated: bool,
}

/// A user-facing deliverable discovered in a Codex workspace.
///
/// Code mode has its own ACP event stream, so it does not pass through the
/// regular chat bridge's `write_file` artifact tracker.  This compact record is
/// returned to the Code UI and persisted into the shared Session artifact
/// index so previews and Local Knowledge can consume the same source of truth.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceArtifact {
    pub path: String,
    pub basename: String,
    pub relative_path: String,
    pub size: u64,
    pub modified: i64,
}

#[derive(Debug, Clone)]
pub(super) struct WorkspacePromptReference {
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileFingerprint {
    size: u64,
    modified: i64,
    sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceBaseline {
    workspace_path: String,
    git: bool,
    #[serde(default)]
    dirty_paths: BTreeSet<String>,
    entries: BTreeMap<String, FileFingerprint>,
}

pub fn list_workspace(root: &Path, relative_path: Option<&str>) -> Result<WorkspaceListing> {
    let root = canonical_workspace(root)?;
    let relative = normalize_relative_path(relative_path.unwrap_or_default())?;
    let directory = resolve_existing_path(&root, &relative, true)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(&directory)
        .with_context(|| format!("读取工作目录失败: {}", directory.display()))?
        .filter_map(|entry| entry.ok())
    {
        if let Some(entry) = workspace_entry(&root, entry.path())? {
            entries.push(entry);
        }
    }
    entries.sort_by(|left, right| {
        let left_dir = left.kind == "directory";
        let right_dir = right.kind == "directory";
        right_dir
            .cmp(&left_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    let truncated = entries.len() > LIST_LIMIT;
    entries.truncate(LIST_LIMIT);
    Ok(WorkspaceListing {
        relative_path: relative,
        entries,
        truncated,
    })
}

pub fn search_workspace(root: &Path, query: &str) -> Result<Vec<WorkspaceEntry>> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let root = canonical_workspace(root)?;
    let mut results = Vec::new();
    for entry in WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_walk)
        .filter_map(|entry| entry.ok())
        .take(WALK_LIMIT)
    {
        if entry.depth() == 0 || !entry.file_type().is_file() {
            continue;
        }
        let relative = relative_text(&root, entry.path())?;
        if relative.to_lowercase().contains(&query) {
            if let Some(item) = workspace_entry(&root, entry.path().to_path_buf())? {
                results.push(item);
            }
            if results.len() >= SEARCH_LIMIT {
                break;
            }
        }
    }
    results.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(results)
}

pub fn preview_workspace_file(root: &Path, relative_path: &str) -> Result<WorkspacePreview> {
    let relative = normalize_relative_path(relative_path)?;
    let path = resolve_existing_path(root, &relative, false)?;
    let metadata = path
        .metadata()
        .with_context(|| format!("读取文件信息失败: {}", path.display()))?;
    if !metadata.is_file() {
        bail!("不是文件: {relative}");
    }
    let kind = file_kind(&path);
    let modified = modified_seconds(&metadata);
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(&relative)
        .to_string();

    if kind == "image" {
        if metadata.len() > IMAGE_PREVIEW_LIMIT {
            return Ok(WorkspacePreview {
                name,
                relative_path: relative,
                kind,
                size: metadata.len(),
                modified,
                text: None,
                data_url: None,
                truncated: true,
            });
        }
        let bytes = fs::read(&path).with_context(|| format!("读取图片失败: {}", path.display()))?;
        let mime = image_mime_type(&path);
        return Ok(WorkspacePreview {
            name,
            relative_path: relative,
            kind,
            size: metadata.len(),
            modified,
            text: None,
            data_url: Some(format!(
                "data:{mime};base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            )),
            truncated: false,
        });
    }

    if kind == "text" {
        let mut file =
            fs::File::open(&path).with_context(|| format!("打开文件失败: {}", path.display()))?;
        let mut bytes = Vec::with_capacity(PREVIEW_LIMIT.min(metadata.len() as usize));
        std::io::Read::by_ref(&mut file)
            .take(PREVIEW_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("读取文件失败: {}", path.display()))?;
        let truncated = bytes.len() > PREVIEW_LIMIT;
        bytes.truncate(PREVIEW_LIMIT);
        let text = String::from_utf8_lossy(&bytes).into_owned();
        return Ok(WorkspacePreview {
            name,
            relative_path: relative,
            kind,
            size: metadata.len(),
            modified,
            text: Some(text),
            data_url: None,
            truncated,
        });
    }

    Ok(WorkspacePreview {
        name,
        relative_path: relative,
        kind,
        size: metadata.len(),
        modified,
        text: None,
        data_url: None,
        truncated: false,
    })
}

/// Discover durable, user-facing outputs produced by a Codex session.
///
/// Temporary workspaces are fully session-owned, so deliverable-shaped files
/// can be indexed directly.  Project workspaces may contain unrelated user
/// files; there we only retain previously indexed outputs, structured ACP diff
/// paths, and files that the workspace baseline attributes to this session.
/// Every candidate is canonicalized back under the workspace root before it is
/// returned.
pub fn discover_artifacts(
    session_id: &str,
    root: &Path,
    temporary_workspace: bool,
    events: &[AcpEventEnvelope],
    retained_paths: &[PathBuf],
) -> Result<Vec<WorkspaceArtifact>> {
    let root = canonical_workspace(root)?;
    let mut candidates = BTreeSet::new();

    for path in retained_paths {
        if let Some(path) = resolve_artifact_candidate(&root, path) {
            candidates.insert(path);
        }
    }

    for raw in timeline_diff_paths(events) {
        if let Some(path) = resolve_artifact_candidate(&root, Path::new(&raw)) {
            candidates.insert(path);
        }
    }

    if temporary_workspace {
        for entry in WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(should_walk)
            .filter_map(|entry| entry.ok())
            .take(WALK_LIMIT)
        {
            if entry.depth() == 0 || !entry.file_type().is_file() {
                continue;
            }
            let path = entry.into_path();
            let Ok(relative) = path.strip_prefix(&root) else {
                continue;
            };
            if !is_deliverable_path(relative) {
                continue;
            }
            if let Some(path) = resolve_artifact_candidate(&root, &path) {
                candidates.insert(path);
            }
        }
    } else {
        // Baseline comparison is an enrichment path. If a legacy session has
        // no usable baseline (or git status temporarily fails), keep the
        // retained/timeline candidates instead of making its artifact panel
        // unavailable altogether.
        if let Ok(changes) = workspace_changes(session_id, &root) {
            for change in changes.changes {
                if change.origin != "session" && change.origin != "preexisting_modified" {
                    continue;
                }
                if let Some(path) =
                    resolve_artifact_candidate(&root, Path::new(&change.relative_path))
                {
                    candidates.insert(path);
                }
            }
        }
    }

    let mut artifacts = candidates
        .into_iter()
        .filter_map(|path| {
            let metadata = fs::metadata(&path).ok()?;
            let basename = path.file_name()?.to_str()?.to_string();
            let relative_path = relative_text(&root, &path).ok()?;
            Some(WorkspaceArtifact {
                path: path.to_string_lossy().into_owned(),
                basename,
                relative_path,
                size: metadata.len(),
                modified: modified_seconds(&metadata),
            })
        })
        .collect::<Vec<_>>();
    artifacts.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });
    Ok(artifacts)
}

fn timeline_diff_paths(events: &[AcpEventEnvelope]) -> Vec<String> {
    let mut paths = Vec::new();
    for envelope in events {
        if envelope.event.event_type != "tool_call"
            && envelope.event.event_type != "tool_call_update"
        {
            continue;
        }
        let Some(content) = envelope
            .event
            .data
            .get("update")
            .and_then(|value| value.get("content"))
        else {
            continue;
        };
        collect_diff_paths(content, &mut paths);
    }
    paths
}

fn collect_diff_paths(value: &serde_json::Value, paths: &mut Vec<String>) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                collect_diff_paths(item, paths);
            }
        }
        serde_json::Value::Object(object) => {
            if object.get("type").and_then(|value| value.as_str()) == Some("diff") {
                if let Some(path) = object.get("path").and_then(|value| value.as_str()) {
                    paths.push(path.to_string());
                }
            }
            for child in object.values() {
                if child.is_array() || child.is_object() {
                    collect_diff_paths(child, paths);
                }
            }
        }
        _ => {}
    }
}

fn resolve_artifact_candidate(root: &Path, candidate: &Path) -> Option<PathBuf> {
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let canonical = fs::canonicalize(joined).ok()?;
    let relative = canonical.strip_prefix(root).ok()?;
    if !canonical.is_file() || !is_deliverable_path(relative) {
        return None;
    }
    Some(canonical)
}

fn is_deliverable_path(path: &Path) -> bool {
    if path.components().any(|component| {
        matches!(component, Component::Normal(value) if value == "tmp" || is_ignored_directory(&value.to_string_lossy()))
    }) {
        return false;
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    DELIVERABLE_EXTENSIONS.contains(&extension.as_str())
}

pub fn capture_baseline(session_id: &str, root: &Path) -> Result<()> {
    let root = canonical_workspace(root)?;
    let git = git_root(&root).is_some_and(|git_root| git_root == root);
    let (dirty_paths, entries) = if git {
        let status = git_status_entries(&root)?;
        let mut entries = BTreeMap::new();
        let mut dirty_paths = BTreeSet::new();
        for change in status {
            dirty_paths.insert(change.relative_path.clone());
            let path = root.join(&change.relative_path);
            if let Some(fingerprint) = fingerprint(&path, true)? {
                entries.insert(change.relative_path, fingerprint);
            }
        }
        (dirty_paths, entries)
    } else {
        (BTreeSet::new(), snapshot_entries(&root)?)
    };
    let baseline = WorkspaceBaseline {
        workspace_path: root.to_string_lossy().into_owned(),
        git,
        dirty_paths,
        entries,
    };
    let path = baseline_path(session_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建工作区基线目录失败: {}", parent.display()))?;
    }
    let payload = serde_json::to_vec_pretty(&baseline).context("序列化工作区基线失败")?;
    let temporary = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&temporary)
            .with_context(|| format!("创建工作区基线失败: {}", temporary.display()))?;
        file.write_all(&payload)
            .with_context(|| format!("写入工作区基线失败: {}", temporary.display()))?;
        file.sync_all().ok();
    }
    fs::rename(&temporary, &path)
        .with_context(|| format!("保存工作区基线失败: {}", path.display()))?;
    Ok(())
}

pub fn workspace_changes(session_id: &str, root: &Path) -> Result<WorkspaceChanges> {
    let root = canonical_workspace(root)?;
    let git = git_root(&root).is_some_and(|git_root| git_root == root);
    let baseline = load_baseline(session_id, &root)?;
    let baseline_available = baseline.is_some();

    let mut changes = if git {
        git_status_entries(&root)?
    } else {
        filesystem_changes(&root, baseline.as_ref())?
    };
    for change in &mut changes {
        change.origin = classify_origin(&root, baseline.as_ref(), &change.relative_path)?;
    }
    changes.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(WorkspaceChanges {
        git,
        branch: git.then(|| git_branch(&root)).flatten(),
        baseline_available,
        changes,
    })
}

pub fn workspace_diff(root: &Path, relative_path: &str) -> Result<WorkspaceDiff> {
    let root = canonical_workspace(root)?;
    let relative = normalize_relative_path(relative_path)?;
    let path = root.join(&relative);
    ensure_path_within_workspace(&root, &path)?;

    let mut text = if git_root(&root).is_some_and(|git_root| git_root == root) {
        let unstaged = git_output(
            &root,
            &["diff", "--no-ext-diff", "--no-color", "--", &relative],
        )?;
        let staged = git_output(
            &root,
            &[
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-color",
                "--",
                &relative,
            ],
        )?;
        let mut combined = String::new();
        if !staged.trim().is_empty() {
            combined.push_str("# 已暂存\n");
            combined.push_str(&staged);
        }
        if !unstaged.trim().is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str("# 未暂存\n");
            combined.push_str(&unstaged);
        }
        if combined.is_empty() && path.is_file() {
            untracked_diff(&path, &relative)?
        } else {
            combined
        }
    } else if path.is_file() {
        let preview = preview_workspace_file(&root, &relative)?;
        preview
            .text
            .unwrap_or_else(|| "该文件不支持文本差异预览。".to_string())
    } else {
        "文件已删除，非 Git 工作区无法还原删除前内容。".to_string()
    };

    let truncated = text.len() > DIFF_LIMIT;
    if truncated {
        text.truncate(DIFF_LIMIT);
        text.push_str("\n\n…差异过大，已截断");
    }
    Ok(WorkspaceDiff {
        relative_path: relative,
        text,
        truncated,
    })
}

pub(super) fn resolve_workspace_references(
    root: &Path,
    relative_paths: &[String],
) -> Result<Vec<WorkspacePromptReference>> {
    let mut unique = BTreeSet::new();
    let mut references = Vec::new();
    for raw in relative_paths {
        let relative = normalize_relative_path(raw)?;
        if !unique.insert(relative.clone()) {
            continue;
        }
        let absolute_path = resolve_existing_path(root, &relative, false)?;
        let metadata = absolute_path
            .metadata()
            .with_context(|| format!("读取工作区文件失败: {}", absolute_path.display()))?;
        if !metadata.is_file() {
            bail!("工作区引用不是文件: {relative}");
        }
        references.push(WorkspacePromptReference {
            relative_path: relative,
            absolute_path,
            size: metadata.len(),
        });
    }
    Ok(references)
}

pub fn resolve_workspace_file(root: &Path, relative_path: &str) -> Result<PathBuf> {
    let relative = normalize_relative_path(relative_path)?;
    resolve_existing_path(root, &relative, false)
}

fn workspace_entry(root: &Path, path: PathBuf) -> Result<Option<WorkspaceEntry>> {
    let metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("读取文件信息失败: {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Ok(None);
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string();
    if name.is_empty() || (metadata.is_dir() && is_ignored_directory(&name)) {
        return Ok(None);
    }
    let kind = if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else {
        return Ok(None);
    };
    let has_children = metadata.is_dir()
        && fs::read_dir(&path)
            .ok()
            .is_some_and(|mut entries| entries.any(|entry| entry.is_ok()));
    Ok(Some(WorkspaceEntry {
        name,
        relative_path: relative_text(root, &path)?,
        kind: kind.to_string(),
        size: if metadata.is_file() {
            metadata.len()
        } else {
            0
        },
        modified: modified_seconds(&metadata),
        has_children,
    }))
}

fn canonical_workspace(root: &Path) -> Result<PathBuf> {
    let canonical =
        fs::canonicalize(root).with_context(|| format!("工作目录不可用: {}", root.display()))?;
    if !canonical.is_dir() {
        bail!("工作目录不可用: {}", canonical.display());
    }
    Ok(canonical)
}

fn normalize_relative_path(raw: &str) -> Result<String> {
    let path = Path::new(raw);
    if path.is_absolute() {
        bail!("工作区路径必须是相对路径");
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("工作区路径不能越过根目录")
            }
        }
    }
    Ok(parts.join("/"))
}

fn resolve_existing_path(root: &Path, relative: &str, directory: bool) -> Result<PathBuf> {
    let root = canonical_workspace(root)?;
    let candidate = if relative.is_empty() {
        root.clone()
    } else {
        root.join(relative)
    };
    let canonical =
        fs::canonicalize(&candidate).with_context(|| format!("工作区路径不存在: {relative}"))?;
    ensure_path_within_workspace(&root, &canonical)?;
    if directory && !canonical.is_dir() {
        bail!("不是目录: {relative}");
    }
    Ok(canonical)
}

fn ensure_path_within_workspace(root: &Path, path: &Path) -> Result<()> {
    if !path.starts_with(root) {
        bail!("工作区路径越过了项目根目录");
    }
    Ok(())
}

fn relative_text(root: &Path, path: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(root)
        .with_context(|| format!("路径不在工作区内: {}", path.display()))?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn modified_seconds(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default()
}

fn is_ignored_directory(name: &str) -> bool {
    IGNORED_DIRECTORIES.contains(&name)
}

fn should_walk(entry: &DirEntry) -> bool {
    entry.depth() == 0
        || !entry.file_type().is_dir()
        || !is_ignored_directory(&entry.file_name().to_string_lossy())
}

fn file_kind(path: &Path) -> String {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg"
    ) {
        return "image".to_string();
    }
    if matches!(
        extension.as_str(),
        "md" | "markdown"
            | "txt"
            | "log"
            | "csv"
            | "json"
            | "yaml"
            | "yml"
            | "toml"
            | "xml"
            | "html"
            | "htm"
            | "rs"
            | "py"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "go"
            | "c"
            | "cpp"
            | "h"
            | "hpp"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "bat"
            | "cmd"
            | "ps1"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "vue"
            | "svelte"
            | "sql"
            | "ini"
            | "conf"
            | "cfg"
            | "env"
            | "properties"
            | "diff"
            | "patch"
            | "lock"
            | "proto"
            | "graphql"
            | "gql"
            | "prisma"
            | "java"
            | "kt"
            | "swift"
    ) {
        return "text".to_string();
    }
    "binary".to_string()
}

fn image_mime_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        _ => "image/png",
    }
}

fn baseline_path(session_id: &str) -> PathBuf {
    crate::platform::paths::sessions_root()
        .join(session_id)
        .join("codex-workspace-baseline.json")
}

fn load_baseline(session_id: &str, root: &Path) -> Result<Option<WorkspaceBaseline>> {
    let path = baseline_path(session_id);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("读取工作区基线失败: {}", path.display()))
        }
    };
    let baseline: WorkspaceBaseline =
        serde_json::from_slice(&bytes).context("解析工作区基线失败")?;
    if baseline.workspace_path != root.to_string_lossy() {
        return Ok(None);
    }
    Ok(Some(baseline))
}

fn snapshot_entries(root: &Path) -> Result<BTreeMap<String, FileFingerprint>> {
    let mut entries = BTreeMap::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_walk)
        .filter_map(|entry| entry.ok())
        .take(WALK_LIMIT)
    {
        if entry.depth() == 0 || !entry.file_type().is_file() {
            continue;
        }
        let relative = relative_text(root, entry.path())?;
        if let Some(fingerprint) = fingerprint(entry.path(), false)? {
            entries.insert(relative, fingerprint);
        }
    }
    Ok(entries)
}

fn fingerprint(path: &Path, include_hash: bool) -> Result<Option<FileFingerprint>> {
    let metadata = match path.metadata() {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("读取文件信息失败: {}", path.display()))
        }
    };
    let sha256 = if include_hash {
        let mut file =
            fs::File::open(path).with_context(|| format!("打开文件失败: {}", path.display()))?;
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .with_context(|| format!("读取文件失败: {}", path.display()))?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        Some(crate::platform::encoding::hex_lower(&digest.finalize()))
    } else {
        None
    };
    Ok(Some(FileFingerprint {
        size: metadata.len(),
        modified: modified_seconds(&metadata),
        sha256,
    }))
}

fn filesystem_changes(
    root: &Path,
    baseline: Option<&WorkspaceBaseline>,
) -> Result<Vec<WorkspaceChange>> {
    let current = snapshot_entries(root)?;
    let Some(baseline) = baseline else {
        return Ok(current
            .keys()
            .map(|path| WorkspaceChange {
                relative_path: path.clone(),
                status: "unknown".to_string(),
                staged: false,
                origin: "unknown".to_string(),
            })
            .collect());
    };
    let mut paths = BTreeSet::new();
    paths.extend(current.keys().cloned());
    paths.extend(baseline.entries.keys().cloned());
    Ok(paths
        .into_iter()
        .filter_map(
            |path| match (baseline.entries.get(&path), current.get(&path)) {
                (None, Some(_)) => Some(("added", path)),
                (Some(_), None) => Some(("deleted", path)),
                (Some(before), Some(after)) if before != after => Some(("modified", path)),
                _ => None,
            },
        )
        .map(|(status, relative_path)| WorkspaceChange {
            relative_path,
            status: status.to_string(),
            staged: false,
            origin: "session".to_string(),
        })
        .collect())
}

fn git_root(root: &Path) -> Option<PathBuf> {
    let output = crate::platform::process::HiddenCommand::new("git")
        .current_dir(root)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    fs::canonicalize(String::from_utf8_lossy(&output.stdout).trim()).ok()
}

fn git_branch(root: &Path) -> Option<String> {
    let branch = git_output(root, &["branch", "--show-current"]).ok()?;
    let branch = branch.trim();
    (!branch.is_empty()).then(|| branch.to_string())
}

fn git_status_entries(root: &Path) -> Result<Vec<WorkspaceChange>> {
    let output = crate::platform::process::HiddenCommand::new("git")
        .current_dir(root)
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            ".",
        ])
        .output()
        .context("执行 git status 失败")?;
    if !output.status.success() {
        bail!(
            "git status 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let records = output.stdout.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut changes = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.len() < 4 {
            continue;
        }
        let x = record[0] as char;
        let y = record[1] as char;
        let path = String::from_utf8_lossy(&record[3..]).replace('\\', "/");
        if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            index += 1;
        }
        changes.push(WorkspaceChange {
            relative_path: path,
            status: git_status_label(x, y).to_string(),
            staged: x != ' ' && x != '?',
            origin: "unknown".to_string(),
        });
    }
    Ok(changes)
}

fn git_status_label(x: char, y: char) -> &'static str {
    if x == '?' && y == '?' {
        "untracked"
    } else if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') {
        "conflict"
    } else if x == 'A' || y == 'A' {
        "added"
    } else if x == 'D' || y == 'D' {
        "deleted"
    } else if x == 'R' || y == 'R' {
        "renamed"
    } else if x == 'C' || y == 'C' {
        "copied"
    } else {
        "modified"
    }
}

fn git_output(root: &Path, arguments: &[&str]) -> Result<String> {
    let output = crate::platform::process::HiddenCommand::new("git")
        .current_dir(root)
        .args(arguments)
        .output()
        .with_context(|| format!("执行 git {} 失败", arguments.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} 失败: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn classify_origin(
    root: &Path,
    baseline: Option<&WorkspaceBaseline>,
    relative_path: &str,
) -> Result<String> {
    let Some(baseline) = baseline else {
        return Ok("unknown".to_string());
    };
    if !baseline.git || !baseline.dirty_paths.contains(relative_path) {
        return Ok("session".to_string());
    }
    let before = baseline.entries.get(relative_path);
    let current = fingerprint(&root.join(relative_path), baseline.git)?;
    if current.as_ref() == before {
        Ok("preexisting".to_string())
    } else {
        Ok("preexisting_modified".to_string())
    }
}

fn untracked_diff(path: &Path, relative: &str) -> Result<String> {
    if file_kind(path) != "text" {
        return Ok("未跟踪的二进制文件不支持差异预览。".to_string());
    }
    let preview = fs::read_to_string(path)
        .with_context(|| format!("读取未跟踪文件失败: {}", path.display()))?;
    let mut output = format!(
        "diff --git a/{0} b/{0}\nnew file mode 100644\n--- /dev/null\n+++ b/{0}\n",
        relative
    );
    for line in preview.lines() {
        output.push('+');
        output.push_str(line);
        output.push('\n');
        if output.len() > DIFF_LIMIT {
            break;
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "pinvou3-codex-workspace-{label}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rejects_workspace_escape() {
        let root = TestDir::new("escape-root");
        assert!(normalize_relative_path("../secret.txt").is_err());
        assert!(normalize_relative_path("/absolute/secret.txt").is_err());
        assert!(resolve_workspace_file(root.path(), "missing.txt").is_err());
    }

    #[test]
    fn lists_and_previews_workspace_files_without_build_directories() {
        let root = TestDir::new("listing");
        fs::create_dir_all(root.path().join("src")).unwrap();
        fs::create_dir_all(root.path().join("node_modules/pkg")).unwrap();
        fs::write(root.path().join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.path().join("node_modules/pkg/index.js"), "ignored").unwrap();

        let listing = list_workspace(root.path(), None).unwrap();
        assert!(listing.entries.iter().any(|entry| entry.name == "src"));
        assert!(!listing
            .entries
            .iter()
            .any(|entry| entry.name == "node_modules"));
        let preview = preview_workspace_file(root.path(), "src/main.rs").unwrap();
        assert_eq!(preview.kind, "text");
        assert_eq!(preview.text.as_deref(), Some("fn main() {}"));
    }

    #[test]
    fn non_git_baseline_detects_added_modified_and_deleted_files() {
        let root = TestDir::new("baseline");
        let session_id = format!("workspace-test-{}", std::process::id());
        fs::write(root.path().join("before.txt"), "before").unwrap();
        fs::write(root.path().join("delete.txt"), "delete").unwrap();
        capture_baseline(&session_id, root.path()).unwrap();
        fs::write(root.path().join("before.txt"), "after-and-longer").unwrap();
        fs::remove_file(root.path().join("delete.txt")).unwrap();
        fs::write(root.path().join("added.txt"), "added").unwrap();

        let changes = workspace_changes(&session_id, root.path()).unwrap();
        assert!(changes
            .changes
            .iter()
            .any(|change| change.relative_path == "before.txt" && change.status == "modified"));
        assert!(changes
            .changes
            .iter()
            .any(|change| change.relative_path == "delete.txt" && change.status == "deleted"));
        assert!(changes.changes.iter().any(|change| {
            change.relative_path == "added.txt"
                && change.status == "added"
                && change.origin == "session"
        }));
        let _ = fs::remove_file(baseline_path(&session_id));
    }

    #[test]
    fn git_baseline_distinguishes_existing_and_session_changes() {
        let root = TestDir::new("git-origin");
        let path = root.path().join("dirty.txt");
        fs::write(&path, "before").unwrap();
        let before = fingerprint(&path, true).unwrap().unwrap();
        let baseline = WorkspaceBaseline {
            workspace_path: root.path().to_string_lossy().into_owned(),
            git: true,
            dirty_paths: BTreeSet::from(["dirty.txt".to_string()]),
            entries: BTreeMap::from([("dirty.txt".to_string(), before)]),
        };

        assert_eq!(
            classify_origin(root.path(), Some(&baseline), "dirty.txt").unwrap(),
            "preexisting"
        );
        fs::write(&path, "after and longer").unwrap();
        assert_eq!(
            classify_origin(root.path(), Some(&baseline), "dirty.txt").unwrap(),
            "preexisting_modified"
        );
        fs::write(root.path().join("new.txt"), "new").unwrap();
        assert_eq!(
            classify_origin(root.path(), Some(&baseline), "new.txt").unwrap(),
            "session"
        );
    }

    #[test]
    fn temporary_workspace_discovers_deliverables_without_process_files() {
        let root = TestDir::new("temporary-artifacts");
        fs::create_dir_all(root.path().join("public")).unwrap();
        fs::create_dir_all(root.path().join("tmp")).unwrap();
        fs::create_dir_all(root.path().join("node_modules/pkg")).unwrap();
        fs::write(root.path().join("public/game.html"), "<html>game</html>").unwrap();
        fs::write(root.path().join("README.md"), "# Game").unwrap();
        fs::write(root.path().join("package.json"), "{}").unwrap();
        fs::write(root.path().join("tmp/draft.html"), "draft").unwrap();
        fs::write(root.path().join("node_modules/pkg/demo.html"), "ignored").unwrap();

        let artifacts =
            discover_artifacts("missing-baseline", root.path(), true, &[], &[]).unwrap();
        let paths = artifacts
            .iter()
            .map(|artifact| artifact.relative_path.as_str())
            .collect::<Vec<_>>();
        assert!(paths.contains(&"public/game.html"));
        assert!(paths.contains(&"README.md"));
        assert!(!paths.contains(&"package.json"));
        assert!(!paths.contains(&"tmp/draft.html"));
        assert!(!paths.contains(&"node_modules/pkg/demo.html"));
    }

    #[test]
    fn project_workspace_uses_structured_diff_paths_and_rejects_escape() {
        let root = TestDir::new("project-artifacts");
        let outside = TestDir::new("outside-artifacts");
        fs::create_dir_all(root.path().join("public")).unwrap();
        fs::write(root.path().join("public/game.html"), "<html>game</html>").unwrap();
        fs::write(root.path().join("preexisting.md"), "old").unwrap();
        fs::write(outside.path().join("secret.html"), "secret").unwrap();
        let event: AcpEventEnvelope = serde_json::from_value(serde_json::json!({
            "version": 1,
            "sessionId": "session-1",
            "turnId": "turn-1",
            "seq": 1,
            "timestamp": "2026-01-01T00:00:00Z",
            "event": {
                "type": "tool_call",
                "data": {
                    "update": {
                        "kind": "edit",
                        "content": [
                            { "type": "diff", "path": "public/game.html" },
                            { "type": "diff", "path": outside.path().join("secret.html") }
                        ]
                    }
                }
            }
        }))
        .unwrap();

        let artifacts =
            discover_artifacts("missing-baseline", root.path(), false, &[event], &[]).unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].relative_path, "public/game.html");
    }

    #[test]
    fn project_workspace_discovers_shell_generated_deliverables_from_baseline() {
        let root = TestDir::new("project-shell-artifacts");
        let session_id = format!("workspace-artifact-test-{}", std::process::id());
        fs::write(root.path().join("preexisting.md"), "old").unwrap();
        capture_baseline(&session_id, root.path()).unwrap();
        fs::create_dir_all(root.path().join("release")).unwrap();
        fs::write(
            root.path().join("release/report.html"),
            "<html>report</html>",
        )
        .unwrap();

        let artifacts = discover_artifacts(&session_id, root.path(), false, &[], &[]).unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].relative_path, "release/report.html");
        let _ = fs::remove_file(baseline_path(&session_id));
    }
}
