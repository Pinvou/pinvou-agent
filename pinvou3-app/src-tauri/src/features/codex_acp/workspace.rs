use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::UNIX_EPOCH;

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::{DirEntry, WalkDir};

const LIST_LIMIT: usize = 500;
const SEARCH_LIMIT: usize = 300;
const WALK_LIMIT: usize = 20_000;
const PREVIEW_LIMIT: usize = 512 * 1024;
const IMAGE_PREVIEW_LIMIT: u64 = 10 * 1024 * 1024;
const DIFF_LIMIT: usize = 1024 * 1024;

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

/// diff 内存缓存：键 = (session_id, 相对路径)，值 = 文件指纹 + diff 文本。
/// 指纹覆盖工作区文件（size + mtime 纳秒 + 头尾采样 hash——内容变而 size/mtime
/// 未变（同尺寸覆盖写）也能失效）与 `.git/index`（git add/reset 等暂存区变化
/// 会更新 index），任一变化即失效；仅进程内存、不落盘，重启即清空。
const DIFF_CACHE_MAX_ENTRIES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffFingerprint {
    file_size: u64,
    file_modified: u128,
    head_tail_hash: u64,
    index_size: u64,
    index_modified: u128,
}

#[derive(Clone)]
struct CachedDiff {
    fingerprint: DiffFingerprint,
    text: String,
    truncated: bool,
}

static DIFF_CACHE: LazyLock<Mutex<HashMap<(String, String), CachedDiff>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn modified_nanos(metadata: &fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or_default()
}

// 头 4KB + 尾 4KB 的 FNV-1a 采样：文件内容变化（含同尺寸覆盖写）时大概率失效，
// 每次开销恒定 ~8KB 读取，远小于重跑 git diff。
fn sample_head_tail_hash(path: &Path, len: u64) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut visit = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x1_0000_0000_01b3);
        }
    };
    if let Ok(mut file) = fs::File::open(path) {
        let head_len = 4096.min(len as usize);
        let mut head = vec![0u8; head_len];
        if file.read_exact(&mut head).is_ok() {
            visit(&head);
        }
        if len > 8192 {
            let tail_len = 4096.min((len - head_len as u64) as usize);
            let mut tail = vec![0u8; tail_len];
            if file.seek(SeekFrom::End(-(tail_len as i64))).is_ok()
                && file.read_exact(&mut tail).is_ok()
            {
                visit(&tail);
            }
        }
    }
    hash
}

fn diff_fingerprint(root: &Path, relative: &str) -> DiffFingerprint {
    let mut fingerprint = DiffFingerprint {
        file_size: 0,
        file_modified: 0,
        head_tail_hash: 0,
        index_size: 0,
        index_modified: 0,
    };
    if let Ok(metadata) = root.join(relative).metadata() {
        fingerprint.file_size = metadata.len();
        fingerprint.file_modified = modified_nanos(&metadata);
        fingerprint.head_tail_hash = sample_head_tail_hash(&root.join(relative), metadata.len());
    }
    // git 暂存区变化会重写 .git/index；非 git 工作区无此文件，字段保持 0。
    if let Ok(metadata) = root.join(".git/index").metadata() {
        fingerprint.index_size = metadata.len();
        fingerprint.index_modified = modified_nanos(&metadata);
    }
    fingerprint
}

fn diff_cache_get(session_id: &str, root: &Path, relative: &str) -> Option<CachedDiff> {
    let fingerprint = diff_fingerprint(root, relative);
    DIFF_CACHE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&(session_id.to_string(), relative.to_string()))
        .filter(|cached| cached.fingerprint == fingerprint)
        .cloned()
}

fn diff_cache_put(session_id: &str, root: &Path, relative: &str, text: String, truncated: bool) {
    let fingerprint = diff_fingerprint(root, relative);
    let mut cache = DIFF_CACHE.lock().unwrap_or_else(|error| error.into_inner());
    // 简单上限：超限整体清空，避免进程内无限膨胀（纯内存，不落盘）。
    if cache.len() >= DIFF_CACHE_MAX_ENTRIES {
        cache.clear();
    }
    cache.insert(
        (session_id.to_string(), relative.to_string()),
        CachedDiff {
            fingerprint,
            text,
            truncated,
        },
    );
}

pub fn workspace_diff(session_id: &str, root: &Path, relative_path: &str) -> Result<WorkspaceDiff> {
    let root = canonical_workspace(root)?;
    let relative = normalize_relative_path(relative_path)?;
    let path = root.join(&relative);
    ensure_path_within_workspace(&root, &path)?;

    if let Some(cached) = diff_cache_get(session_id, &root, &relative) {
        return Ok(WorkspaceDiff {
            relative_path: relative,
            text: cached.text,
            truncated: cached.truncated,
        });
    }

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
    diff_cache_put(session_id, &root, &relative, text.clone(), truncated);
    Ok(WorkspaceDiff {
        relative_path: relative,
        text,
        truncated,
    })
}

/// 校验相对路径落在工作区内，但不要求文件存在（git diff 可展示已删除文件）。
pub fn validate_workspace_relative_path(root: &Path, relative_path: &str) -> Result<()> {
    let relative = normalize_relative_path(relative_path)?;
    let path = root.join(&relative);
    ensure_path_within_workspace(root, &path)
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
            | "rb"
            | "php"
            | "cs"
            | "lua"
            | "scala"
            | "gradle"
            | "tf"
            | "tex"
            | "rst"
            | "pl"
            | "pm"
            | "r"
            | "m"
            | "mm"
    ) {
        return "text".to_string();
    }
    // 无扩展名的常见文本文件名（Dockerfile、Makefile、LICENSE、.gitignore 等）。
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        file_name.as_str(),
        "dockerfile"
            | "makefile"
            | "gnumakefile"
            | "jenkinsfile"
            | "vagrantfile"
            | "gemfile"
            | "rakefile"
            | "brewfile"
            | "cmakelists.txt"
            | "license"
            | "licence"
            | "copying"
            | "notice"
            | "authors"
            | "contributors"
            | "changelog"
            | "readme"
            | ".gitignore"
            | ".gitattributes"
            | ".gitmodules"
            | ".editorconfig"
            | ".npmrc"
            | ".yarnrc"
            | ".env"
            | ".envrc"
    ) {
        return "text".to_string();
    }
    // 扩展名白名单之外的内容嗅探：头部 8KB 无 NUL 且为合法 UTF-8 即按文本处理，
    // 覆盖 .hbs/.ipynb/.cmake 等未列举扩展名及无扩展名的文本文件。
    if looks_like_text(path) {
        return "text".to_string();
    }
    "binary".to_string()
}

fn looks_like_text(path: &Path) -> bool {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut buffer = [0u8; 8192];
    let read = std::io::Read::read(&mut file, &mut buffer).unwrap_or(0);
    let sample = &buffer[..read];
    if sample.contains(&0) {
        return false;
    }
    match std::str::from_utf8(sample) {
        Ok(_) => true,
        // 8KB 截断点恰好落在多字节字符中间时，容忍末尾不完整序列（≤4 字节）。
        Err(error) => error.valid_up_to() > 0 && sample.len() - error.valid_up_to() <= 4,
    }
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
    fn validates_relative_path_without_requiring_file_existence() {
        let root = TestDir::new("validate-relative");
        assert!(validate_workspace_relative_path(root.path(), "src/main.py").is_ok());
        assert!(validate_workspace_relative_path(root.path(), "").is_ok());
        assert!(validate_workspace_relative_path(root.path(), "../secret.txt").is_err());
        assert!(validate_workspace_relative_path(root.path(), "/absolute/secret.txt").is_err());
    }

    #[test]
    fn diff_cache_hits_within_session_and_invalidates_on_file_change() {
        // 无真实 git 仓库时 workspace_diff 走文件系统对比分支，内容随文件变化——正适合验证缓存失效。
        let root = TestDir::new("diff-cache");
        fs::write(root.path().join("main.py"), "print(1)\n").unwrap();

        let first = workspace_diff("s-1", root.path(), "main.py").unwrap();
        assert!(first.text.contains("print(1)"));
        // 同会话、文件未变：命中缓存，内容一致。
        let second = workspace_diff("s-1", root.path(), "main.py").unwrap();
        assert_eq!(first.text, second.text);

        // 文件变化 → 指纹失效，重新计算出新内容。
        fs::write(root.path().join("main.py"), "print(2)\n").unwrap();
        let third = workspace_diff("s-1", root.path(), "main.py").unwrap();
        assert_ne!(first.text, third.text);
        assert!(third.text.contains("print(2)"));
        // 稳定后再次命中缓存。
        let fourth = workspace_diff("s-1", root.path(), "main.py").unwrap();
        assert_eq!(third.text, fourth.text);
    }

    #[test]
    fn diff_fingerprint_covers_file_and_git_index() {
        let root = TestDir::new("diff-fingerprint");
        fs::create_dir_all(root.path().join(".git")).unwrap();
        fs::write(root.path().join(".git/index"), b"idx1").unwrap();
        fs::write(root.path().join("main.py"), "print(1)\n").unwrap();

        let baseline = diff_fingerprint(root.path(), "main.py");
        // 文件内容变化 → 指纹变化。
        fs::write(root.path().join("main.py"), "print(2)\n").unwrap();
        assert_ne!(baseline, diff_fingerprint(root.path(), "main.py"));
        // 暂存区变化（.git/index 重写）→ 指纹变化。
        fs::write(root.path().join(".git/index"), b"idx2").unwrap();
        assert_ne!(baseline, diff_fingerprint(root.path(), "main.py"));
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
    fn preview_kind_covers_special_names_and_sniffs_unknown_text() {
        let root = TestDir::new("file-kind");
        fs::write(root.path().join("Dockerfile"), "FROM scratch\n").unwrap();
        fs::write(root.path().join("LICENSE"), "MIT License\n").unwrap();
        fs::write(root.path().join(".gitignore"), "target/\n").unwrap();
        fs::write(root.path().join("script.rb"), "puts 1\n").unwrap();
        fs::write(root.path().join("notes"), "plain text without extension\n").unwrap();
        fs::write(root.path().join("empty.weird"), "").unwrap();
        fs::write(
            root.path().join("payload.bin"),
            [0x89, 0x50, 0x4E, 0x47, 0x00, 0x0D],
        )
        .unwrap();

        for (path, expected) in [
            ("Dockerfile", "text"),
            ("LICENSE", "text"),
            (".gitignore", "text"),
            ("script.rb", "text"),
            ("notes", "text"),
            ("empty.weird", "text"),
            ("payload.bin", "binary"),
        ] {
            let preview = preview_workspace_file(root.path(), path).unwrap();
            assert_eq!(preview.kind, expected, "unexpected kind for {path}");
        }
        let preview = preview_workspace_file(root.path(), "notes").unwrap();
        assert_eq!(
            preview.text.as_deref(),
            Some("plain text without extension\n")
        );
    }

    #[test]
    fn preview_kind_tolerates_utf8_boundary_at_sniff_cutoff() {
        let root = TestDir::new("utf8-boundary");
        // 8191 个 ASCII + 一个三字节字符：8KB 采样正好切在该字符中间。
        let content = format!("{}中", "a".repeat(8191));
        fs::write(root.path().join("boundary.unknown"), content).unwrap();
        let preview = preview_workspace_file(root.path(), "boundary.unknown").unwrap();
        assert_eq!(preview.kind, "text");
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
}
