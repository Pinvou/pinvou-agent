// architecture-guard: allow-target-cfg -- the round-24 owner-probe EACCES pin is cfg(unix)-gated: the fault is a chmod-0000 sessions directory with no portable non-unix equivalent; the test self-skips when the mode is not enforced and no platform behavior leaks into shared code.
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::json;

// Round-46 M2: bumped 5 -> 6 — this PR persists two new per-record fields
// (workspace_roots, explicit_ancestor_roots); binaries WITHOUT this gate
// strip them on their own whole-table persists. Readers carrying the check
// refuse the newer index instead (rebuild from sidecars, not data loss);
// every reader in THIS binary checks it (round-48 minor 3: the refusal is
// a property of the checking reader, not of the file's writer era).
const STORE_VERSION: u32 = 6;
const CONFIG_DEFAULTS_VERSION: u32 = 1;
/// 原生代码会话 sidecar 的 schema 版本；未来字段演进时用于迁移。
/// Round-41 M4: 2 adds the `explicit_ancestor_roots` provenance field (the
/// same downgrade-refusal rationale as the workspace-binding sidecar).
const CODE_SESSION_SIDECAR_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum AgentBackend {
    #[default]
    Deepseek,
    CodexAcp,
    ClaudeAcp,
    KimiAcp,
}

// parse/agent_id 是 backend 字符串表示(序列化契约)的公开解析入口,生产链路
// 在用;as_str 仅测试断言别名契约,已单独 #[cfg(test)] 门控。
impl AgentBackend {
    /// ACP Agent 目录的单一注册表。新增后端时只需在这里登记，列表、状态与 UI
    /// 都从同一顺序派生，避免某一端遗漏后只显示部分 Agent。
    pub const ACP_BACKENDS: [Self; 3] = [Self::CodexAcp, Self::ClaudeAcp, Self::KimiAcp];

    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value.unwrap_or("deepseek") {
            "deepseek" | "pinvou" => Ok(Self::Deepseek),
            "codex-acp" | "codex" => Ok(Self::CodexAcp),
            "claude-acp" | "claude" => Ok(Self::ClaudeAcp),
            "kimi-acp" | "kimi" => Ok(Self::KimiAcp),
            other => anyhow::bail!("不支持的 Agent 后端: {other}"),
        }
    }

    #[cfg(test)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deepseek => "deepseek",
            Self::CodexAcp => "codex-acp",
            Self::ClaudeAcp => "claude-acp",
            Self::KimiAcp => "kimi-acp",
        }
    }

    pub fn agent_id(self) -> Option<&'static str> {
        match self {
            Self::Deepseek => None,
            Self::CodexAcp => Some("codex"),
            Self::ClaudeAcp => Some("claude"),
            Self::KimiAcp => Some("kimi"),
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Deepseek => "品悟",
            Self::CodexAcp => "Codex",
            Self::ClaudeAcp => "Claude Code",
            Self::KimiAcp => "Kimi",
        }
    }

    pub fn is_acp(self) -> bool {
        !matches!(self, Self::Deepseek)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CodexWorkspaceKind {
    #[default]
    Temporary,
    Project,
}

/// 产品模式类型已上移到 core（store/bridge 策略等多 feature 共用）；
/// 这里 re-export 保持既有 `store::SessionMode` 路径可用。持久化关注点
/// （`code_session` 键的布尔兼容 serde）留在本模块（见 `session_mode_serde`）。
pub use crate::core::session_mode::SessionMode;

/// `code_session` 键的兼容 serde：写布尔（旧格式），读布尔或字符串。
mod session_mode_serde {
    use super::SessionMode;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(mode: &SessionMode, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(mode.is_code())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<SessionMode, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Compat {
            Bool(bool),
            Mode(SessionMode),
        }
        Ok(match Compat::deserialize(deserializer)? {
            Compat::Bool(true) => SessionMode::Code,
            Compat::Bool(false) => SessionMode::Plain,
            Compat::Mode(mode) => mode,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionAgentRecord {
    #[serde(default)]
    pub backend: AgentBackend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_model_id: Option<String>,
    /// 用户明确选择的 ACP 权限模式。ACP Agent 在 new/load 时可能恢复默认
    /// `agent`，所以 Pinvou 必须在运行时就绪前重新应用该期望值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_mode_id: Option<String>,
    /// 当前 Pinvou 会话最后一次确认成功的 ACP 配置。
    ///
    /// `model` / `mode` 仍保留独立字段用于兼容旧版本；其余 Agent 动态上报的
    /// 配置项统一保存在这里，恢复同一会话时不会被 Agent 默认值覆盖。
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub acp_config_values: HashMap<String, String>,
    /// ACP Agent 的执行目录类型。旧记录没有该字段时按临时会话兼容。
    #[serde(default)]
    pub workspace_kind: CodexWorkspaceKind,
    /// 项目会话保存创建时选定的绝对目录；临时会话目录由 session id 推导。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<PathBuf>,
    /// Keychain snapshot locked at creation (§6): the full accessible roots
    /// (including the primary root). Old records missing this key or empty =
    /// single-root semantics (the caller normalizes by cwd).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_roots: Vec<PathBuf>,
    /// Round-40 (zhuowp P2): the ancestors EXPLICITLY aligned into the
    /// keychain — the grant's recorded provenance. Delivery exempts exactly
    /// these spellings from the ancestor re-check, so removing the backing
    /// project folder / deleting the project / moving the session out does
    /// not revoke a granted ancestor while its directory is unchanged; the
    /// grant's authority is what align recorded, not mutable project state.
    /// Symlink widening is still rejected: the exemption matches the
    /// canonicalized recorded spelling only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub explicit_ancestor_roots: Vec<PathBuf>,
    /// 产品模式（plain/code）。旧记录没有该字段时按 plain 兼容；
    /// 序列化保持原布尔格式（true=code），旧版本应用读新文件不误判。
    #[serde(
        default,
        rename = "code_session",
        with = "session_mode_serde",
        skip_serializing_if = "SessionMode::is_plain"
    )]
    pub mode: SessionMode,
}

/// 原生代码会话的权威 sidecar（per-session 持久化真相源）。
///
/// `session-agents.json` 是运行期辅助索引，损坏/丢失后可以被丢弃重建；而原生
/// 代码会话的类型与项目目录绑定必须跨进程持久存在，否则会话会静默掉回普通
/// 会话、执行根退回私有目录。本 sidecar 存于 session 私有目录
/// `~/.pinvou3/sessions/<sid>/code-session.json`，随会话存续，是辅助索引缺失时
/// 恢复 `SessionAgentRecord` 的依据（见 `restore_missing_code_session_record`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeSessionSidecar {
    #[serde(default = "code_session_sidecar_version")]
    pub version: u32,
    /// 原生代码会话的工作区类型。
    pub workspace_kind: CodexWorkspaceKind,
    /// 项目会话保存的绝对目录；临时会话为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<PathBuf>,
    /// Keychain snapshot locked at creation (§6): the full accessible roots
    /// (including the primary root); an old sidecar missing this key or empty
    /// = single-root semantics.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_roots: Vec<PathBuf>,
    /// Round-11 SF2: the align-granted ancestor provenance for the roots
    /// above. An old sidecar missing the key = no recorded grant (same
    /// fail-safe direction as an empty set); without this field an
    /// index-loss restore rebuilt the record with empty provenance and the
    /// round-40 explicit-ancestor grants silently evaporated in exactly the
    /// recovery window they exist for (the plain lane already persisted its
    /// provenance in its own sidecar).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub explicit_ancestor_roots: Vec<PathBuf>,
    /// 首次绑定时间（Unix 秒）。仅作元信息，不参与恢复语义。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_at: Option<i64>,
}

/// Output of `rebind_workspace_prefix`.
#[derive(Debug, Default)]
pub struct RebindWorkspacePrefixOutcome {
    /// Affected (session_id, translated path): index-matched sessions plus
    /// orphan sidecars whose rewrite succeeded. An orphan whose sidecar write
    /// failed is NOT listed — the sidecar is its binding, so claiming it
    /// moved to the new path would be a false report (review #463 nit).
    pub affected: Vec<(String, PathBuf)>,
    /// Sessions whose sidecar is finally stale: no self-healing path remains
    /// in this run. Two sources — orphans (rewrite pass failed, no backfill
    /// path) and indexed sessions (both the rewrite and the rewrite-retry
    /// pass failed). A stale sidecar still sits under the `from` prefix; the
    /// command layer should count it into failed_session_ids so a user rerun
    /// converges it via the on-disk prefix scan. Not reporting it would let a
    /// restore with a damaged index resurrect the old path and silently undo
    /// the rebind (review #463 Major 2).
    pub sidecar_final_stale: Vec<String>,
}

/// Session ids that own a directory under the code-session sidecar root.
///
/// Single source of truth for every scan that walks this directory — the boot
/// restore in `mod.rs`, the rebind candidate scan in
/// [`SessionAgentStore::sessions_under_workspace`] and the rewrite pass in
/// [`SessionAgentStore::rebind_workspace_prefix`] (review #463 round-8
/// elegance): the hand-rolled copies had already drifted on entry typing and
/// error handling. A missing root — no sidecar ever written — is normal and
/// yields nothing; any other `read_dir` failure is logged, never silently
/// swallowed (review #463 minor: failing open would let boot restore
/// resurrect old paths with zero signal). Directory entries that cannot be
/// read are logged and skipped, so one bad entry never hides the rest.
/// Round-35 M2: the CHECKED twin of [`code_session_dir_ids`] for the
/// expulsion/delete enumeration lanes — a transient EACCES on the sidecar
/// root must fail the command (nothing has moved yet; a retry re-enumerates
/// cleanly) instead of returning empty, silently under-enumerating off-index
/// orphan members whose tombstone the store transaction then skips forever
/// (the "root removed, member revived" class the sessions lane fixed in
/// round-21 #13). `NotFound` still yields empty: a home that never ran a
/// code session has no sidecar root, and that is not an error.
pub(super) fn try_code_session_dir_ids(
    store_path: &Path,
    context: &str,
) -> std::io::Result<Vec<String>> {
    let root = code_session_sidecar_root(store_path);
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(std::io::Error::new(
                error.kind(),
                format!("{context}: sidecar root {} unreadable", root.display()),
            ));
        }
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!("[pinvou3-app] {context}: unreadable sidecar entry skipped ({error})");
                continue;
            }
        };
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            ids.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    Ok(ids)
}

pub(super) fn code_session_dir_ids(store_path: &Path, context: &str) -> Vec<String> {
    let root = code_session_sidecar_root(store_path);
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            eprintln!(
                "[pinvou3-app] sidecar root unreadable during {context}: {} ({error})",
                root.display()
            );
            return Vec::new();
        }
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!("[pinvou3-app] {context}: unreadable sidecar entry skipped ({error})");
                continue;
            }
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let Some(session_id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        ids.push(session_id);
    }
    ids
}

fn code_session_sidecar_version() -> u32 {
    CODE_SESSION_SIDECAR_VERSION
}

/// 原生代码会话 sidecar 的根目录：`<session-agents.json 父目录>/sessions`。
/// 生产为 `~/.pinvou3/sessions`，测试随 store.path 一并隔离。
/// 启动扫描（mod.rs）与 sidecar 读写共用本函数，保证「扫描根 == 读取根」单一来源。
pub(super) fn code_session_sidecar_root(store_path: &Path) -> PathBuf {
    store_path
        .parent()
        .map(|parent| parent.join("sessions"))
        .unwrap_or_else(crate::platform::paths::sessions_root)
}

/// 该 session 的原生代码会话 sidecar 路径。
fn code_session_sidecar_path(store_path: &Path, session_id: &str) -> PathBuf {
    code_session_sidecar_root(store_path)
        .join(session_id)
        .join("code-session.json")
}

/// Translate every `from`-prefixed root onto `to` (suffix-cut); `None`
/// when nothing matched (round-30 NIT: the dead "translated" defensive
/// branch folded into an Option).
fn translate_roots_under(roots: &[PathBuf], from: &Path, to: &Path) -> Option<Vec<PathBuf>> {
    let mut translated = false;
    let next: Vec<PathBuf> = roots
        .iter()
        .map(
            |root| match crate::platform::os::path_relative_suffix_under(root, from) {
                Some(suffix) if suffix.as_os_str().is_empty() => {
                    translated = true;
                    to.to_path_buf()
                }
                Some(suffix) => {
                    translated = true;
                    to.join(suffix)
                }
                None => root.clone(),
            },
        )
        .collect();
    translated.then_some(next)
}

/// Write receipt of `set_session_workspace_roots` (review #484 round-10 M2):
/// `applied` = agent record committed; `sidecar_committed` = native-code
/// sidecar rewrite receipt — `None` = non-code session, no sidecar needed,
/// `Some(false)` = index committed but the sidecar rewrite failed;
/// `state_committed` = acp-state.json snapshot sync receipt — `None` = no
/// file to sync (native lane / test-constructed store), `Some(false)` =
/// index committed but the snapshot sync failed (the stale snapshot keeps
/// the wider pre-align root set and resurrects it in the over-grant
/// direction on index-loss recovery). Both failure kinds must surface via
/// the align outcome (`sidecar_stale`) rather than stay silent (review
/// #484 round-13 M3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceRootsWrite {
    pub applied: bool,
    pub sidecar_committed: Option<bool>,
    pub state_committed: Option<bool>,
}

impl WorkspaceRootsWrite {
    fn skipped() -> Self {
        Self {
            applied: false,
            sidecar_committed: None,
            state_committed: None,
        }
    }
}

/// 原子写入原生代码会话 sidecar；写入失败逐条记日志并返回 false，不阻断会话
/// 绑定主流程（辅助索引仍然可用，丢失恢复兜底时才依赖 sidecar；缺失的 sidecar
/// 由启动时的 `backfill_missing_code_session_sidecars` 自愈补写）。
fn write_code_session_sidecar(
    store_path: &Path,
    session_id: &str,
    kind: CodexWorkspaceKind,
    workspace_path: Option<PathBuf>,
    workspace_roots: Vec<PathBuf>,
    explicit_ancestor_roots: Vec<PathBuf>,
) -> bool {
    let path = code_session_sidecar_path(store_path, session_id);
    let sidecar = CodeSessionSidecar {
        version: CODE_SESSION_SIDECAR_VERSION,
        workspace_kind: kind,
        workspace_path,
        workspace_roots,
        explicit_ancestor_roots,
        bound_at: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or_default(),
        ),
    };
    match persist_code_session_sidecar(&path, &sidecar) {
        Ok(()) => true,
        Err(error) => {
            eprintln!(
                "[pinvou3-app] 写入原生代码会话 sidecar 失败（{}）: {error:#}",
                path.display()
            );
            false
        }
    }
}

/// Rebind retry variant of [`write_code_session_sidecar`] (review #464
/// follow-up nit): a rebind moves an existing binding rather than creating
/// one, so the original `bound_at` first-bind timestamp is carried over
/// instead of being re-stamped with now. A missing or unreadable sidecar
/// yields None — the timestamp is unknown and must not be fabricated.
fn rewrite_code_session_sidecar_preserving_bound_at(
    store_path: &Path,
    session_id: &str,
    workspace_path: PathBuf,
    workspace_roots: Vec<PathBuf>,
) -> bool {
    let path = code_session_sidecar_path(store_path, session_id);
    let sidecar = CodeSessionSidecar {
        version: CODE_SESSION_SIDECAR_VERSION,
        workspace_kind: CodexWorkspaceKind::Project,
        workspace_path: Some(workspace_path),
        workspace_roots,
        // Round-11 SF6: every non-align keychain rewrite RESETS the
        // provenance (the invariant the index lanes already carry) — a
        // rebind replacement keychain has not been granted any ancestor.
        explicit_ancestor_roots: Vec::new(),
        bound_at: read_code_session_sidecar(store_path, session_id)
            .and_then(|sidecar| sidecar.bound_at),
    };
    match persist_code_session_sidecar(&path, &sidecar) {
        Ok(()) => true,
        Err(error) => {
            // Rebind path: the module's rebind log-hygiene rule applies (the
            // error chain embeds sessions/<id>/ paths), so log the root cause
            // only — never the path or the session id — matching the rebind
            // rewrite loop below. The bind-path writer above is exempt.
            eprintln!(
                "[pinvou3-app] rebind retry rewrite of the native code session sidecar failed: {}",
                error.root_cause()
            );
            false
        }
    }
}

/// 读取原生代码会话 sidecar；不存在、解析失败或 schema 版本高于当前支持版本时
/// 返回 None（按缺失处理，走恢复/回填路径），异常均记日志。
pub(super) fn read_code_session_sidecar(
    store_path: &Path,
    session_id: &str,
) -> Option<CodeSessionSidecar> {
    let path = code_session_sidecar_path(store_path, session_id);
    let payload = match fs::read(&path) {
        Ok(payload) => payload,
        // Round-8 should-fix 6: a sidecar that exists but cannot be READ
        // (permissions, io error) used to disappear silently, so both rebind
        // scans skipped it and the run reported success while the orphan
        // binding stayed at `from`. Make the skip visible; id and path stay
        // out of the log per the rebind log-hygiene rule.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!(
                "[pinvou3-app] rebind scan skips an unreadable code-session sidecar: {}",
                error.kind()
            );
            return None;
        }
    };
    match serde_json::from_slice::<CodeSessionSidecar>(&payload) {
        Ok(sidecar) => {
            // A future-version format must never parse silently as the
            // current one: refuse to read it and treat it as missing; the
            // restore/backfill path rewrites it in the current version. Log
            // hygiene (review #463 round-14 should-fix 5): the path embeds
            // the session id — same rule as the sessions side — so only the
            // version numbers are logged, never the path.
            if sidecar.version > CODE_SESSION_SIDECAR_VERSION {
                eprintln!(
                    "[pinvou3-app] 原生代码会话 sidecar 版本 {} 高于当前支持的 {}，按缺失处理",
                    sidecar.version, CODE_SESSION_SIDECAR_VERSION,
                );
                return None;
            }
            Some(sidecar)
        }
        Err(error) => {
            eprintln!("[pinvou3-app] 解析原生代码会话 sidecar 失败: {error:#}");
            None
        }
    }
}

/// 删除原生代码会话 sidecar（会话删除时调用）。
///
/// Returns whether the sidecar file was actually removed, so callers that
/// count or log the removal stay silent when there was nothing to remove.
pub(super) fn remove_code_session_sidecar(store_path: &Path, session_id: &str) -> bool {
    let sidecar = code_session_sidecar_path(store_path, session_id);
    match fs::remove_file(&sidecar) {
        Ok(()) => {
            // Round-8 should-fix 7: drop the session dir too when the sidecar
            // was its last occupant, so a deleted session leaves no ghost dir
            // for persist's create_dir_all / boot backfill to keep alive.
            // Fails harmlessly while other files remain.
            if let Some(parent) = sidecar.parent() {
                let _ = fs::remove_dir(parent);
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            eprintln!(
                "[pinvou3-app] 清理原生代码会话 sidecar 失败（{}）: {error:#}",
                sidecar.display()
            );
            false
        }
    }
}

#[cfg(test)]
static INJECT_SIDECAR_PERSIST_FAIL_IDS: std::sync::Mutex<Option<Vec<String>>> =
    std::sync::Mutex::new(None);

/// Test-only owner-probe seam (round-24 minor 6): an armed id's owner probe
/// reports dead while the record file is still healthy on disk — the exact
/// state a write pass sees when the owner is deleted between the command
/// layer's scan and the store passes. Unlike a fixture that deletes the
/// record, this leaves every read healthy, so a deleted write-pass gate
/// would COMPLETE the rewrite and fail the pin. Production never arms it.
#[cfg(test)]
static OWNER_PROBE_DEAD_IDS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn persist_code_session_sidecar(path: &Path, sidecar: &CodeSessionSidecar) -> Result<()> {
    // Cross-platform fault injection for the rebind tests (round-22: the
    // atomic_write migration defeated path-occupation injections —
    // randomized temp names cannot be pre-occupied). Test-only, and
    // SESSION-ID-SCOPED so parallel tests never see each other's faults;
    // the id is the sidecar's parent directory name.
    #[cfg(test)]
    if let Some(ids) = INJECT_SIDECAR_PERSIST_FAIL_IDS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
    {
        let sid = path
            .parent()
            .and_then(|dir| dir.file_name())
            .and_then(|name| name.to_str());
        if let Some(sid) = sid {
            if ids.iter().any(|injected| injected == sid) {
                anyhow::bail!("injected sidecar persist failure");
            }
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建会话目录失败: {}", parent.display()))?;
    }
    // Same durability as the index persist (review #463 round-22 minor 4):
    // atomic_write carries the fsync the plain fs::write+rename pair lacked.
    crate::platform::filesystem::atomic_write_private(path, &serde_json::to_vec_pretty(sidecar)?)
        .with_context(|| format!("保存 {} 失败", path.display()))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AgentStoreFile {
    #[serde(default = "store_version")]
    version: u32,
    #[serde(default)]
    sessions: HashMap<String, SessionAgentRecord>,
}

fn store_version() -> u32 {
    STORE_VERSION
}

#[derive(Clone)]
pub struct SessionAgentStore {
    path: PathBuf,
    records: Arc<RwLock<HashMap<String, SessionAgentRecord>>>,
}

/// Outcome of the durable-owner-record probe: does `sessions/<id>.json`
/// still exist? The path derivation and the stat judgment live in exactly
/// one place (`SessionAgentStore::owner_record_state`) so the read side
/// (rebind lanes) and the deletion side (boot prune, backfill gate) cannot
/// drift; each caller maps this state onto its own disposition.
pub(super) enum OwnerRecordState {
    /// The durable record exists as a regular file. (A non-file entry at
    /// the record path counts as absent — the app never creates one, so a
    /// stray directory there is not a durable record.)
    Live,
    /// No durable record: the path is gone, or it is not a regular file.
    NotFound,
    /// The id fails validation, so the record path was not probed at all.
    InvalidId,
    /// A non-NotFound stat error: the record's state is unknowable.
    ProbeError(std::io::ErrorKind),
}

impl SessionAgentStore {
    /// Round-46 M2: the newer-schema refusal every index reader must apply
    /// (load + both reload paths). An unchecked newer index would be
    /// re-persisted stamped with the CURRENT version — a silent schema
    /// downgrade defeating the gate's refusing-to-overwrite purpose.
    fn assert_index_schema(file: &AgentStoreFile) -> Result<()> {
        if file.version > STORE_VERSION {
            anyhow::bail!(
                "session-agents.json uses a newer schema ({} > {}); refusing to overwrite — the index is rebuildable from the per-session sidecars",
                file.version,
                STORE_VERSION
            );
        }
        Ok(())
    }

    pub fn load() -> Result<Self> {
        Self::load_from(crate::platform::paths::pinvou3_home().join("session-agents.json"))
    }

    /// Round-46 M2: load() with an injectable path so the schema gate is
    /// pinnable on this path too (the global home is not test-safe).
    fn load_from(path: std::path::PathBuf) -> Result<Self> {
        let records = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("读取 {} 失败", path.display()))?;
            let file = serde_json::from_str::<AgentStoreFile>(&raw)
                .with_context(|| format!("解析 {} 失败", path.display()))?;
            // Round-41 M4 introduced the gate; round-46 M2 moved it into the
            // shared assertion so the reload paths enforce the same refusal
            // (rationale: pre-PR binaries strip the round-40 fields on
            // whole-table persists; refusing costs a sidecar rebuild).
            Self::assert_index_schema(&file)?;
            file.sessions
        } else {
            HashMap::new()
        };
        Ok(Self {
            path,
            records: Arc::new(RwLock::new(records)),
        })
    }

    /// 外部 Agent ACP 是可选能力，它的辅助索引损坏时不能阻断 Pinvou 主程序启动。
    ///
    /// 加载失败时不主动覆盖原始文件。Round-51 review 更正：自 round-48 起
    /// 所有 mutator 都经 `lock_and_reload` 在锁下重读整个索引，损坏的索引
    /// 会让它们 fail-closed 拒绝（防止 GUI 用解析不了的旧内容覆盖一次 CLI
    /// rebind），而不是本注释旧版所说的「下一次 persist 自愈」——启动时的
    /// restore/backfill 恢复路径持同一把锁，也被同一拒绝挡住。修复方式是
    /// 按错误提示手工删除/修复 `session-agents.json`；本函数仅留给不写索引
    /// 的只读调用方兜底启动。
    pub fn load_or_empty() -> Self {
        match Self::load() {
            Ok(store) => store,
            Err(error) => {
                let path = crate::platform::paths::pinvou3_home().join("session-agents.json");
                eprintln!("[pinvou3-app] ACP session index unavailable, starting empty: {error:#}");
                Self {
                    path,
                    records: Arc::new(RwLock::new(HashMap::new())),
                }
            }
        }
    }

    /// 一个不读盘的空实例，仅供只需要索引路径的调用方使用。Round-45 review：
    /// `sessions delete` 只需要 `path()`，其删除通道
    /// （`remove_for_second_process`）本来就会在锁下重读整个索引；经
    /// `load_or_empty` 取路径会把启动恢复的「starting empty」stderr 行带进
    /// CLI 的失败输出，正好污染用户必须手工修复的那个场景。
    pub fn empty_without_loading() -> Self {
        Self {
            path: crate::platform::paths::pinvou3_home().join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn backend(&self, session_id: &str) -> AgentBackend {
        self.records
            .read()
            .get(session_id)
            .map(|record| record.backend)
            .unwrap_or_default()
    }

    /// 辅助索引文件路径（`session-agents.json`）。sidecar 目录由其派生。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 测试专用：以指定索引路径构造空 store（sidecar 根随之派生，与生产同源）。
    #[cfg(test)]
    pub(crate) fn for_test(path: PathBuf) -> Self {
        Self {
            path,
            records: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn get(&self, session_id: &str) -> SessionAgentRecord {
        self.records
            .read()
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Snapshot of the in-memory index's session ids. `load` is all-or-nothing
    /// (an unparsable file leaves the map empty) and every persist rewrites
    /// the file from this same state, so iterating memory is iterating the
    /// state that would be persisted — the boot prune walks this set instead
    /// of reaching into the on-disk format.
    pub(super) fn session_ids(&self) -> Vec<String> {
        self.records.read().keys().cloned().collect()
    }

    /// Returns session ids bound to the given project workspace for
    /// code-capable sessions: ACP sessions in any mode (they execute file
    /// edits in the project directory even in plain mode) and native sessions
    /// in code mode. Both paths are canonicalized before comparison,
    /// tolerating symlink/trailing-slash differences.
    pub fn code_sessions_in_workspace(&self, root: &Path) -> Vec<String> {
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        self.records
            .read()
            .iter()
            .filter(|(_, record)| {
                if record.workspace_kind != CodexWorkspaceKind::Project {
                    return false;
                }
                if !record.mode.is_code() && !record.backend.is_acp() {
                    return false;
                }
                record
                    .workspace_path
                    .as_deref()
                    .map(|path| {
                        let path =
                            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                        path == canonical
                    })
                    .unwrap_or(false)
            })
            .map(|(session_id, _)| session_id.clone())
            .collect()
    }

    /// ACP session 一旦建立就不允许换 Agent 或目录，避免同一个 Agent 上下文跨
    /// 后端或跨项目漂移。
    pub fn set_acp_workspace(
        &self,
        session_id: &str,
        backend: AgentBackend,
        kind: CodexWorkspaceKind,
        workspace_path: Option<PathBuf>,
        workspace_roots: Vec<PathBuf>,
    ) -> Result<()> {
        if !backend.is_acp() {
            anyhow::bail!("ACP 会话不能绑定非 ACP 后端");
        }
        if kind == CodexWorkspaceKind::Project && workspace_path.is_none() {
            anyhow::bail!("项目会话缺少 ACP 工作目录");
        }
        if kind == CodexWorkspaceKind::Temporary && workspace_path.is_some() {
            anyhow::bail!("临时会话不能保存项目工作目录");
        }
        let _section = self.lock_and_reload()?;
        // Keychain snapshot (§6): project sessions store the full roots locked
        // at creation, normalized before persisting into cwd-first dedup order
        // (review #484 round-5 M2 — the foundation's normalize_workspace_roots
        // consumes cwd-first; persisted order and effective order must match,
        // or the chip mislabels the primary root); temporary sessions are
        // always empty (§9.1: no bound workspace), same as
        // bind_code_native_session's Temporary handling (review #484 m3).
        let workspace_roots = match (kind, workspace_path.as_deref()) {
            (CodexWorkspaceKind::Project, Some(cwd)) => {
                crate::features::sessions::cwd_first_workspace_roots(cwd, workspace_roots)
            }
            _ => Vec::new(),
        };
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            if record.acp_session_id.is_some()
                && (record.backend != backend
                    || record.workspace_kind != kind
                    || record.workspace_path != workspace_path)
            {
                anyhow::bail!("ACP 会话已开始，不能更换 Agent 或工作目录；请新建会话");
            }
            record.backend = backend;
            record.workspace_kind = kind;
            record.workspace_path = workspace_path;
            // A repeated set (calling again on the same session) keeps the
            // latest value, at the same write point as workspace_path.
            record.workspace_roots = workspace_roots;
            // Round-11 SF7: same-shape repeat = non-align rewrite: reset the
            // provenance with the keychain (see the sibling write above).
            record.explicit_ancestor_roots = Vec::new();
            // ACP 会话不是代码模式会话：绑定 ACP 时重置为 plain 模式，
            // 避免 is_code_session() 误判、且 restore 时不会拒绝 ACP 覆盖。
            record.mode = SessionMode::Plain;
        }
        // 先持久化辅助索引，再清理权威 sidecar：persist 失败时 sidecar 仍在，与
        // 磁盘索引保持一致，不会出现「sidecar 已删、索引未更新」的中间态；若 sidecar
        // 清理失败，残留 sidecar 会在下次启动扫描时被识别为 ACP 会话残留并清理
        // （见 mod.rs `restore_code_native_sessions_from_sidecars`）。
        self.persist()?;
        // 该会话不再是原生代码会话：清理权威 sidecar，防止辅助索引重建时误恢复。
        remove_code_session_sidecar(&self.path, session_id);
        Ok(())
    }

    /// 绑定“代码”模块的原生（品悟 Engine）会话。临时会话目录由 session id 推导；
    /// 项目会话保存创建时选定的绝对目录（调用前须经 `validate_codex_project_workspace`
    /// 校验）。与 ACP 会话同样遵循“会话开始后不可换 Agent 或工作目录”。
    pub fn bind_code_native_session(
        &self,
        session_id: &str,
        kind: CodexWorkspaceKind,
        workspace_path: Option<PathBuf>,
        workspace_roots: Vec<PathBuf>,
    ) -> Result<()> {
        if kind == CodexWorkspaceKind::Project && workspace_path.is_none() {
            anyhow::bail!("项目会话缺少工作目录");
        }
        if kind == CodexWorkspaceKind::Temporary && workspace_path.is_some() {
            anyhow::bail!("临时会话不能保存项目工作目录");
        }
        let _section = self.lock_and_reload()?;
        // Keychain snapshot (§6): project sessions store the full roots, with
        // the same pre-persist normalization as set_acp_workspace (cwd-first
        // dedup, review #484 round-5 M2); temporary sessions stay empty
        // (single root).
        let workspace_roots = match (kind, workspace_path.as_deref()) {
            (CodexWorkspaceKind::Project, Some(cwd)) => {
                crate::features::sessions::cwd_first_workspace_roots(cwd, workspace_roots)
            }
            _ => Vec::new(),
        };
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            if record.acp_session_id.is_some() && record.backend.is_acp() {
                anyhow::bail!("ACP 会话已开始，不能更换 Agent；请新建会话");
            }
            // 已绑定的原生代码会话不允许改绑到其他工作区（同值重复绑定幂等放行）。
            if record.mode.is_code()
                && (record.workspace_kind != kind || record.workspace_path != workspace_path)
            {
                anyhow::bail!("代码会话已开始，不能更换工作目录；请新建会话");
            }
            record.backend = AgentBackend::Deepseek;
            record.workspace_kind = kind;
            record.workspace_path = workspace_path.clone();
            record.workspace_roots = workspace_roots;
            // Round-11 SF7: a same-shape repeat (calling again on the same
            // session) is a non-align keychain rewrite — it RESETS the
            // provenance in the same write, honoring the "every non-align
            // keychain rewrite resets the explicit-ancestor provenance"
            // invariant instead of leaving the previous align's grants on a
            // replaced keychain.
            record.explicit_ancestor_roots = Vec::new();
            record.mode = SessionMode::Code;
        }
        self.persist()?;
        // 权威 sidecar：辅助索引损坏/丢失后据此恢复原生代码会话类型与项目绑定。
        // 写失败已逐条记日志；缺失的 sidecar 由启动时回填自愈补写。
        let (record_roots, record_explicit) = {
            let records = self.records.read();
            let record = records.get(session_id);
            (
                record
                    .map(|record| record.workspace_roots.clone())
                    .unwrap_or_default(),
                // Round-11 SF7: a same-shape set_acp_workspace replacement
                // resets the provenance with the keychain — the index lane
                // already asserts every non-align rewrite resets it, and the
                // sidecar must not resurrect what the index just dropped.
                Vec::new(),
            )
        };
        write_code_session_sidecar(
            &self.path,
            session_id,
            kind,
            workspace_path,
            record_roots,
            record_explicit,
        );
        Ok(())
    }

    /// Keychain replacement for "align to project" (§9.7): rewrites the
    /// creation-time snapshot wholesale. This method **deliberately** does
    /// not follow the "permissions only grow" convention — alignment is an
    /// explicit user action, and the snapshot is wholly replaced by the
    /// owning project's current roots, so extra roots may grow or shrink
    /// (removed roots leave the snapshot, voiding their foundation-side
    /// write exemptions); consistency is the caller's (command layer) to
    /// guarantee via the active-turn fence and the SyncSession push. Native
    /// code sessions also rewrite the authoritative sidecar (bound_at
    /// refreshes to now — metadata only, no recovery semantics, matching
    /// write_code_session_sidecar's always-rewrite behavior); ACP sessions
    /// have no sidecar, index only. Returns Ok(false) when the record is
    /// missing or has no bound workspace (temporary session); the caller
    /// then takes the pure-binding lane.
    pub fn set_session_workspace_roots(
        &self,
        session_id: &str,
        workspace_roots: Vec<PathBuf>,
    ) -> Result<WorkspaceRootsWrite> {
        // Every non-align keychain rewrite RESETS the explicit-ancestor
        // provenance: align is the only producer of project-backed
        // ancestors, and a rebind/heal replacement keychain has not been
        // granted any (round-40 zhuowp P2).
        self.set_session_workspace_roots_with_explicit_ancestors(
            session_id,
            workspace_roots,
            Vec::new(),
        )
    }

    /// Round-40 (zhuowp P2): the align lane's writer — records which
    /// entries of the new keychain are explicitly-granted ancestors, so
    /// delivery keeps them after the backing project mutates. The
    /// provenance rolls back with the keychain on a persist failure.
    ///
    /// Round-11 R1 (review #484 round-11): this and the detached sweep are
    /// the last two mutators to join the cross-process section-lock
    /// discipline every sibling runs — the whole-table persist below could
    /// revert a CLI `projects rebind` / `sessions delete` committed from a
    /// second process between this store's boot load and the align's
    /// persist, and a sibling's reload could wipe the align from memory
    /// while it still reports `applied`. Section lock → fresh reload →
    /// mutate → persist, and a corrupt index is REFUSED instead of being
    /// replaced by the stale-table persist.
    pub fn set_session_workspace_roots_with_explicit_ancestors(
        &self,
        session_id: &str,
        workspace_roots: Vec<PathBuf>,
        explicit_ancestor_roots: Vec<PathBuf>,
    ) -> Result<WorkspaceRootsWrite> {
        let _section = self.lock_and_reload()?;
        // Snapshot the current keychain (and provenance) so a persist
        // failure can roll memory back to the on-disk state — the same
        // convention as rebind_workspace_prefix: without the rollback the
        // process believes the align happened while disk says otherwise,
        // and the next read would report roots that were never persisted
        // (review #484 M2).
        let (original_roots, original_explicit) = {
            let mut records = self.records.write();
            let Some(record) = records.get_mut(session_id) else {
                return Ok(WorkspaceRootsWrite::skipped());
            };
            if record.workspace_kind != CodexWorkspaceKind::Project
                || record.workspace_path.is_none()
            {
                return Ok(WorkspaceRootsWrite::skipped());
            }
            let original_roots = record.workspace_roots.clone();
            let original_explicit = record.explicit_ancestor_roots.clone();
            (original_roots, original_explicit)
        };
        self.commit_session_workspace_roots_locked(
            session_id,
            workspace_roots,
            explicit_ancestor_roots,
            original_roots,
            original_explicit,
        )
    }

    /// Round-36 P1: the detached sweep's per-target writer — derives the
    /// replacement keychain from the record AS RELOADED UNDER THE SECTION
    /// LOCK, not from a pre-lock scan. The old shape computed `next` before
    /// `set_session_workspace_roots` took the lock, so a root a peer added
    /// to the SAME session between the scan and the locked write was
    /// silently dropped by the stale-derived replacement. The stationary
    /// binding filter is likewise re-evaluated on the fresh record.
    pub fn retranslate_detached_roots_for(
        &self,
        session_id: &str,
        from: &Path,
        to: &Path,
    ) -> Result<WorkspaceRootsWrite> {
        let _section = self.lock_and_reload()?;
        let (original_roots, original_explicit, next) = {
            let records = self.records.read();
            let Some(record) = records.get(session_id) else {
                return Ok(WorkspaceRootsWrite::skipped());
            };
            if record.workspace_kind != CodexWorkspaceKind::Project
                || record.workspace_path.is_none()
            {
                return Ok(WorkspaceRootsWrite::skipped());
            }
            // Stationary binding only, re-judged on the fresh record: the
            // binding-moved population belongs to the main lanes.
            let binding_moved = record.workspace_path.as_deref().is_some_and(|path| {
                crate::platform::os::path_relative_suffix_under(path, from).is_some()
            });
            if binding_moved {
                return Ok(WorkspaceRootsWrite::skipped());
            }
            let Some(next) = translate_roots_under(&record.workspace_roots, from, to) else {
                return Ok(WorkspaceRootsWrite::skipped());
            };
            (
                record.workspace_roots.clone(),
                record.explicit_ancestor_roots.clone(),
                next,
            )
        };
        self.commit_session_workspace_roots_locked(
            session_id,
            next,
            // A rebind replacement keychain carries no grants (SF6).
            Vec::new(),
            original_roots,
            original_explicit,
        )
    }

    /// The shared commit tail of the two keychain writers above — runs with
    /// the caller's section guard still held: mutate-rollback persist, the
    /// native sidecar rewrite (with provenance), and the acp-state
    /// convergence patch.
    fn commit_session_workspace_roots_locked(
        &self,
        session_id: &str,
        workspace_roots: Vec<PathBuf>,
        explicit_ancestor_roots: Vec<PathBuf>,
        original_roots: Vec<PathBuf>,
        original_explicit: Vec<PathBuf>,
    ) -> Result<WorkspaceRootsWrite> {
        // The mutation step (the callers snapshot the originals they pass
        // in): apply the new keychain under the write lock, then the
        // persist-rollback tail below.
        {
            let mut records = self.records.write();
            if let Some(record) = records.get_mut(session_id) {
                record.workspace_roots = workspace_roots.clone();
                record.explicit_ancestor_roots = explicit_ancestor_roots.clone();
            }
        }
        if let Err(error) = self.persist() {
            if let Some(record) = self.records.write().get_mut(session_id) {
                record.workspace_roots = original_roots;
                record.explicit_ancestor_roots = original_explicit;
            }
            return Err(error);
        }
        let record = self.get(session_id);
        let state_workspace_path = if record.workspace_kind == CodexWorkspaceKind::Project {
            record.workspace_path.clone()
        } else {
            None
        };
        let sidecar_committed = if record.mode.is_code() {
            Some(write_code_session_sidecar(
                &self.path,
                session_id,
                record.workspace_kind,
                record.workspace_path,
                workspace_roots.clone(),
                // Round-11 SF2: the sidecar is the durable truth an index
                // loss rebuilds from — persisting the roots without the
                // grant provenance empties every recorded ancestor in
                // exactly that recovery window.
                explicit_ancestor_roots.clone(),
            ))
        } else {
            None
        };
        // The per-session acp-state.json is the only durable ACP artifact that
        // survives a session-agents.json loss (review #484 round-11 M8): keep
        // its roots snapshot convergent with the index, or a later index loss
        // lets the recovery path resurrect the pre-align (wider) root set.
        // Failure here cannot roll the index back (it is already committed),
        // so it is reported through `state_committed` and surfaces as
        // `sidecar_stale` on the align outcome — swallowing it would report a
        // narrowing align as clean while the durable ACP copy keeps the wider
        // pre-align set (review #484 round-13 M3). Sync without creating:
        // acp-state.json creation belongs to the spawn path; sessions without
        // the file (native lane, test-constructed stores) skip it — recovery
        // cannot read it anyway. The next respawn rewrites the file wholesale.
        let mut state_committed: Option<bool> = None;
        if record.workspace_kind == CodexWorkspaceKind::Project {
            if let Some(workspace_path) = &state_workspace_path {
                // Round-26 minor 7: an Err from state_path (an invalid id
                // shape) used to fall through with state_committed = None —
                // silently unknown. Report an honest convergence failure
                // instead: Some(false) surfaces as sidecar_stale.
                match super::events::state_path(session_id) {
                    Ok(state_path) if state_path.exists() => {
                        // Round-34 minor 10: normalize the patch payload the
                        // same way the index write does (cwd-first, §6) — the
                        // raw argument was persisted into acp-state verbatim
                        // while the index normalized, a latent order mismatch
                        // for a future non-normalizing caller.
                        // `workspace_path` is already &PathBuf (the Some arm
                        // of `state_workspace_path`) — normalize through it
                        // directly.
                        let normalized_roots = crate::features::sessions::cwd_first_workspace_roots(
                            workspace_path,
                            workspace_roots.to_vec(),
                        );
                        // Round-37 M6: the ancestors ride the SAME patch —
                        // acp-state.json is the one durable ACP artifact
                        // that survives an index loss, and without the
                        // field here the recovery record below rebuilds
                        // with empty provenance, silently revoking every
                        // align-granted ancestor in exactly that window
                        // (the native lane's round-11 SF2 closed its twin).
                        let normalized_ancestors: Vec<PathBuf> = explicit_ancestor_roots
                            .iter()
                            .filter(|root| normalized_roots.contains(root))
                            .cloned()
                            .collect();
                        let patch = json!({
                            "workspace": {
                                "kind": record.workspace_kind,
                                "path": workspace_path,
                                "roots": normalized_roots,
                                "explicit_ancestor_roots": normalized_ancestors,
                            }
                        });
                        state_committed = Some(
                            match super::events::patch_acp_state_with_outcome(session_id, patch) {
                                // Round-20 SF1: a corrupt file reset by the
                                // patch counts as a convergence FAILURE —
                                // the durable artifact lost its
                                // adapter/session sections, and reporting it
                                // as committed would toast plain success.
                                Ok(super::events::AcpStatePatchOutcome::Clean) => true,
                                Ok(super::events::AcpStatePatchOutcome::ResetCorrupt) => false,
                                Err(error) => {
                                    eprintln!(
                                        "[codex-acp] acp-state keychain convergence failed: {error:#}"
                                    );
                                    false
                                }
                            },
                        );
                    }
                    Ok(_) => {}
                    // Round-26 minor 7: the id failed the path shape check —
                    // report an honest convergence failure, not a silent
                    // unknown (None).
                    Err(error) => {
                        eprintln!(
                            "[codex-acp] acp-state path unusable; convergence reported failed: {error}"
                        );
                        state_committed = Some(false);
                    }
                }
            }
        }
        Ok(WorkspaceRootsWrite {
            applied: true,
            sidecar_committed,
            state_committed,
        })
    }

    /// Keychain snapshot locked at session creation (§6): the full accessible
    /// roots (including the primary root); old records / temporary sessions /
    /// no record = empty (single-root semantics; caller normalizes by cwd).
    pub fn session_workspace_roots(&self, session_id: &str) -> Vec<PathBuf> {
        self.records
            .read()
            .get(session_id)
            .map(|record| record.workspace_roots.clone())
            .unwrap_or_default()
    }

    /// Round-40 M6: both keychain fields from ONE record read — the
    /// compose lane used to take the records lock twice per session per
    /// refresh.
    pub fn session_workspace_keychain(&self, session_id: &str) -> (Vec<PathBuf>, Vec<PathBuf>) {
        self.records
            .read()
            .get(session_id)
            .map(|record| {
                (
                    record.workspace_roots.clone(),
                    record.explicit_ancestor_roots.clone(),
                )
            })
            .unwrap_or_default()
    }

    /// Round-40 (zhuowp P2): the recorded explicit-ancestor provenance for
    /// this keychain (empty = none). Same read shape as
    /// [`Self::session_workspace_roots`].
    pub fn session_explicit_ancestor_roots(&self, session_id: &str) -> Vec<PathBuf> {
        self.records
            .read()
            .get(session_id)
            .map(|record| record.explicit_ancestor_roots.clone())
            .unwrap_or_default()
    }

    /// 该会话的产品模式（plain/code）；无记录时按 plain 缺省
    /// （与历史 `is_code_session` 缺省 false 等价）。
    pub fn session_mode(&self, session_id: &str) -> SessionMode {
        self.records
            .read()
            .get(session_id)
            .map(|record| record.mode)
            .unwrap_or_default()
    }

    /// 是否为“代码”模块的原生（品悟 Engine）会话。
    pub fn is_code_session(&self, session_id: &str) -> bool {
        self.session_mode(session_id).is_code()
    }

    /// 原生代码会话绑定的项目目录；非代码会话或临时会话返回 None。
    /// 这是“两个根”的唯一判定入口：执行根（engine/shell）命中它时解析到项目目录，
    /// 账本根（附件/审计/产物）命中它时必须改用会话私有目录。
    pub fn code_project_workspace(&self, session_id: &str) -> Option<PathBuf> {
        let record = self.get(session_id);
        if record.mode.is_code() && record.workspace_kind == CodexWorkspaceKind::Project {
            record.workspace_path
        } else {
            None
        }
    }

    /// The workspace path recorded by the session's code-session SIDECAR (the
    /// authoritative store for native code sessions), None when the sidecar
    /// is absent, unreadable, or not a project binding. The rebind's
    /// stranded-index detector compares the index against this directly
    /// (review #463 round-14 R1): for an index-arm scan hit the surfaced
    /// path IS the index path, so comparing against it can never detect an
    /// index/sidecar divergence.
    pub fn code_sidecar_workspace(&self, session_id: &str) -> Option<PathBuf> {
        let sidecar = read_code_session_sidecar(&self.path, session_id)?;
        if sidecar.workspace_kind != CodexWorkspaceKind::Project {
            return None;
        }
        sidecar.workspace_path
    }

    /// The native sidecar's recorded keychain roots (round-26 MAJOR 2):
    /// `None` = no readable project sidecar. The align retry path compares
    /// this against the target shape — the index alone cannot detect a
    /// previously failed sidecar rewrite (the advertised retry convergence
    /// would stay dead code).
    pub fn code_sidecar_workspace_roots(&self, session_id: &str) -> Option<Vec<PathBuf>> {
        let sidecar = read_code_session_sidecar(&self.path, session_id)?;
        if sidecar.workspace_kind != CodexWorkspaceKind::Project {
            return None;
        }
        Some(sidecar.workspace_roots)
    }

    /// Round-29 M2: translate `from`-prefixed KEYCHAIN roots of
    /// project-shaped records whose BINDING is stationary. The main rebind
    /// lanes translate only sessions whose binding moved (the candidate set
    /// is binding-prefix matched); a session bound at cwd `P` holding a
    /// shared `/from/extra` root used to keep the vanished spelling forever
    /// — compose soft-kept it, delivery re-persisted it verbatim, and the
    /// report was silent. This sweep rewrites each affected record's roots
    /// through [`Self::set_session_workspace_roots`] (which also rewrites
    /// the native sidecar and converges the ACP acp-state copy) and returns
    /// the touched session ids for the report. `from`/`to` are the command
    /// layer's display forms.
    ///
    /// Domain residual (round-30 minor 4, lane-consistent with the main
    /// migration): the suffix match folds case/separators only, so on
    /// alias-divergent platforms a soft-kept lexical spelling (`/var/...`)
    /// whose `from` resolves to `/private/var/...` escapes this sweep
    /// exactly as it escapes the main keychain lanes.
    pub fn rebind_detached_workspace_roots(
        &self,
        from: &Path,
        to: &Path,
    ) -> (Vec<String>, Vec<String>) {
        // Round-36 P1: the scan is only a pre-filter now — the replacement
        // set and the stationary-binding check are both derived INSIDE
        // retranslate_detached_roots_for's section lock, from the record as
        // reloaded there, so a root a peer added to the same session between
        // this scan and the locked write is translated (not dropped by a
        // stale-derived replacement).
        let targets: Vec<String> = {
            let records = self.records.read();
            records
                .iter()
                .filter(|(_, record)| record.workspace_kind == CodexWorkspaceKind::Project)
                .filter(|(_, record)| {
                    let binding_moved = record.workspace_path.as_deref().is_some_and(|path| {
                        crate::platform::os::path_relative_suffix_under(path, from).is_some()
                    });
                    !binding_moved
                        && record.workspace_roots.iter().any(|root| {
                            crate::platform::os::path_relative_suffix_under(root, from).is_some()
                        })
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        let mut touched = Vec::new();
        let mut failed = Vec::new();
        for session_id in targets {
            // Round-30 MAJOR 2: owner gate (review #463 round-15 SF-B), the
            // contract every main lane enforces — a retention purge deletes
            // the SavedSession record and session directory without touching
            // this index; sweeping the ghost rewrote it, reported the dead
            // id as Rebound (firing workspace_rebound for it), and the
            // sidecar rewrite's create_dir_all resurrected the purged
            // session directory.
            if !self.binding_owner_exists(&session_id) {
                continue;
            }
            match self.retranslate_detached_roots_for(&session_id, from, to) {
                Ok(write) if write.applied => {
                    // Round-31 MAJOR 2: a committed index with a failed
                    // durable receipt (sidecar/acp-state Some(false)) is the
                    // exact partial-failure signal align surfaces as
                    // sidecar_stale — reporting it as Rebound (with the
                    // event) overclaims exactly the way the plain twin was
                    // fixed not to.
                    if write.sidecar_committed == Some(false)
                        || write.state_committed == Some(false)
                    {
                        failed.push(session_id);
                    } else {
                        touched.push(session_id);
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    // Log hygiene: the error chain carries session paths —
                    // log only the root-cause class.
                    eprintln!(
                        "[codex-acp] detached keychain-root translation failed: {}",
                        error.root_cause()
                    );
                    failed.push(session_id);
                }
            }
        }
        (touched, failed)
    }

    /// The acp-state.json durable keychain roots (round-29 M1): the ACP
    /// lane's stale-delivery artifact, read for the align retry's stale
    /// probe exactly like the native lane reads its sidecar. `None` = no
    /// state file / unreadable / no workspace.roots array.
    pub fn acp_state_workspace_roots(&self, session_id: &str) -> Option<Vec<PathBuf>> {
        super::events::acp_state_workspace_roots(session_id)
    }

    /// Index records still bound under `from`, i.e. a rewrite that did not
    /// stick. Used by the post-pass fence of
    /// [`Self::rebind_workspace_prefix`] (review #463 round-8 minor 8) and
    /// extracted so the rule is unit-testable without racing a real concurrent
    /// writer.
    fn records_still_under_workspace(
        records: &HashMap<String, SessionAgentRecord>,
        from: &Path,
    ) -> Vec<String> {
        records
            .iter()
            .filter(|(_, record)| record.workspace_kind == CodexWorkspaceKind::Project)
            .filter_map(|(session_id, record)| {
                let path = record.workspace_path.as_ref()?;
                Self::rebind_relative_suffix(path, from)?;
                Some(session_id.clone())
            })
            .collect()
    }

    /// Folded-key prefix match + literal suffix: matching goes through the
    /// shared `filesystem_path_identity_key` fold (Windows folds separators
    /// and case, so a case-only rename no longer misses) and the component
    /// predicate is the shared
    /// `platform::os::path_identity_is_same_or_nested` (review #463 round-8
    /// elegance), while the suffix is cut from the original path by component
    /// count, preserving the subdirectory's original casing. Returns `None` =
    /// not under `from`; an empty suffix = the path IS `from`.
    fn rebind_relative_suffix(path: &Path, from: &Path) -> Option<PathBuf> {
        // Single shared suffix cut (round-8 review should-fix 9).
        crate::platform::os::path_relative_suffix_under(path, from)
    }

    /// Lists project sessions bound under the `from` prefix (candidate set for
    /// directory rebind; `from` has usually vanished from disk, so matching is
    /// a folded-key prefix match, not a canonicalize comparison). Besides the
    /// helper index this also scans the code-session sidecar directory: with a
    /// damaged/lost index every native code session is an off-index orphan,
    /// and an index-only scan would leave the rebind fence and the metadata
    /// replay collectively blind (review #463 M6). The index record wins for a
    /// shared session_id; orphans only fill the difference set.
    /// Round-35 M2: the CHECKED twin for the expulsion/delete command lanes —
    /// the in-memory scan stays lossy (an index read cannot fail here), but
    /// the off-index orphan walk bubbles its read failures so the command
    /// aborts before anything moves (mirroring the plain lane's
    /// `try_workspace_bindings_under_any` contract).
    pub fn try_sessions_under_workspace(
        &self,
        from: &Path,
    ) -> std::io::Result<Vec<(String, PathBuf)>> {
        let mut matched: Vec<(String, PathBuf)> = self
            .records
            .read()
            .iter()
            .filter_map(|(session_id, record)| {
                if record.workspace_kind != CodexWorkspaceKind::Project {
                    return None;
                }
                let path = record.workspace_path.as_ref()?;
                Self::rebind_relative_suffix(path, from).map(|_| (session_id.clone(), path.clone()))
            })
            .collect();
        for session_id in try_code_session_dir_ids(&self.path, "expel candidate scan")? {
            if matched.iter().any(|(sid, _)| *sid == session_id) {
                continue;
            }
            let Some(sidecar) = read_code_session_sidecar(&self.path, &session_id) else {
                continue;
            };
            if sidecar.workspace_kind != CodexWorkspaceKind::Project {
                continue;
            }
            let Some(path) = sidecar.workspace_path else {
                continue;
            };
            if Self::rebind_relative_suffix(&path, from).is_some() {
                matched.push((session_id, path));
            }
        }
        // Round-37 M3: the same dead-owner retain the lossy twin and the
        // plain lane's checked twin enforce (round-463 SF-B: "dead ids are
        // never reported") — a retention-purged session whose index record
        // survives must not land in the expulsion list as a permanent
        // tombstone + verbatim CLI report entry.
        let matched = matched
            .into_iter()
            .filter(|(session_id, _)| self.binding_owner_exists(session_id))
            .collect();
        Ok(matched)
    }

    pub fn sessions_under_workspace(&self, from: &Path) -> Vec<(String, PathBuf)> {
        let mut matched: Vec<(String, PathBuf)> = self
            .records
            .read()
            .iter()
            .filter_map(|(session_id, record)| {
                if record.workspace_kind != CodexWorkspaceKind::Project {
                    return None;
                }
                let path = record.workspace_path.as_ref()?;
                Self::rebind_relative_suffix(path, from).map(|_| (session_id.clone(), path.clone()))
            })
            .collect();
        // Off-index orphan sidecars: startup restore treats the sidecar as
        // authoritative, so the candidate set must use the same scope (same
        // scan shape as the sidecar rewrite pass of rebind_workspace_prefix).
        for session_id in code_session_dir_ids(&self.path, "rebind candidate scan") {
            if matched.iter().any(|(sid, _)| *sid == session_id) {
                continue;
            }
            let Some(sidecar) = read_code_session_sidecar(&self.path, &session_id) else {
                continue;
            };
            if sidecar.workspace_kind != CodexWorkspaceKind::Project {
                continue;
            }
            let Some(path) = sidecar.workspace_path else {
                continue;
            };
            if Self::rebind_relative_suffix(&path, from).is_some() {
                matched.push((session_id, path));
            }
        }
        // Owner gate (review #463 round-15 SF-B): retention deletes purge the
        // SavedSession record and session directory without touching this
        // index, so an index record can outlive its owner — rewriting it and
        // reporting the dead id as Rebound violates the "dead ids are never
        // reported" contract. The plain lane gates identically.
        matched.retain(|(session_id, _)| self.binding_owner_exists(session_id));
        matched
    }

    /// Computes the rebind target for a candidate binding path: a path under
    /// the `from` prefix moves its suffix onto `to`; a path already under
    /// `to` (a retry candidate translated by a previous run whose metadata
    /// was not synced) is returned unchanged. Neither matching returns None
    /// (cannot be a candidate).
    pub fn rebind_target_path(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
        if let Some(suffix) = Self::rebind_relative_suffix(path, from) {
            return Some(if suffix.as_os_str().is_empty() {
                to.to_path_buf()
            } else {
                to.join(suffix)
            });
        }
        Self::rebind_relative_suffix(path, to).map(|_| path.to_path_buf())
    }

    /// Whether the session still owns a durable SavedSession record
    /// (`<id>.json` beside the session directories). Retention deletes purge
    /// the record and the session directory WITHOUT touching this index
    /// (review #463 round-15 SF-B), so an index record here can outlive its
    /// owner; a binding without an owner is inert — the plain lane's
    /// `workspace_binding_owner_exists` refuses exactly this shape, and the
    /// rebind scan/write passes gate on it the same way so a dead session is
    /// never rewritten and never classified Rebound.
    pub fn binding_owner_exists(&self, session_id: &str) -> bool {
        match self.owner_record_state(session_id) {
            OwnerRecordState::Live => true,
            // Round-24 minor 2 (was round-20 minor 10 + round-21 SF-3):
            // a non-NotFound stat error is treated as LIVE and logged.
            // The conservative direction here is inclusion, not death: a
            // transient EACCES blip must not silently drop a live
            // session from the scan, both write passes, the fence and
            // the post-lane rescan while the run reports success. A
            // session included on a stale probe whose reads then fail is
            // reported failed (retryable) by the passes themselves.
            OwnerRecordState::ProbeError(kind) => {
                eprintln!(
                    "[codex] rebind owner probe failed, treating the session as live (io kind: {kind})"
                );
                true
            }
            // Read-side disposition: an invalid id owns nothing.
            OwnerRecordState::NotFound | OwnerRecordState::InvalidId => false,
        }
    }

    /// The shared owner-record probe (see `OwnerRecordState`): one path
    /// derivation and one stat judgment for every caller. The test seam
    /// (`OWNER_PROBE_DEAD_IDS`) applies to every consumer by construction.
    pub(super) fn owner_record_state(&self, session_id: &str) -> OwnerRecordState {
        #[cfg(test)]
        if let Ok(armed) = OWNER_PROBE_DEAD_IDS.lock() {
            if armed.iter().any(|sid| sid == session_id) {
                return OwnerRecordState::NotFound;
            }
        }
        if crate::features::sessions::validate_session_id(session_id).is_err() {
            return OwnerRecordState::InvalidId;
        }
        match code_session_sidecar_root(&self.path)
            .join(format!("{session_id}.json"))
            .metadata()
        {
            Ok(meta) if meta.is_file() => OwnerRecordState::Live,
            Ok(_) => OwnerRecordState::NotFound,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                OwnerRecordState::NotFound
            }
            Err(error) => OwnerRecordState::ProbeError(error.kind()),
        }
    }

    /// Directory rebind (broken-link repair): translates every project
    /// session bound under the `from` prefix onto `to`. Unlike
    /// `set_acp_workspace`/`bind_code_native_session` ("no directory change
    /// after a session starts"), this method bypasses that constraint — the
    /// command layer calls it only when the directory is gone (or the user
    /// explicitly confirmed while it still exists), and the command layer
    /// owns the active-turn fence and target validation.
    ///
    /// Two of the three consistency points are persisted here: the helper
    /// index (session-agents.json) and the authoritative sidecars
    /// (code-session.json, including off-index orphans — startup scan
    /// restores from sidecars, so a missed rewrite resurrects the old
    /// value); the third, SavedSession metadata, is written by the command
    /// layer via SessionStore. Idempotent: a from→to rerun with no match is
    /// a no-op.
    ///
    /// Storage-form invariant (review #463 m1): `from`/`to` must be in the
    /// same form as the bindings stored at bind time (for existing
    /// directories the bind entry already canonicalized them). The command
    /// layer normalizes `from` once at the entry (review #463 B1), so an
    /// alias spelling (macOS `/tmp` vs `/private/tmp`) cannot half-rewrite.
    ///
    /// Returns the affected (session_id, new path) pairs and the finally
    /// stale sidecar list (orphan rewrite failure, or indexed session whose
    /// rewrite AND retry pass both failed — no self-healing path remains in
    /// this run; the command layer should count them into
    /// failed_session_ids, and a rerun converges via the on-disk prefix
    /// scan; review #463 m2 / Major 2).
    pub fn rebind_workspace_prefix(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<RebindWorkspacePrefixOutcome> {
        if from == to {
            return Ok(RebindWorkspacePrefixOutcome::default());
        }
        // Round-41 review: `projects rebind` reaches this lane from a second
        // process (the CLI), so the index rewrite carries the same discipline
        // as `remove_for_second_process`: cross-process section lock → fresh
        // reload → mutate → persist. Without it, this whole-table persist
        // writes the store's BOOT-ERA snapshot: a GUI session bind landing
        // while the rebind runs is silently dropped from the index (ACP
        // sessions keep no workspace sidecar after bind, so nothing restores
        // it — the run exits 0 and the loss is undisclosed), and a corrupt
        // index is replaced by a (near-)empty table instead of being
        // refused. The lock is held across the sidecar passes too — they
        // consult the reloaded index memory through `binding_owner_exists`.
        // Round-42 review: the residual is now closed from the other side —
        // every GUI mutator that whole-table-persists runs the same
        // lock-and-reload discipline (see `lock_and_reload`), so no surface
        // persists a stale boot-era table over a peer's committed write.
        let _section = self.lock_and_reload()?;
        let mut affected: Vec<(String, PathBuf)> = Vec::new();
        let mut sidecar_final_stale: Vec<String> = Vec::new();
        // Original bindings of the mutated records, kept so a persist
        // failure can roll memory back to the on-disk state (review #463
        // minor: unconditional persist() + advanced memory on failure left
        // the process believing the rebind happened while disk said
        // otherwise).
        // Round-41 minor 1: the rollback tuple carries the provenance too —
        // the mutation loop resets it (SF6), so the rollback must restore it
        // or memory loses it while disk retains it until the next reload.
        let mut originals: Vec<(String, Option<PathBuf>, Vec<PathBuf>, Vec<PathBuf>)> = Vec::new();
        {
            let mut records = self.records.write();
            for (session_id, record) in records.iter_mut() {
                if record.workspace_kind != CodexWorkspaceKind::Project {
                    continue;
                }
                let Some(path) = record.workspace_path.clone() else {
                    continue;
                };
                let Some(suffix) = Self::rebind_relative_suffix(&path, from) else {
                    continue;
                };
                // Owner re-check (review #463 round-15 SF-B, mirrors the
                // plain lane's round-14 M2): retention deletes do not touch
                // this index, so a record can outlive its session — a dead
                // owner is never rewritten (and never reported as moved).
                if !self.binding_owner_exists(session_id) {
                    continue;
                }
                let next = if suffix.as_os_str().is_empty() {
                    to.to_path_buf()
                } else {
                    to.join(suffix)
                };
                originals.push((
                    session_id.clone(),
                    record.workspace_path.clone(),
                    record.workspace_roots.clone(),
                    record.explicit_ancestor_roots.clone(),
                ));
                record.workspace_path = Some(next.clone());
                // The keychain snapshot migrates with the binding (§7): roots
                // under the `from` prefix shift onto `to`, roots outside it
                // are untouched. Stale `/from/...` entries would otherwise be
                // normalized into additional writable roots outside `to` on
                // the next resume (review #484 M3).
                //
                // Round-11 SF6: the rebind replacement keychain RESETS the
                // explicit-ancestor provenance — align is the only producer
                // of grants and a rebind has not been granted any, so leaving
                // the `from`-spelled entries is a trap for the next reader
                // (the plain lane and the detached wrapper already reset).
                record.explicit_ancestor_roots = Vec::new();
                for root in record.workspace_roots.iter_mut() {
                    if let Some(root_suffix) = Self::rebind_relative_suffix(root, from) {
                        *root = if root_suffix.as_os_str().is_empty() {
                            to.to_path_buf()
                        } else {
                            to.join(root_suffix)
                        };
                    }
                }
                affected.push((session_id.clone(), next));
            }
        }
        if let Err(error) = self.persist() {
            // Roll the in-memory bindings back to the persisted state: the
            // caller reports the rebind as failed, and a retry must see the
            // original `from` bindings (not ones memory pretends moved).
            // The keychain translation is rolled back with the path.
            let mut records = self.records.write();
            for (session_id, original_path, original_roots, original_explicit) in originals {
                if let Some(record) = records.get_mut(&session_id) {
                    record.workspace_path = original_path;
                    record.workspace_roots = original_roots;
                    record.explicit_ancestor_roots = original_explicit;
                }
            }
            return Err(error);
        }
        // Sidecar rewrite: the authoritative binding of native code
        // sessions. ACP sessions have no sidecar (cleared at bind time);
        // off-index orphan sidecars are rewritten too — otherwise a restart
        // restore resurrects the old directory. A write failure is logged
        // and NOT marked rewritten (the retry pass below retries it); it is
        // never silently counted as success.
        let mut sidecar_rewritten: Vec<String> = Vec::new();
        for session_id in code_session_dir_ids(&self.path, "rebind sidecar rewrite") {
            // Same owner gate as the index rewrite above (round-15 SF-B): a
            // sidecar whose session record is gone belongs to a deleted
            // session; rewriting it would resurrect a dead id into the
            // report.
            if !self.binding_owner_exists(&session_id) {
                continue;
            }
            let Some(sidecar) = read_code_session_sidecar(&self.path, &session_id) else {
                continue;
            };
            if sidecar.workspace_kind != CodexWorkspaceKind::Project {
                continue;
            }
            let Some(path) = sidecar.workspace_path.as_ref() else {
                continue;
            };
            let Some(suffix) = Self::rebind_relative_suffix(path, from) else {
                continue;
            };
            let next = if suffix.as_os_str().is_empty() {
                to.to_path_buf()
            } else {
                to.join(suffix)
            };
            // The orphan sidecar's keychain migrates with its binding too:
            // roots under the `from` prefix shift onto `to`, roots outside it
            // are untouched. Preserving the snapshot verbatim would strand
            // `/from/...` entries (usually including the old cwd) that the
            // foundation normalizes into additional writable roots outside
            // `to` on resume (review #484 M3).
            let rebound_roots: Vec<PathBuf> = sidecar
                .workspace_roots
                .iter()
                .map(|root| match Self::rebind_relative_suffix(root, from) {
                    Some(root_suffix) if root_suffix.as_os_str().is_empty() => to.to_path_buf(),
                    Some(root_suffix) => to.join(root_suffix),
                    None => root.clone(),
                })
                .collect();
            match persist_code_session_sidecar(
                &code_session_sidecar_path(&self.path, &session_id),
                &CodeSessionSidecar {
                    version: CODE_SESSION_SIDECAR_VERSION,
                    workspace_kind: CodexWorkspaceKind::Project,
                    workspace_path: Some(next.clone()),
                    workspace_roots: rebound_roots,
                    // Round-11 SF6: non-align rewrite — reset the provenance.
                    explicit_ancestor_roots: Vec::new(),
                    bound_at: sidecar.bound_at,
                },
            ) {
                Err(error) => {
                    // The old sidecar is still on disk; a restart restore
                    // would resurrect the old directory. Indexed sessions
                    // get retried by the rewrite pass below; orphans have
                    // no retry path — both go into the finally-stale list
                    // (an indexed session leaves the list if the retry
                    // succeeds) for the command layer to count as failed
                    // (review #463 m2/Major 2). Failed sessions reach the
                    // frontend via sidecar_final_stale; the log carries
                    // only the root cause, never a plaintext session id —
                    // the error chain embeds sessions/<id>/ paths (CodeQL
                    // cleartext-logging, review #463 round 7).
                    eprintln!(
                        "[pinvou3-app] rebind rewrite of the native code session sidecar failed: {}",
                        error.root_cause()
                    );
                    sidecar_final_stale.push(session_id.clone());
                }
                Ok(()) => {
                    sidecar_rewritten.push(session_id.clone());
                    // Orphan with a missing index record: only a
                    // successful write counts as affected — the sidecar
                    // IS its binding, and claiming a failed one moved to
                    // `next` would be a false report (review #463 nit).
                    // Indexed sessions are already in `affected`.
                    if !affected.iter().any(|(sid, _)| *sid == session_id) {
                        affected.push((session_id, next));
                    }
                }
            }
        }
        // Rewrite-retry pass for indexed native code sessions already
        // rebound. Sessions rewritten above are skipped: that pass already
        // carried the original bound_at over, and rewriting again would be a
        // double write (review #463 minor). The retry writer preserves the
        // original bound_at too (review #464 follow-up nit) — a rebind is a
        // move, not a new bind. A failed retry = finally stale: backfill
        // self-healing only rewrites *missing* sidecars, and boot restore
        // skips sidecars while the index is intact, so nothing would ever
        // rewrite this stale record — it stays in the list for the command
        // layer to count as failed; a successful retry removes it (review
        // #463 Major 2).
        {
            let records = self.records.read();
            for (session_id, path) in &affected {
                if sidecar_rewritten.iter().any(|sid| sid == session_id) {
                    continue;
                }
                if records
                    .get(session_id)
                    .is_some_and(|record| record.mode.is_code())
                {
                    // The keychain snapshot rides along with the sidecar
                    // rewrite: the record pass above already translated the
                    // roots in memory, so re-reading them here persists the
                    // rebound set. The retry writer also carries the original
                    // first-bind timestamp over (the bind-time writer would
                    // re-stamp it).
                    let record_roots = records
                        .get(session_id)
                        .map(|record| record.workspace_roots.clone())
                        .unwrap_or_default();
                    if rewrite_code_session_sidecar_preserving_bound_at(
                        &self.path,
                        session_id,
                        path.clone(),
                        record_roots,
                    ) {
                        sidecar_final_stale.retain(|sid| sid != session_id);
                    } else if !sidecar_final_stale.iter().any(|sid| sid == session_id) {
                        sidecar_final_stale.push(session_id.clone());
                    }
                }
            }
        }
        // Divergence repair (review #463 round-8 minor 9). The index and the
        // sidecar are both written on bind and are meant to agree, so a
        // disagreement means an earlier run was interrupted between its write
        // phases. Without this, a re-keyed rerun (from→to delivered the index
        // but both sidecar passes failed, then from→to2 moves only the
        // sidecar and the metadata) strands the index record at `to`, matching
        // neither lane of the second run — no path ever converges it. The
        // sidecar is the authoritative binding, so the disagreeing record is
        // re-keyed onto the same target the sidecar just moved to.
        let mut index_rekeys: Vec<(String, PathBuf)> = Vec::new();
        {
            let records = self.records.read();
            for (session_id, path) in &affected {
                let Some(record) = records.get(session_id) else {
                    continue;
                };
                if !record.mode.is_code() || record.workspace_kind != CodexWorkspaceKind::Project {
                    continue;
                }
                // `affected` already holds the translated target for this run;
                // an index record that agrees with it needs no repair.
                if record.workspace_path.as_ref() == Some(path) {
                    continue;
                }
                index_rekeys.push((session_id.clone(), path.clone()));
            }
        }
        if !index_rekeys.is_empty() {
            self.commit_index_rekeys(&index_rekeys)?;
        }
        // Post-pass fence (review #463 round-8 minor 8). The rebind gate
        // serializes rebinds only, so a concurrent bind/remove can land in the
        // unlocked sidecar-IO window and clobber a rewritten sidecar back to
        // `from` — which this run would otherwise report as success. Re-reading
        // the index catches exactly that (and any other late writer): a record
        // still bound under `from` after the whole rewrite is finally stale, so
        // the command layer counts the session as failed and a rerun converges
        // it via the on-disk prefix scan. This fence is a point-in-time read;
        // the window between it and the command's return is covered by the
        // command layer's symmetric post-lane rescan into `final_stale`
        // (review #463 round-15 disclosure gap).
        {
            let records = self.records.read();
            for session_id in Self::records_still_under_workspace(&records, from) {
                // The write passes skip dead owners by design (round-15
                // SF-B); the fence must not flag them as "still under from"
                // or a deleted session would be reported failed.
                if !self.binding_owner_exists(&session_id) {
                    continue;
                }
                if !sidecar_final_stale.iter().any(|sid| sid == &session_id) {
                    sidecar_final_stale.push(session_id);
                }
            }
        }
        Ok(RebindWorkspacePrefixOutcome {
            affected,
            sidecar_final_stale,
        })
    }

    /// Commits a batch of index re-keys with the rollback contract of the
    /// index move above (review #463 round-10 Major 1, extracted so the
    /// divergence repair inside `rebind_workspace_prefix` and the
    /// command-driven stranded-index repair share one implementation): memory
    /// must never claim a repair disk does not have, so a persist failure
    /// rolls every mutated record back to its pre-call binding and errors —
    /// the caller reports this run as failed. Convergence on the rerun is
    /// command-driven, not lane-driven: neither `from` nor `to` prefix scan
    /// reaches a record stranded at an intermediate target, so the rerun's
    /// to-lane admission must surface the sidecar and call
    /// [`Self::repair_stranded_index_records`] again.
    fn commit_index_rekeys(&self, rekeys: &[(String, PathBuf)]) -> Result<()> {
        let mut originals: Vec<(String, Option<PathBuf>)> = Vec::with_capacity(rekeys.len());
        {
            let mut records = self.records.write();
            for (session_id, path) in rekeys {
                if let Some(record) = records.get_mut(session_id) {
                    originals.push((
                        session_id.clone(),
                        record.workspace_path.replace(path.clone()),
                    ));
                }
            }
        }
        // Success log only AFTER the persist that can roll the re-keys back
        // (review #463 round-19 SF-9): announcing success before the write
        // made the rollback path contradict the log. Count only: the log
        // must not carry plaintext session ids (CodeQL cleartext-logging,
        // review #463 round 7).
        if let Err(error) = self.persist() {
            let mut records = self.records.write();
            for (session_id, original) in originals {
                if let Some(record) = records.get_mut(&session_id) {
                    record.workspace_path = original;
                }
            }
            return Err(error);
        }
        eprintln!(
            "[pinvou3-app] rebind repaired {} index/sidecar disagreement(s)",
            rekeys.len()
        );
        Ok(())
    }

    /// Re-keys index records that survived a divergence-repair persist
    /// failure onto the sidecar's authoritative target (review #463 round-10
    /// Major 1). The stranded shape: run 1 (from→to) delivered the index but
    /// both sidecar passes failed; run 2 (from→to2) moved the sidecar and
    /// then failed to persist the re-key — index@`to`, sidecar@`to2`. Run 3
    /// (from→to2) matches neither prefix in either store pass, so the lane
    /// alone reports an empty success while the index record still points at
    /// the vanished `to` and boot restore (which skips sidecars while the
    /// index holds a code record) keeps resurrecting it. The command layer
    /// detects the strand via its to-lane scan — the sidecar surfaces the
    /// session, the index disagrees — and drives this repair; it must run
    /// BEFORE the metadata loop so a persist failure leaves the metadata
    /// stale and the rerun re-admits (and re-repairs) the session.
    ///
    /// Records already sitting on their target are skipped (idempotent); the
    /// returned list holds only the actually re-keyed session ids.
    pub fn repair_stranded_index_records(
        &self,
        targets: &[(String, PathBuf)],
    ) -> Result<Vec<String>> {
        // Round-42 review: same whole-table-persist hazard as the other GUI
        // mutators — the repair runs right after `rebind_workspace_prefix`
        // released the section lock, so a concurrent writer in that window
        // would be reverted by this whole-table persist. Lock + fresh reload
        // covers the read phase and the `commit_index_rekeys` write (which
        // stays lock-free for its in-rebind caller).
        let _section = self.lock_and_reload()?;
        let mut rekeys = Vec::new();
        {
            let records = self.records.read();
            for (session_id, path) in targets {
                if records
                    .get(session_id)
                    .is_some_and(|record| record.workspace_path.as_ref() != Some(path))
                {
                    rekeys.push((session_id.clone(), path.clone()));
                }
            }
        }
        if rekeys.is_empty() {
            return Ok(Vec::new());
        }
        let repaired = rekeys.iter().map(|(sid, _)| sid.clone()).collect();
        self.commit_index_rekeys(&rekeys)?;
        Ok(repaired)
    }

    pub fn set_acp_session(
        &self,
        session_id: &str,
        acp_session_id: String,
        model_id: Option<String>,
        config_values: HashMap<String, String>,
    ) -> Result<()> {
        let _section = self.lock_and_reload()?;
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            record.acp_session_id = Some(acp_session_id);
            record.acp_model_id = model_id
                .clone()
                .or_else(|| config_values.get("model").cloned());
            record.acp_mode_id = config_values.get("mode").cloned();
            record.acp_config_values = config_values;
            if let Some(model_id) = model_id {
                record
                    .acp_config_values
                    .insert("model".to_string(), model_id);
            }
        }
        self.persist()
    }

    pub fn set_acp_config_value(
        &self,
        session_id: &str,
        config_id: &str,
        value_id: &str,
    ) -> Result<()> {
        let _section = self.lock_and_reload()?;
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            record
                .acp_config_values
                .insert(config_id.to_string(), value_id.to_string());
            if config_id == "model" {
                record.acp_model_id = Some(value_id.to_string());
            } else if config_id == "mode" {
                record.acp_mode_id = Some(value_id.to_string());
            }
        }
        self.persist()
    }

    pub fn clear_acp_config_value(&self, session_id: &str, config_id: &str) -> Result<()> {
        let _section = self.lock_and_reload()?;
        {
            let mut records = self.records.write();
            let Some(record) = records.get_mut(session_id) else {
                return Ok(());
            };
            record.acp_config_values.remove(config_id);
            if config_id == "model" {
                record.acp_model_id = None;
            } else if config_id == "mode" {
                record.acp_mode_id = None;
            }
        }
        self.persist()
    }

    pub(super) fn restore_missing_acp_record(
        &self,
        session_id: &str,
        recovered: SessionAgentRecord,
        source_holds_recovery: impl Fn() -> bool,
    ) -> Result<bool> {
        if !recovered.backend.is_acp()
            || recovered
                .acp_session_id
                .as_deref()
                .is_none_or(str::is_empty)
        {
            anyhow::bail!("恢复的 ACP 会话索引不完整");
        }
        let _section = self.lock_and_reload()?;
        {
            let mut records = self.records.write();
            if records
                .get(session_id)
                .is_some_and(|record| record.backend.is_acp())
            {
                return Ok(false);
            }
            // Round-44 review: the recovery record was read outside the
            // section lock, and a CLI `sessions delete` landing in the
            // window removes the session (and its acp-state.json) before
            // this insert. Re-verify the source under the lock so the boot
            // pass cannot mint an orphan ACP record for a deleted session.
            if !source_holds_recovery() {
                return Ok(false);
            }
            records.insert(session_id.to_string(), recovered);
        }
        self.persist()?;
        Ok(true)
    }

    pub fn remove(&self, session_id: &str) -> Result<()> {
        let _section = self.lock_and_reload()?;
        // Most purged sessions (plain chats) have no index entry; without
        // this short-circuit every such purge would rewrite the whole index
        // file for nothing, serialized inside the purge flow. It must sit
        // AFTER `lock_and_reload`: only the fresh disk table proves durable
        // absence. An in-memory miss on a boot-era snapshot proves nothing
        // — a prior call whose persist failed leaves the entry on disk
        // while memory already dropped it, and short-circuiting on that
        // state would make the removal silently unretryable.
        if self.records.write().remove(session_id).is_none() {
            return Ok(());
        }
        self.persist()?;
        // 删除会话时同步清理权威 sidecar，避免残留的 sidecar 让重建后的索引
        // 误恢复一个已删除的原生代码会话。
        remove_code_session_sidecar(&self.path, session_id);
        Ok(())
    }

    /// Round-42 review: every whole-table persist must observe the disk
    /// truth under the cross-process section lock. The persist rewrites the
    /// entire `session-agents.json` from memory, so a mutator running on a
    /// boot-era snapshot silently reverts another process's committed write
    /// (a routine GUI config write erasing the CLI `projects rebind` index,
    /// or a GUI bind resurrecting a CLI-deleted record). Same discipline as
    /// the round-37/40 reviews closed for the providers store and
    /// [`Self::remove_for_second_process`]: section lock → fresh reload →
    /// mutate → persist, and a corrupt index is REFUSED instead of being
    /// replaced by a stale-table persist. Returns the section guard — the
    /// caller mutates `self.records` and calls `self.persist()` while it is
    /// held. `rebind_workspace_prefix` takes the lock + fresh reload FIRST
    /// through this helper and only then translates the paths, and
    /// [`Self::commit_index_rekeys`] stays lock-free because its other
    /// caller already holds the lock.
    fn lock_and_reload(&self) -> Result<Option<std::fs::File>> {
        let lock_path = self.path.with_extension("json.lock");
        let section = super::cross_process_section_lock(&lock_path, "session-agents");
        if self.path.exists() {
            let raw = std::fs::read_to_string(&self.path)
                .with_context(|| format!("读取 {} 失败", self.path.display()))?;
            let file = serde_json::from_str::<AgentStoreFile>(&raw)
                .with_context(|| format!("解析 {} 失败", self.path.display()))?;
            // Round-46 M2: reload paths adopt-and-persist under the CURRENT
            // version stamp — a newer-schema file slipping through here is
            // downgraded silently on the caller's next persist.
            Self::assert_index_schema(&file)?;
            let fresh = file.sessions;
            let mut records = self.records.write();
            *records = fresh;
        }
        Ok(section)
    }

    /// Second-process removal for the CLI's `sessions delete`
    /// (round-40 review). The GUI lane above is always the boot-owner of
    /// this store; the CLI is not, and the `load_or_empty` → `remove` →
    /// `persist` sequence it used had two loss windows: a GUI session
    /// creation landing between the CLI's load and persist was dropped
    /// from the index by the whole-table persist, and an index that turned
    /// corrupt was loaded as an EMPTY map whose persist threw away every
    /// remaining record. Same discipline as the round-37 review closed for
    /// `AcpConfigDefaultsStore`: cross-process section lock → fresh reload
    /// → mutate → persist, and a corrupt index is REFUSED instead of being
    /// replaced by an empty-table persist. A missing index returns
    /// `Ok(())` without creating the file: `sessions delete` must not be
    /// the thing that creates `session-agents.json` in a home that never
    /// ran an ACP session.
    pub fn remove_for_second_process(&self, session_id: &str) -> Result<()> {
        if !self.path.exists() {
            return Ok(());
        }
        let lock_path = self.path.with_extension("json.lock");
        let _section = super::cross_process_section_lock(&lock_path, "session-agents");
        // Round-44 review: this mutator exists for the second process (the
        // CLI), so its failure chain must be CLI-English — a zh context here
        // surfaces verbatim through `pinvou sessions delete` output.
        let raw = fs::read_to_string(&self.path).with_context(|| {
            format!(
                "failed to read the shared session-agent index {}: it stays untouched",
                self.path.display()
            )
        })?;
        let file = serde_json::from_str::<AgentStoreFile>(&raw).with_context(|| {
            format!(
                "failed to parse the shared session-agent index {}: it stays untouched",
                self.path.display()
            )
        })?;
        // Round-46 M2: same schema refusal as load/lock_and_reload — the CLI
        // lane rewrites the whole table, so a newer-schema file must stop
        // here instead of being re-persisted under the current stamp.
        Self::assert_index_schema(&file)?;
        let mut fresh = file.sessions;
        fresh.remove(session_id);
        {
            let mut records = self.records.write();
            *records = fresh;
        }
        self.persist()?;
        // 同 `remove`：删除后清理权威 sidecar（幂等）。
        remove_code_session_sidecar(&self.path, session_id);
        Ok(())
    }

    /// 回填缺失的原生代码会话 sidecar（启动自愈）。
    ///
    /// 两类来源：sidecar 持久化修复前构建创建的存量会话从未写过 sidecar；绑定时
    /// sidecar 写失败只记日志未补写。索引记录 `code_session=true` 而 sidecar 缺失
    /// 时按索引补写；返回成功补写的数量（写失败已逐条记日志，不计入）。
    ///
    /// Ghost gate: index entries whose durable record `<id>.json` was already
    /// deleted by retention are skipped as dead instead of being backfilled —
    /// otherwise every boot would mass-"resurrect" long-evicted sessions as
    /// dirs containing only `code-session.json`. This gate only skips and
    /// counts; removing the entry itself belongs to the boot scan's prune.
    /// The probe keeps `binding_owner_exists`'s conservative semantics: only
    /// a real NotFound counts as dead, so a transient stat error never keeps
    /// a live session from being backfilled.
    pub fn backfill_missing_code_session_sidecars(&self) -> Result<usize> {
        // Round-43 review: the boot-era snapshot must not decide which
        // sidecars are "missing" — a CLI `sessions delete` landing after
        // this process booted removes the record AND its sidecar, and
        // backfilling from the stale table would recreate it for a
        // deleted session, which the next boot's restore pass then turns
        // back into an index record. Same section discipline as every
        // other boot-pass mutator; a corrupt index refuses (fail-closed)
        // instead of backfilling against an unreadable table.
        let _section = self.lock_and_reload()?;
        let records = self.records.read().clone();
        let mut backfilled = 0usize;
        let mut skipped_dead = 0usize;
        for (session_id, record) in records {
            if !record.mode.is_code() {
                continue;
            }
            if read_code_session_sidecar(&self.path, &session_id).is_some() {
                continue;
            }
            // Ghost gate: an entry whose durable record was deleted by
            // retention gets no backfilled sidecar — that would resurrect a
            // ghost dir under sessions/<id>/; removing the entry itself
            // belongs to the boot scan's prune.
            if !self.binding_owner_exists(&session_id) {
                skipped_dead += 1;
                continue;
            }
            if write_code_session_sidecar(
                &self.path,
                &session_id,
                record.workspace_kind,
                record.workspace_path,
                record.workspace_roots,
                record.explicit_ancestor_roots,
            ) {
                backfilled += 1;
                eprintln!("[pinvou3-app] 回填原生代码会话 sidecar: {session_id}");
            }
        }
        if skipped_dead > 0 {
            eprintln!(
                "[pinvou3-app] backfill skipped {skipped_dead} native code session entries without a live owner record"
            );
        }
        Ok(backfilled)
    }

    /// 从权威 sidecar 恢复原生代码会话记录（辅助索引缺失/损坏时的兜底）。
    ///
    /// 与 ACP 的 `restore_missing_acp_record` 对称：sidecar 是长期权威
    /// 依据，辅助索引只负责加速。恢复成功即持久化回 `session-agents.json`，
    /// 使后续读取不再依赖 sidecar。返回是否真实发生了恢复：索引已持有
    /// code 模式记录时返回 `Ok(false)`，调用方不得把它计入恢复信号。
    ///
    /// Round-44 review: the caller's enumeration snapshot is only a
    /// pre-filter (a missing sidecar at scan time skips the call entirely);
    /// the record is rebuilt from the sidecar bytes re-read under the
    /// section lock, so both stale directions of the enumeration close — a
    /// sidecar removed by a peer `sessions delete`, and a sidecar rewritten
    /// onto a new root by a peer `projects rebind`.
    pub fn restore_missing_code_session_record(&self, session_id: &str) -> Result<bool> {
        // Round-43 review: this is the last whole-table-persisting mutator
        // outside the section discipline (its ACP twin
        // `restore_missing_acp_record` already runs it). Without the
        // lock-and-reload, the GUI boot pass persists its boot-era table
        // and silently reverts a CLI `sessions delete` / `projects rebind`
        // that committed between this process's `load_or_empty` and this
        // recovery — resurrecting the deleted record. Taking the lock here
        // also moves the code/ACP checks onto the fresh table, so they
        // decide on disk truth instead of the boot snapshot.
        let _section = self.lock_and_reload()?;
        // The caller enumerated sidecars outside the section lock, so the
        // snapshot can be stale in BOTH directions: a concurrent
        // `sessions delete` removes the sidecar (restoring from the
        // enumeration copy would re-create exactly the record the delete
        // erased), and a concurrent `projects rebind` REWRITES the sidecar
        // onto the new root (restoring from the enumeration copy would
        // re-insert the pre-rebind binding the rebind just moved). Re-read
        // the authoritative sidecar under the lock and rebuild from it.
        let Some(recovered) = read_code_session_sidecar(&self.path, session_id) else {
            return Ok(false);
        };
        let record = SessionAgentRecord {
            backend: AgentBackend::Deepseek,
            workspace_kind: recovered.workspace_kind,
            workspace_path: recovered.workspace_path,
            mode: SessionMode::Code,
            // The keychain rides in the sidecar (review #484 round-3 minor):
            // dropping it here turns a lost index into a silent keychain
            // loss — the session would resume single-root after a crash.
            workspace_roots: recovered.workspace_roots,
            // Round-11 SF2: the align-granted ancestor provenance rides in
            // the sidecar too — rebuilding without it silently empties every
            // recorded grant in the exact recovery window it exists for.
            explicit_ancestor_roots: recovered.explicit_ancestor_roots,
            ..Default::default()
        };
        if record.workspace_kind == CodexWorkspaceKind::Project && record.workspace_path.is_none() {
            anyhow::bail!("恢复的原生代码会话缺少项目工作目录");
        }
        if record.workspace_kind == CodexWorkspaceKind::Temporary && record.workspace_path.is_some()
        {
            anyhow::bail!("恢复的原生代码会话临时目录不应保存项目工作目录");
        }
        {
            let mut records = self.records.write();
            if records
                .get(session_id)
                .is_some_and(|record| record.mode.is_code())
            {
                return Ok(false);
            }
            if records
                .get(session_id)
                .is_some_and(|record| record.backend.is_acp())
            {
                anyhow::bail!("会话已是 ACP 会话，拒绝用原生代码会话 sidecar 覆盖");
            }
            records.insert(session_id.to_string(), record);
        }
        self.persist()?;
        Ok(true)
    }

    fn persist(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let value = AgentStoreFile {
            version: STORE_VERSION,
            sessions: self.records.read().clone(),
        };
        // atomic_write, like every other rebind lane (review #463 round-21
        // SF-7): the plain fs::write+rename pair skipped the fsync, so a
        // power loss could durably commit roots@to (badge gone) while
        // reverting the index@from against sidecars@to — a state no in-app
        // healer reaches.
        crate::platform::filesystem::atomic_write_private(
            &self.path,
            &serde_json::to_vec_pretty(&value)?,
        )?;
        Ok(())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AcpConfigDefaultsFile {
    #[serde(default = "config_defaults_version")]
    version: u32,
    #[serde(default)]
    agents: HashMap<AgentBackend, HashMap<String, String>>,
}

fn config_defaults_version() -> u32 {
    CONFIG_DEFAULTS_VERSION
}

/// 用户为每个 ACP Agent 选择的新会话默认配置。
///
/// ACP 只定义 session 级配置，不负责跨 session 持久化；Pinvou 作为 client 将
/// 用户成功应用过的配置按 Agent 隔离保存，并在新建 session 后重新应用。
#[derive(Clone)]
pub struct AcpConfigDefaultsStore {
    path: PathBuf,
    records: Arc<RwLock<HashMap<AgentBackend, HashMap<String, String>>>>,
}

impl AcpConfigDefaultsStore {
    pub fn load() -> Result<Self> {
        let path = crate::platform::paths::pinvou3_home().join("acp-agent-defaults.json");
        let records = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("读取 {} 失败", path.display()))?;
            serde_json::from_str::<AcpConfigDefaultsFile>(&raw)
                .with_context(|| format!("解析 {} 失败", path.display()))?
                .agents
        } else {
            HashMap::new()
        };
        Ok(Self {
            path,
            records: Arc::new(RwLock::new(records)),
        })
    }

    /// 默认值文件损坏不能阻断 Agent 启动；保留原文件，下一次用户成功修改配置时
    /// 会重新生成可用内容。
    pub fn load_or_empty() -> Self {
        match Self::load() {
            Ok(store) => store,
            Err(error) => {
                let path = crate::platform::paths::pinvou3_home().join("acp-agent-defaults.json");
                eprintln!(
                    "[pinvou3-app] ACP agent defaults unavailable, starting empty: {error:#}"
                );
                Self {
                    path,
                    records: Arc::new(RwLock::new(HashMap::new())),
                }
            }
        }
    }

    pub fn get(&self, backend: AgentBackend) -> HashMap<String, String> {
        self.records
            .read()
            .get(&backend)
            .cloned()
            .unwrap_or_default()
    }

    /// Reload-backed read for the session-spawn path (round-40 review).
    /// `get` answers from the boot-era memory map; concurrent in-process
    /// writers (user config changes persisted from blocking workers, the
    /// boot migration's `set_all_if_absent`) rewrite
    /// `acp-agent-defaults.json` while this process is live, so a session
    /// spawned after such a write must not start from the STALE boot-era
    /// default. The reload reuses [`Self::reload_into`]'s in-memory
    /// semantics WITHOUT the section lock: a read needs no exclusion
    /// because the writers persist by atomic rename, so a concurrent
    /// reader sees the old or the new file whole, never torn — worst case
    /// it answers the pre-write snapshot, which is exactly the staleness
    /// it exists to remove.
    pub fn get_after_reload(&self, backend: AgentBackend) -> HashMap<String, String> {
        {
            let mut records = self.records.write();
            Self::reload_into(&mut records, &self.path);
        }
        self.get(backend)
    }

    pub fn has_backend(&self, backend: AgentBackend) -> bool {
        self.records.read().contains_key(&backend)
    }

    pub fn set(&self, backend: AgentBackend, config_id: &str, value_id: &str) -> Result<()> {
        if !backend.is_acp() {
            anyhow::bail!("不能为非 ACP Agent 保存配置默认值");
        }
        // Round-37 review MAJOR: concurrent mutators of this store (user
        // config changes on blocking workers, the boot migration) must not
        // persist the boot-era snapshot over already-committed writes. Same
        // discipline as the providers store: section lock → reload →
        // mutate → persist.
        let _section = self.section_lock();
        {
            let mut records = self.records.write();
            Self::reload_into(&mut records, &self.path);
            records
                .entry(backend)
                .or_default()
                .insert(config_id.to_string(), value_id.to_string());
            // Round-54 audit A4: persist INSIDE the write guard. A
            // get_after_reload acquiring the guard in the release-to-persist
            // window used to reload the disk map over the not-yet-persisted
            // insert, and this fn's persist then wrote the stomped map — the
            // saved default silently lost in memory AND on disk (the
            // reader's own answer was fine; the side effect was the loss).
            // The providers store closed the same shape with persist_locked;
            // now a reader either orders before the mutate or after the
            // persist.
            Self::persist_locked(&records, &self.path)?;
        }
        #[cfg(test)]
        set_persist_park_point();
        Ok(())
    }

    pub fn set_all_if_absent(
        &self,
        backend: AgentBackend,
        values: HashMap<String, String>,
    ) -> Result<bool> {
        if !backend.is_acp() || values.is_empty() {
            return Ok(false);
        }
        // Round-37 review MAJOR: same discipline as `set` — the boot
        // migration can run while a user config change mutates, and a
        // persist of the boot-era snapshot would revert it.
        let _section = self.section_lock();
        {
            let mut records = self.records.write();
            Self::reload_into(&mut records, &self.path);
            if records.contains_key(&backend) {
                return Ok(false);
            }
            records.insert(backend, values);
            // Round-54 audit A4: persist under the write guard — same
            // lost-update window as `set` (see its comment).
            Self::persist_locked(&records, &self.path)?;
        }
        Ok(true)
    }

    /// Section lock (OS flock) over the reload→mutate→persist window, same
    /// shape as the providers store's (round-37 review MAJOR): every
    /// mutator takes it, so a whole-table persist of a boot-era snapshot
    /// cannot silently revert a concurrent writer's committed rows.
    fn section_lock(&self) -> Option<std::fs::File> {
        let lock_path = self.path.with_extension("json.lock");
        super::cross_process_section_lock(&lock_path, "acp-agent-defaults")
    }

    /// Best-effort reload of the disk state into an ALREADY-HELD write
    /// guard, matching the providers store's `reload_into` semantics: an
    /// unreadable/unparsable file keeps the in-memory state (and a file
    /// that turned corrupt after boot is quarantined once, `.invalid`
    /// sibling, so the next persist cannot silently destroy the peer's
    /// only copy).
    fn reload_into(records: &mut HashMap<AgentBackend, HashMap<String, String>>, path: &Path) {
        let Ok(raw) = fs::read_to_string(path) else {
            return;
        };
        let Ok(file) = serde_json::from_str::<AcpConfigDefaultsFile>(&raw) else {
            let mut name = path.file_name().unwrap_or_default().to_os_string();
            name.push(".invalid");
            let quarantine = path.with_file_name(name);
            if !quarantine.exists() {
                let _ = fs::copy(path, &quarantine);
            }
            return;
        };
        *records = file.agents;
    }

    // Round-1 review m3: the read-snapshot `persist()` wrapper that survived
    // the A4 fix had ZERO remaining callers (both mutators persist under
    // the write guard via `persist_locked`) — deleted rather than kept,
    // because a future caller reaching for it would silently reintroduce
    // the exact release-then-persist lost-update shape A4 closed.

    /// Writes the defaults file from an ALREADY-CAPTURED map: `set` /
    /// `set_all_if_absent` pass the write-guarded records so the persist
    /// lands inside the same critical section as the mutate (round-54
    /// audit A4). This is the store's only persist path.
    fn persist_locked(
        records: &HashMap<AgentBackend, HashMap<String, String>>,
        path: &Path,
    ) -> Result<()> {
        // `atomic_write` stages inside the target's parent, so the directory
        // must exist first (unchanged from the hand-rolled writer).
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let value = AcpConfigDefaultsFile {
            version: CONFIG_DEFAULTS_VERSION,
            agents: records.clone(),
        };
        // Round-51 review: the hand-rolled pid-suffixed tmp+rename carried
        // no fsync, so power loss could land a truncated/empty
        // `acp-agent-defaults.json`. `atomic_write` provides the unique
        // per-writer staging name this comment already demanded (pid +
        // timestamp — the shared rule that keeps concurrent writers, here
        // the store's own in-process mutators, from renaming each other's
        // half-written file into place), the fsync-before-rename the
        // sibling stores migrated for, and the same two-arm tmp cleanup.
        crate::platform::filesystem::atomic_write(path, &serde_json::to_vec_pretty(&value)?)?;
        Ok(())
    }
}

pub fn validate_codex_project_workspace(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        anyhow::bail!("请选择项目目录");
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Codex 项目目录不存在或不可访问: {}", path.display()))?;
    if !canonical.is_dir() {
        anyhow::bail!("Codex 工作目录必须是文件夹: {}", canonical.display());
    }
    // Windows 的 canonicalize 返回 \\?\ 前缀的 verbatim 路径；ACP agent（如 kimi acp）的
    // 工作目录校验不识别该形式，会把一切相对路径误判为“工作目录之外”而拒绝读取。
    // 统一归一化为常规盘符路径（非 Windows 平台为恒等映射）。
    Ok(crate::platform::os::platform_compat_path(
        &canonical.to_string_lossy(),
    ))
}

#[cfg(test)]
pub(crate) mod set_persist_park {
    //! Test-only seam for the lost-update regression test: `set` parks at
    //! the point after its critical section so the test can run
    //! `get_after_reload` in exactly the window the round-54 audit's
    //! lost-update interleave needs (with the fix, the persist has already
    //! run inside the guard by the time the park signals).

    use std::sync::mpsc::Sender;
    use std::sync::{Condvar, Mutex};

    pub static ARMED: Mutex<Option<Sender<()>>> = Mutex::new(None);
    pub static RELEASE: Mutex<bool> = Mutex::new(false);
    pub static RELEASE_CV: Condvar = Condvar::new();
}

#[cfg(test)]
fn set_persist_park_point() {
    if let Some(tx) = set_persist_park::ARMED.lock().unwrap().take() {
        let _ = tx.send(());
        let mut released = set_persist_park::RELEASE.lock().unwrap();
        while !*released {
            released = set_persist_park::RELEASE_CV.wait(released).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rebind scan/write passes gate on a durable SavedSession record
    /// (review #463 round-15 SF-B); store-layer tests create it directly.
    fn touch_owner_record(store_path: &Path, session_id: &str) {
        let record = code_session_sidecar_root(store_path).join(format!("{session_id}.json"));
        fs::write(record, "{}").unwrap();
    }

    /// Temp directory plus a fresh empty [`SessionAgentStore`] over
    /// `session-agents.json` inside it — one call per store instance in the
    /// lock-and-reload tests below. The directory keeps the module's
    /// existing `pinvou3-<tag>-{pid}` on-disk naming shape (the trailing
    /// `-` stays in `tag`) and is recreated fresh per call so parallel
    /// tests never share a directory. Same shape as the providers test
    /// module's `section_test_dir` + `tmp_store`.
    fn store_fixture(tag: &str) -> (PathBuf, SessionAgentStore) {
        let root = std::env::temp_dir().join(format!("{tag}{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = store_at(&root.join("session-agents.json"));
        (root, store)
    }

    /// A fresh empty store over `path` — the second-process stand-in for
    /// tests that need two independent in-memory tables over the same
    /// on-disk index (a second `store_fixture` call would wipe the
    /// directory the first store already committed to).
    fn store_at(path: &Path) -> SessionAgentStore {
        SessionAgentStore {
            path: path.to_path_buf(),
            records: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Round-54 audit A4: `set` must persist INSIDE the write guard. A
    /// `get_after_reload` running in the guard-release-to-persist window
    /// reloads the disk map over the not-yet-persisted insert and the
    /// subsequent persist writes the stomped map — the saved default is
    /// silently lost in memory and on disk. The park seam makes the
    /// interleave deterministic: the worker signals at the seam, the main
    /// thread reloads, and the value must survive both the reload and the
    /// eventual persist.
    #[test]
    fn set_survives_concurrent_get_after_reload() {
        let root = std::env::temp_dir().join(format!(
            "pinvou-acp-defaults-lost-update-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let path = root.join("acp-agent-defaults.json");
        let store = AcpConfigDefaultsStore {
            path: path.clone(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let worker_store = AcpConfigDefaultsStore {
            path: path.clone(),
            records: Arc::clone(&store.records),
        };

        // Seed the disk first: reload_into is a no-op while the file is
        // absent, so the audited stomp (a reload REPLACING the memory map)
        // needs an existing file with sibling content to restore.
        store
            .set(AgentBackend::CodexAcp, "config-0", "model-0")
            .expect("seed set must succeed");

        let (tx, rx) = std::sync::mpsc::channel();
        *set_persist_park::ARMED.lock().unwrap() = Some(tx);
        let handle = std::thread::spawn(move || {
            worker_store
                .set(AgentBackend::CodexAcp, "config-a", "model-b")
                .expect("set under the park must succeed");
        });
        rx.recv_timeout(std::time::Duration::from_secs(30))
            .expect("worker reached the park point");
        let seen = store.get_after_reload(AgentBackend::CodexAcp);
        assert_eq!(
            seen.get("config-a").map(String::as_str),
            Some("model-b"),
            "get_after_reload in the park window must observe the concurrent set"
        );
        *set_persist_park::RELEASE.lock().unwrap() = true;
        set_persist_park::RELEASE_CV.notify_all();
        handle.join().unwrap();
        // And the disk truth carries it after everything settles.
        let reread = store.get_after_reload(AgentBackend::CodexAcp);
        assert_eq!(reread.get("config-a").map(String::as_str), Some("model-b"));
        // Reset the seam so a future second park test arms from a clean
        // RELEASE (round-1 review nit: the static is never reset otherwise).
        *set_persist_park::RELEASE.lock().unwrap() = false;
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn backend_aliases_are_stable() {
        assert_eq!(AgentBackend::parse(None).unwrap(), AgentBackend::Deepseek);
        assert_eq!(
            AgentBackend::parse(Some("pinvou")).unwrap(),
            AgentBackend::Deepseek
        );
        assert_eq!(
            AgentBackend::parse(Some("codex")).unwrap(),
            AgentBackend::CodexAcp
        );
        assert_eq!(AgentBackend::CodexAcp.as_str(), "codex-acp");
        assert_eq!(
            AgentBackend::parse(Some("claude")).unwrap(),
            AgentBackend::ClaudeAcp
        );
        assert_eq!(
            AgentBackend::parse(Some("kimi-acp")).unwrap(),
            AgentBackend::KimiAcp
        );
        assert!(AgentBackend::ClaudeAcp.is_acp());
    }

    #[test]
    fn empty_store_defaults_to_deepseek() {
        let store = SessionAgentStore {
            path: PathBuf::from("/tmp/unused-session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        assert_eq!(store.backend("missing"), AgentBackend::Deepseek);
    }

    #[test]
    fn rebind_retry_sidecar_write_preserves_first_bind_timestamp() {
        // review #464 follow-up nit: write_code_session_sidecar stamps
        // bound_at with now (it is the bind-time writer); the rebind retry
        // pass must carry the original timestamp over instead of resetting it.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-agent-store-rebind-bound-at-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // The sidecar root derives as <store_path parent>/sessions, so pass
        // the conventional session-agents.json path INSIDE the nonce'd dir —
        // passing the dir itself would write into the shared, non-nonce'd
        // $TMPDIR/sessions/ (litter plus a cross-process flake window) and
        // the cleanup below would remove only the empty nonce'd dir.
        let store_path = dir.join("session-agents.json");

        persist_code_session_sidecar(
            &code_session_sidecar_path(&store_path, "s1"),
            &CodeSessionSidecar {
                version: CODE_SESSION_SIDECAR_VERSION,
                workspace_kind: CodexWorkspaceKind::Project,
                workspace_path: Some(PathBuf::from("/old/root")),
                workspace_roots: Vec::new(),
                explicit_ancestor_roots: Vec::new(),
                bound_at: Some(42),
            },
        )
        .unwrap();
        // Union pin (#484 rebase): the retry writer persists the translated
        // keychain alongside the moved path while preserving the timestamp.
        assert!(rewrite_code_session_sidecar_preserving_bound_at(
            &store_path,
            "s1",
            PathBuf::from("/new/root"),
            vec![PathBuf::from("/new/keep")]
        ));
        let sidecar = read_code_session_sidecar(&store_path, "s1").unwrap();
        assert_eq!(
            sidecar.workspace_path.as_deref(),
            Some(std::path::Path::new("/new/root"))
        );
        assert_eq!(
            sidecar.workspace_roots,
            vec![PathBuf::from("/new/keep")],
            "the retry pass persists the translated keychain"
        );
        assert_eq!(
            sidecar.bound_at,
            Some(42),
            "the retry pass preserves the first-bind timestamp"
        );

        // No prior sidecar: the timestamp stays unknown instead of being
        // fabricated with now.
        assert!(rewrite_code_session_sidecar_preserving_bound_at(
            &store_path,
            "s2",
            PathBuf::from("/new/root"),
            Vec::new()
        ));
        assert_eq!(
            read_code_session_sidecar(&store_path, "s2")
                .unwrap()
                .bound_at,
            None
        );
        // A CORRUPT prior sidecar reads the same way (round-14 pin): the
        // read's parse-failure arm is the unknown-timestamp arm, not a
        // preserve arm — never re-stamped, never fabricated.
        let corrupt_path = code_session_sidecar_path(&store_path, "s3");
        std::fs::create_dir_all(corrupt_path.parent().unwrap()).unwrap();
        std::fs::write(&corrupt_path, b"not json").unwrap();
        assert!(rewrite_code_session_sidecar_preserving_bound_at(
            &store_path,
            "s3",
            PathBuf::from("/new/root"),
            Vec::new()
        ));
        assert_eq!(
            read_code_session_sidecar(&store_path, "s3")
                .unwrap()
                .bound_at,
            None,
            "a corrupt prior sidecar yields the unknown timestamp"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn code_sessions_in_workspace_matches_project_code_records_only() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-agent-store-ws-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let record = |kind, path, mode, backend| SessionAgentRecord {
            backend,
            mode,
            workspace_kind: kind,
            workspace_path: path,
            ..SessionAgentRecord::default()
        };
        let store = SessionAgentStore {
            path: dir.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::from([
                (
                    "acp-proj".to_string(),
                    record(
                        CodexWorkspaceKind::Project,
                        Some(dir.clone()),
                        SessionMode::Plain,
                        AgentBackend::CodexAcp,
                    ),
                ),
                (
                    "native-proj".to_string(),
                    record(
                        CodexWorkspaceKind::Project,
                        Some(dir.clone()),
                        SessionMode::Code,
                        AgentBackend::Deepseek,
                    ),
                ),
                (
                    "temp-code".to_string(),
                    record(
                        CodexWorkspaceKind::Temporary,
                        None,
                        SessionMode::Code,
                        AgentBackend::Deepseek,
                    ),
                ),
                (
                    "plain-chat".to_string(),
                    record(
                        CodexWorkspaceKind::Project,
                        Some(dir.clone()),
                        SessionMode::Plain,
                        AgentBackend::Deepseek,
                    ),
                ),
            ]))),
        };
        let mut sessions = store.code_sessions_in_workspace(&dir);
        sessions.sort();
        assert_eq!(
            sessions,
            vec!["acp-proj".to_string(), "native-proj".to_string()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_record_defaults_to_temporary_workspace() {
        let record: SessionAgentRecord = serde_json::from_value(serde_json::json!({
            "backend": "codex-acp",
            "acp_session_id": "legacy-acp"
        }))
        .unwrap();
        assert_eq!(record.workspace_kind, CodexWorkspaceKind::Temporary);
        assert_eq!(record.workspace_path, None);
        assert_eq!(record.acp_mode_id, None);
        assert!(record.acp_config_values.is_empty());
        assert!(!record.mode.is_code());
    }

    #[test]
    fn session_mode_deserializes_legacy_bool_and_serializes_as_bool() {
        // 旧格式布尔键：true=code、false=plain，缺字段按 plain 兼容（缺字段
        // 已由 legacy_record_defaults_to_temporary_workspace 覆盖）。
        let code: SessionAgentRecord = serde_json::from_value(serde_json::json!({
            "backend": "deepseek",
            "code_session": true
        }))
        .unwrap();
        assert_eq!(code.mode, SessionMode::Code);
        let plain: SessionAgentRecord = serde_json::from_value(serde_json::json!({
            "backend": "deepseek",
            "code_session": false
        }))
        .unwrap();
        assert_eq!(plain.mode, SessionMode::Plain);
        // 序列化保持原布尔键与省略语义：code 写 true，plain 不写该键，
        // 旧版本应用读新文件不误判。
        let code_json = serde_json::to_value(&code).unwrap();
        assert_eq!(code_json["code_session"], serde_json::json!(true));
        let plain_json = serde_json::to_value(&plain).unwrap();
        assert!(plain_json.get("code_session").is_none());
        // 前向兼容：未来 kebab-case 字符串格式也能读出。
        let future: SessionAgentRecord = serde_json::from_value(serde_json::json!({
            "backend": "deepseek",
            "code_session": "code"
        }))
        .unwrap();
        assert_eq!(future.mode, SessionMode::Code);
    }

    #[test]
    fn project_workspace_must_exist_and_be_a_directory() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-workspace-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let validated = validate_codex_project_workspace(&root).unwrap();
        assert!(validated.is_dir());
        assert_eq!(
            validated.canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
        // 回归断言：交给 ACP agent 的工作目录不得保留 \\?\ verbatim 前缀，
        // 否则 kimi acp 会把相对路径误判为“工作目录之外”。该断言全平台有效——
        // 非 Windows 的 canonicalize 本就不产生该前缀，Windows 上由 platform_compat_path 归一化。
        assert!(
            !validated.to_string_lossy().starts_with(r"\\?\"),
            "validated workspace must not keep the verbatim prefix: {}",
            validated.display()
        );
        assert!(validate_codex_project_workspace(&root.join("missing")).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn started_codex_session_cannot_change_workspace() {
        let root =
            std::env::temp_dir().join(format!("pinvou3-codex-store-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .set_acp_workspace(
                "session-1",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        store
            .set_acp_session("session-1", "acp-1".to_string(), None, HashMap::new())
            .unwrap();
        assert!(
            store
                .set_acp_workspace(
                    "session-1",
                    AgentBackend::CodexAcp,
                    CodexWorkspaceKind::Temporary,
                    None,
                    Vec::new(),
                )
                .is_err()
        );
        assert!(
            store
                .set_acp_workspace(
                    "session-1",
                    AgentBackend::ClaudeAcp,
                    CodexWorkspaceKind::Project,
                    Some(root.clone()),
                    Vec::new(),
                )
                .is_err()
        );
        assert_eq!(store.get("session-1").backend, AgentBackend::CodexAcp);
        assert_eq!(
            store.get("session-1").workspace_path.as_deref(),
            Some(root.as_path())
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_binding_marks_temporary_deepseek_session() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-store-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session-agents.json");
        let store = SessionAgentStore {
            path: path.clone(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        assert!(!store.is_code_session("session-1"));
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Temporary, None, Vec::new())
            .unwrap();

        let record = store.get("session-1");
        assert_eq!(record.backend, AgentBackend::Deepseek);
        assert_eq!(record.workspace_kind, CodexWorkspaceKind::Temporary);
        assert_eq!(record.workspace_path, None);
        assert!(record.mode.is_code());
        assert!(store.is_code_session("session-1"));
        // 原生绑定不需要 Agent 上下文，同值重复绑定保持幂等。
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Temporary, None, Vec::new())
            .unwrap();
        assert!(store.is_code_session("session-1"));

        let persisted: AgentStoreFile = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let record = &persisted.sessions["session-1"];
        assert!(record.mode.is_code());
        assert_eq!(record.backend, AgentBackend::Deepseek);
        fs::remove_dir_all(&root).unwrap();
    }

    /// Round-42 review: every GUI mutator that whole-table-persists must
    /// reload the disk truth under the cross-process section lock first.
    /// A GUI store instance boot-loaded before the CLI committed its writes
    /// used to persist that stale boot-era table back over them — a routine
    /// config write silently reverted a completed `projects rebind` (and a
    /// bind resurrected a CLI-deleted record), with no error anywhere.
    #[test]
    fn gui_mutators_reload_under_the_section_lock_instead_of_persisting_a_stale_table() {
        let (root, gui) = store_fixture("pinvou3-session-agents-lock-reload-test-");
        let path = gui.path.clone();
        gui.set_acp_session("gui-session", "acp-1".to_string(), None, HashMap::new())
            .unwrap();
        // The CLI is a separate process committing after the GUI booted.
        let cli = store_at(&path);
        cli.set_acp_session("cli-session", "acp-2".to_string(), None, HashMap::new())
            .unwrap();

        // The GUI's routine config write must observe the CLI's committed
        // record instead of reverting it with the boot-era table.
        gui.set_acp_config_value("gui-session", "model", "glm-5")
            .unwrap();
        let on_disk: AgentStoreFile = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(
            on_disk.sessions.contains_key("cli-session"),
            "a GUI config write must not revert the CLI's committed record: {:?}",
            on_disk.sessions.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            on_disk.sessions["gui-session"]
                .acp_config_values
                .get("model"),
            Some(&"glm-5".to_string())
        );

        // Same discipline for remove: a stale-table persist must not
        // resurrect the CLI-side removed record.
        cli.remove_for_second_process("cli-session").unwrap();
        gui.bind_code_native_session(
            "gui-session",
            CodexWorkspaceKind::Temporary,
            None,
            Vec::new(),
        )
        .unwrap();
        let on_disk: AgentStoreFile = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(
            !on_disk.sessions.contains_key("cli-session"),
            "a GUI bind must not resurrect a CLI-removed record: {:?}",
            on_disk.sessions.keys().collect::<Vec<_>>()
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// Round-43 review: `restore_missing_code_session_record` is the boot
    /// pass's whole-table-persisting mutator, so it must run the same
    /// lock-and-reload discipline — a peer record committed after this
    /// process booted must survive the recovery persist, and a corrupt
    /// index must be refused instead of being overwritten.
    #[test]
    fn restore_missing_code_session_record_observes_the_section_truth() {
        let (root, gui) = store_fixture("pinvou3-restore-code-record-lock-test-");
        let path = gui.path.clone();
        gui.bind_code_native_session(
            "booted-session",
            CodexWorkspaceKind::Temporary,
            None,
            Vec::new(),
        )
        .unwrap();
        // The sidecar this boot pass will restore from (index record
        // missing — the recovery lane's premise).
        assert!(write_code_session_sidecar(
            &path,
            "recovered-session",
            CodexWorkspaceKind::Temporary,
            None,
            Vec::new(),
            Vec::new(),
        ));
        // A peer process commits a fresh bind after the GUI booted.
        let peer = store_at(&path);
        peer.bind_code_native_session(
            "peer-session",
            CodexWorkspaceKind::Temporary,
            None,
            Vec::new(),
        )
        .unwrap();

        gui.restore_missing_code_session_record("recovered-session")
            .unwrap();

        let on_disk: AgentStoreFile = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(
            on_disk.sessions.contains_key("peer-session"),
            "the recovery persist must not revert the peer's committed bind: {:?}",
            on_disk.sessions.keys().collect::<Vec<_>>()
        );
        assert!(on_disk.sessions.contains_key("recovered-session"));

        // A sidecar removed together with its session after the boot pass
        // enumerated it must not be restored from the stale copy.
        peer.bind_code_native_session(
            "deleted-session",
            CodexWorkspaceKind::Temporary,
            None,
            Vec::new(),
        )
        .unwrap();
        peer.remove_for_second_process("deleted-session").unwrap();
        assert_eq!(
            gui.restore_missing_code_session_record("deleted-session")
                .unwrap(),
            false,
            "a sidecar deleted with its session must not restore the deleted record"
        );
        let on_disk: AgentStoreFile = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(!on_disk.sessions.contains_key("deleted-session"));

        // Fail-closed on a corrupt index, same as the other lock-and-reload
        // mutators.
        fs::write(&path, "{not json").unwrap();
        assert!(
            gui.restore_missing_code_session_record("recovered-session")
                .is_err(),
            "a corrupt index must be refused, not replaced by a stale-table persist"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// Round-11 R1 (review #484 round-11): the align lane's keychain writer
    /// is one of the last two mutators to join the section-lock discipline.
    /// Without it, a whole-table persist reverted any second-process commit
    /// that landed after this store's boot load — the exact shape the
    /// reviewer demonstrated with `pinvou projects rebind` / `sessions
    /// delete` in a CLI process. Pin: a peer commit between the GUI boot
    /// and the align persist must survive, and a corrupt index must be
    /// refused instead of replaced by the stale table.
    #[test]
    fn set_session_workspace_roots_observes_the_section_truth() {
        let (root, gui) = store_fixture("pinvou3-align-roots-section-truth-");
        let path = gui.path.clone();
        let project = root.join("proj");
        fs::create_dir_all(&project).unwrap();
        gui.bind_code_native_session(
            "aligned-session",
            CodexWorkspaceKind::Project,
            Some(project.clone()),
            Vec::new(),
        )
        .unwrap();
        // A peer process commits a fresh bind after the GUI booted.
        let peer = store_at(&path);
        peer.bind_code_native_session(
            "peer-session",
            CodexWorkspaceKind::Temporary,
            None,
            Vec::new(),
        )
        .unwrap();

        let write = gui
            .set_session_workspace_roots_with_explicit_ancestors(
                "aligned-session",
                vec![project.clone()],
                Vec::new(),
            )
            .unwrap();
        assert!(write.applied);

        let on_disk: AgentStoreFile = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(
            on_disk.sessions.contains_key("peer-session"),
            "the align persist must not revert the peer's committed bind: {:?}",
            on_disk.sessions.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            on_disk.sessions["aligned-session"].workspace_roots,
            vec![project.clone()],
            "the align itself must persist"
        );

        // Fail-closed on a corrupt index, same as the other lock-and-reload
        // mutators: the align must refuse, not replace the file with the
        // stale in-memory table.
        fs::write(&path, "{not json").unwrap();
        assert!(
            gui.set_session_workspace_roots("aligned-session", vec![project])
                .is_err(),
            "a corrupt index must be refused, not replaced by a stale-table persist"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// Round-11 R1, second writer: the detached-root sweep mutates through
    /// [`Self::set_session_workspace_roots`] per target, so the sweep as a
    /// whole must not revert peer commits either — each per-target persist
    /// reloads under the section lock.
    #[test]
    fn rebind_detached_workspace_roots_observes_the_section_truth() {
        let (root, gui) = store_fixture("pinvou3-detached-sweep-section-truth-");
        let path = gui.path.clone();
        let from = root.join("from-root");
        let to = root.join("to-root");
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&to).unwrap();
        gui.bind_code_native_session(
            "swept-session",
            CodexWorkspaceKind::Project,
            Some(root.join("stationary")),
            vec![from.join("extra")],
        )
        .unwrap();
        // The sweep's owner gate requires the SavedSession record
        // (`sessions/<id>.json` beside the index) to exist.
        fs::write(
            code_session_sidecar_root(&path).join("swept-session.json"),
            "{}",
        )
        .unwrap();
        // A peer process commits a fresh bind after the GUI booted, AND adds
        // a brand-new root to the SAME session the GUI's scan is about to
        // sweep (round-36 P1): the old shape computed the replacement from
        // the pre-lock scan, so this root was silently dropped by the
        // stale-derived replacement.
        let peer = store_at(&path);
        peer.bind_code_native_session(
            "peer-session",
            CodexWorkspaceKind::Temporary,
            None,
            Vec::new(),
        )
        .unwrap();
        let peer_added = root.join("peer-added");
        fs::create_dir_all(&peer_added).unwrap();
        peer.set_session_workspace_roots(
            "swept-session",
            vec![
                root.join("stationary"),
                from.join("extra"),
                peer_added.clone(),
            ],
        )
        .unwrap();

        let (touched, failed) = gui.rebind_detached_workspace_roots(&from, &to);
        assert_eq!(touched, vec!["swept-session".to_string()]);
        assert!(failed.is_empty());

        let on_disk: AgentStoreFile = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(
            on_disk.sessions.contains_key("peer-session"),
            "the sweep's per-target persist must not revert the peer's committed bind: {:?}",
            on_disk.sessions.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            on_disk.sessions["swept-session"].workspace_roots,
            vec![
                root.join("stationary"),
                to.join("extra"),
                peer_added.clone()
            ],
            "the swept keychain must land on the `to` spelling (cwd-first) and the              peer-added root committed after the scan must SURVIVE the locked write"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// Round-11 SF2/SF6/SF7: the align-granted ancestor provenance must
    /// survive an index loss (it now rides the authoritative sidecar), and
    /// every NON-align keychain rewrite — a same-shape rebind, a repeated
    /// set_acp_workspace, and the prefix rebind — must RESET it in the same
    /// write, honoring the invariant the store's own docs assert.
    #[test]
    fn explicit_ancestor_provenance_survives_index_loss_and_resets_on_rewrites() {
        let (root, gui) = store_fixture("pinvou3-provenance-lifecycle-");
        let path = gui.path.clone();
        let project = root.join("proj");
        let granted = root.join("granted-ancestor");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&granted).unwrap();
        gui.bind_code_native_session(
            "provenance-session",
            CodexWorkspaceKind::Project,
            Some(project.clone()),
            Vec::new(),
        )
        .unwrap();
        // The owner record the recovery/restore paths require.
        fs::write(
            code_session_sidecar_root(&path).join("provenance-session.json"),
            "{}",
        )
        .unwrap();

        // The align grant: the keychain plus its explicit ancestors.
        let write = gui
            .set_session_workspace_roots_with_explicit_ancestors(
                "provenance-session",
                vec![project.clone(), granted.clone()],
                vec![granted.clone()],
            )
            .unwrap();
        assert!(write.applied);

        // SF2: a lost index rebuilds THROUGH the sidecar — the grant must
        // ride it, not evaporate in the recovery window. The reboot is a
        // fresh store handle over the same path (the old in-memory table
        // would short-circuit the recovery as "already code").
        fs::remove_file(&path).unwrap();
        let rebooted = store_at(&path);
        assert!(
            rebooted
                .restore_missing_code_session_record("provenance-session")
                .unwrap(),
            "the sidecar-backed recovery must rebuild the record"
        );
        let record = rebooted.get("provenance-session");
        assert_eq!(
            record.explicit_ancestor_roots,
            vec![granted.clone()],
            "SF2: the rebuild must carry the sidecar's recorded grant"
        );

        // SF6: a prefix rebind is a non-align rewrite — the translated
        // keychain carries no grant; the stale spellings must not linger.
        let to = root.join("moved");
        fs::create_dir_all(&to).unwrap();
        rebooted.rebind_workspace_prefix(&project, &to).unwrap();
        assert_eq!(
            rebooted.get("provenance-session").explicit_ancestor_roots,
            Vec::<PathBuf>::new(),
            "SF6: the rebind replacement keychain resets the provenance"
        );

        // SF7: re-align, then a same-shape set_acp_workspace repeat — the
        // replacement keychain resets the previous align's grants.
        let write = rebooted
            .set_session_workspace_roots_with_explicit_ancestors(
                "provenance-session",
                vec![to.clone(), granted.clone()],
                vec![granted.clone()],
            )
            .unwrap();
        assert!(write.applied);
        rebooted
            .bind_code_native_session(
                "provenance-session",
                CodexWorkspaceKind::Project,
                Some(to.clone()),
                vec![to.clone(), granted.clone()],
            )
            .unwrap();
        assert_eq!(
            rebooted.get("provenance-session").explicit_ancestor_roots,
            Vec::<PathBuf>::new(),
            "SF7: a same-shape repeat set_acp_workspace resets the provenance"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// Round-44 review: the under-lock re-check guards the DELETE direction
    /// of the stale enumeration, but a concurrent `projects rebind` REWRITES
    /// the sidecar onto the new root. Restoring from the passed-in copy
    /// would re-insert the pre-rebind binding while the sidecar (and the
    /// rebind receipt) already say otherwise. The recovery must rebuild the
    /// record from the sidecar bytes read under the section lock.
    #[test]
    fn restore_missing_code_session_record_rebuilds_from_the_rewritten_sidecar() {
        let (root, gui) = store_fixture("pinvou3-restore-code-record-rewrite-test-");
        let path = gui.path.clone();
        // What the boot pass enumerated: an orphan sidecar bound to the old
        // temporary workspace (no index record — the recovery lane's premise).
        assert!(write_code_session_sidecar(
            &path,
            "rebound-session",
            CodexWorkspaceKind::Temporary,
            None,
            Vec::new(),
            Vec::new(),
        ));
        // A peer `projects rebind` commits between the enumeration and the
        // restore: the sidecar is rewritten onto the new project root.
        let rebound_root = root.join("rebound-project");
        fs::create_dir_all(&rebound_root).unwrap();
        assert!(write_code_session_sidecar(
            &path,
            "rebound-session",
            CodexWorkspaceKind::Project,
            Some(rebound_root.clone()),
            Vec::new(),
            Vec::new(),
        ));

        assert!(
            gui.restore_missing_code_session_record("rebound-session")
                .unwrap(),
            "an orphan sidecar must still restore"
        );

        let on_disk: AgentStoreFile = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let restored = &on_disk.sessions["rebound-session"];
        assert_eq!(restored.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(
            restored.workspace_path.as_deref(),
            Some(rebound_root.as_path())
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// Round-43 review: the sidecar backfill decides "missing" from the
    /// table, so it must reload under the section lock — backfilling from
    /// the boot-era snapshot would re-create the sidecar of a session a
    /// peer deleted (record and sidecar together), which the next boot's
    /// restore pass would turn back into an index record.
    #[test]
    fn sidecar_backfill_does_not_resurrect_a_peer_deleted_session() {
        let (root, gui) = store_fixture("pinvou3-backfill-lock-test-");
        let path = gui.path.clone();
        gui.bind_code_native_session(
            "legacy-session",
            CodexWorkspaceKind::Temporary,
            None,
            Vec::new(),
        )
        .unwrap();
        // A pre-sidecar-persist build's legacy record: index in, sidecar
        // out — exactly what the backfill exists to heal.
        remove_code_session_sidecar(&path, "legacy-session");
        // The peer deletes the session (record and sidecar) after boot.
        let peer = store_at(&path);
        peer.remove_for_second_process("legacy-session").unwrap();

        let backfilled = gui.backfill_missing_code_session_sidecars().unwrap();

        assert_eq!(backfilled, 0);
        assert!(
            read_code_session_sidecar(&path, "legacy-session").is_none(),
            "backfill must not re-create the sidecar of a deleted session"
        );
        let on_disk: AgentStoreFile = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(!on_disk.sessions.contains_key("legacy-session"));
        fs::remove_dir_all(&root).unwrap();
    }

    /// Round-42 review: the lock-and-reload lanes refuse a corrupt index
    /// (fail-closed) instead of persisting a stale table over it, matching
    /// `remove_for_second_process` and `rebind_workspace_prefix`.
    #[test]
    fn gui_mutators_refuse_a_corrupt_index_instead_of_persisting_over_it() {
        let (root, store) = store_fixture("pinvou3-session-agents-corrupt-refusal-test-");
        let path = store.path.clone();
        store
            .set_acp_session("session-1", "acp-1".to_string(), None, HashMap::new())
            .unwrap();
        fs::write(&path, "{not json").unwrap();
        let corrupt_bytes = fs::read(&path).unwrap();

        assert!(
            store
                .set_acp_config_value("session-1", "model", "glm-5")
                .is_err(),
            "a corrupt index must be refused, not replaced by a stale-table persist"
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            corrupt_bytes,
            "the refused write must leave the corrupt file untouched"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn started_acp_session_cannot_be_rebound_to_code_native() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-rebind-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .set_acp_workspace(
                "session-1",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Temporary,
                None,
                Vec::new(),
            )
            .unwrap();
        store
            .set_acp_session("session-1", "acp-1".to_string(), None, HashMap::new())
            .unwrap();
        assert!(
            store
                .bind_code_native_session(
                    "session-1",
                    CodexWorkspaceKind::Temporary,
                    None,
                    Vec::new()
                )
                .is_err()
        );
        let record = store.get("session-1");
        assert_eq!(record.backend, AgentBackend::CodexAcp);
        assert!(!record.mode.is_code());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_project_binding_validates_kind_and_path() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-project-bind-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        // kind 与 path 必须配套。
        assert!(
            store
                .bind_code_native_session(
                    "session-1",
                    CodexWorkspaceKind::Project,
                    None,
                    Vec::new()
                )
                .is_err()
        );
        assert!(
            store
                .bind_code_native_session(
                    "session-1",
                    CodexWorkspaceKind::Temporary,
                    Some(root.clone()),
                    Vec::new(),
                )
                .is_err()
        );
        assert!(!store.is_code_session("session-1"));

        store
            .bind_code_native_session(
                "session-1",
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        let record = store.get("session-1");
        assert_eq!(record.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(record.workspace_path.as_deref(), Some(root.as_path()));
        assert!(record.mode.is_code());

        // 已绑定的代码会话不可改绑工作区；同值重复绑定幂等。
        assert!(
            store
                .bind_code_native_session(
                    "session-1",
                    CodexWorkspaceKind::Temporary,
                    None,
                    Vec::new()
                )
                .is_err()
        );
        assert!(
            store
                .bind_code_native_session(
                    "session-1",
                    CodexWorkspaceKind::Project,
                    Some(root.join("other")),
                    Vec::new(),
                )
                .is_err()
        );
        store
            .bind_code_native_session(
                "session-1",
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        let record = store.get("session-1");
        assert_eq!(record.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(record.workspace_path.as_deref(), Some(root.as_path()));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rebind_workspace_prefix_rewrites_index_and_sidecars() {
        let root =
            std::env::temp_dir().join(format!("pinvou3-codex-rebind-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let from = root.join("from");
        let to = root.join("to");
        fs::create_dir_all(from.join("sub")).unwrap();
        fs::create_dir_all(&to).unwrap();

        // s1: native code session, binding = from itself (writes a sidecar);
        // s2: ACP project session, binding = from/sub (no sidecar);
        // s3: bound elsewhere, unaffected; prefix boundary: from-x must not
        // match from.
        store
            .bind_code_native_session(
                "s1",
                CodexWorkspaceKind::Project,
                Some(from.clone()),
                Vec::new(),
            )
            .unwrap();
        store
            .set_acp_workspace(
                "s2",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(from.join("sub")),
                Vec::new(),
            )
            .unwrap();
        store
            .bind_code_native_session(
                "s3",
                CodexWorkspaceKind::Project,
                Some(root.join("elsewhere")),
                Vec::new(),
            )
            .unwrap();
        store
            .bind_code_native_session(
                "s4",
                CodexWorkspaceKind::Project,
                Some(root.join("from-x")),
                Vec::new(),
            )
            .unwrap();
        for sid in ["s1", "s2", "s3", "s4"] {
            touch_owner_record(&store.path, sid);
        }

        let matched = store.sessions_under_workspace(&from);
        // s1 matches in both the index and the sidecar; dedupe by session_id
        // counts it once (index wins).
        assert_eq!(matched.len(), 2);

        let bound_at_before =
            read_code_session_sidecar(&store.path, "s1").and_then(|sidecar| sidecar.bound_at);
        assert!(bound_at_before.is_some());

        let outcome = store.rebind_workspace_prefix(&from, &to).unwrap();
        let mut ids: Vec<&str> = outcome
            .affected
            .iter()
            .map(|(sid, _)| sid.as_str())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["s1", "s2"]);
        assert!(outcome.sidecar_final_stale.is_empty());
        assert_eq!(
            store.get("s1").workspace_path.as_deref(),
            Some(to.as_path())
        );
        assert_eq!(
            store.get("s2").workspace_path.as_deref(),
            Some(to.join("sub").as_path())
        );
        assert_eq!(
            store.get("s3").workspace_path.as_deref(),
            Some(root.join("elsewhere").as_path()),
            "prefix 外会话不动"
        );
        assert_eq!(
            store.get("s4").workspace_path.as_deref(),
            Some(root.join("from-x").as_path()),
            "目录边界:sibling 前缀不得误命中"
        );

        // The authoritative sidecar is rewritten too (s1); the ACP session s2
        // has no sidecar. bound_at keeps the first-bind timestamp — the
        // index rewrite-retry pass below carries the original over instead
        // of re-stamping (review #463 minor; the preserved-stamp writer is
        // the #464 follow-up nit).
        let sidecar = read_code_session_sidecar(&store.path, "s1").unwrap();
        assert_eq!(sidecar.workspace_path.as_deref(), Some(to.as_path()));
        assert_eq!(sidecar.bound_at, bound_at_before);

        // Idempotent: a second from→to run matches nothing.
        let rerun = store.rebind_workspace_prefix(&from, &to).unwrap();
        assert!(rerun.affected.is_empty());
        assert!(rerun.sidecar_final_stale.is_empty());

        // Orphan sidecars (no index record) are rewritten too, so a restart
        // restore does not resurrect the old directory.
        persist_code_session_sidecar(
            &code_session_sidecar_path(&store.path, "orphan"),
            &CodeSessionSidecar {
                version: CODE_SESSION_SIDECAR_VERSION,
                workspace_kind: CodexWorkspaceKind::Project,
                workspace_path: Some(from.join("deep")),
                workspace_roots: Vec::new(),
                explicit_ancestor_roots: Vec::new(),
                bound_at: None,
            },
        )
        .unwrap();
        touch_owner_record(&store.path, "orphan");
        // The candidate set covers off-index orphans too (review #463 M6):
        // the fence no longer misses orphans.
        let matched = store.sessions_under_workspace(&root.join("from"));
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].0, "orphan");
        assert_eq!(matched[0].1, from.join("deep"));
        let affected = store
            .rebind_workspace_prefix(&root.join("from"), &root.join("to2"))
            .unwrap();
        fs::create_dir_all(root.join("to2")).unwrap();
        // The previous run moved `to` away; this round has no index hit for
        // from, but the orphan sidecar matches.
        assert!(affected.affected.iter().any(|(sid, _)| sid == "orphan"));
        assert!(affected.sidecar_final_stale.is_empty());
        let orphan = read_code_session_sidecar(&store.path, "orphan").unwrap();
        assert_eq!(
            orphan.workspace_path.as_deref(),
            Some(root.join("to2").join("deep").as_path())
        );

        fs::remove_dir_all(&root).unwrap();
    }

    /// Round-41 review: the rebind is a second-process wholesale write, so it
    /// must reload the index under the section lock instead of persisting its
    /// boot-era snapshot. A GUI session bind landing after the rebind
    /// process booted (s2 below) has no workspace sidecar to restore it —
    /// the whole-table persist used to drop it silently.
    #[test]
    fn rebind_workspace_prefix_persists_a_record_bound_after_this_process_booted() {
        let (root, store) = store_fixture("pinvou3-codex-rebind-second-process-");
        // `store` is the rebind process: its memory holds only s1 (the
        // boot-era snapshot).
        let from = root.join("from");
        let to = root.join("to");
        fs::create_dir_all(from.join("sub")).unwrap();
        fs::create_dir_all(&to).unwrap();
        store
            .set_acp_workspace(
                "s1",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(from.clone()),
                Vec::new(),
            )
            .unwrap();

        // The GUI bind landing mid-rebind: s2 enters the on-disk index only
        // (ACP sessions keep no workspace sidecar after bind). The record is
        // built through a real `set_acp_workspace` on a scratch store so the
        // serialized shape stays the production one, then spliced into the
        // index the way the GUI's own persist would have.
        let scratch_store = store_at(&root.join("scratch-index.json"));
        scratch_store
            .set_acp_workspace(
                "s2",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(from.join("sub")),
                Vec::new(),
            )
            .unwrap();
        let mut index: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&store.path).unwrap()).unwrap();
        let scratch: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&scratch_store.path).unwrap()).unwrap();
        index["sessions"]["s2"] = scratch["sessions"]["s2"].clone();
        fs::write(&store.path, serde_json::to_string_pretty(&index).unwrap()).unwrap();
        // ACP sessions keep no workspace sidecar, but the owner-existence
        // gate reads a record file in the sidecar root — create the root the
        // way a native session's bind would have.
        fs::create_dir_all(code_session_sidecar_root(&store.path)).unwrap();
        touch_owner_record(&store.path, "s1");
        touch_owner_record(&store.path, "s2");

        let outcome = store.rebind_workspace_prefix(&from, &to).unwrap();
        let mut ids: Vec<&str> = outcome
            .affected
            .iter()
            .map(|(sid, _)| sid.as_str())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["s1", "s2"]);

        // Both records survive the whole-table persist, moved onto `to`.
        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&store.path).unwrap()).unwrap();
        assert_eq!(
            persisted["sessions"]["s1"]["workspace_path"].as_str(),
            Some(to.to_string_lossy().as_ref())
        );
        assert_eq!(
            persisted["sessions"]["s2"]["workspace_path"].as_str(),
            Some(to.join("sub").to_string_lossy().as_ref()),
            "a bind landing after this process booted must not be dropped by the rebind persist"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    /// Round-41 review: a corrupt index must fail the rebind (fail-closed,
    /// like `remove_for_second_process` and the plain lane's
    /// REBIND_LEGACY_TABLE_CORRUPT), not be replaced by an (near-)empty
    /// table persist that throws away every recoverable record.
    #[test]
    fn rebind_workspace_prefix_refuses_a_corrupt_index_instead_of_emptying_it() {
        let (root, store) = store_fixture("pinvou3-codex-rebind-corrupt-index-");
        let from = root.join("from");
        let to = root.join("to");
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&to).unwrap();
        fs::write(&store.path, "{ not json").unwrap();

        let outcome = store.rebind_workspace_prefix(&from, &to);
        assert!(
            outcome.is_err(),
            "a corrupt index must be refused, not rewritten"
        );
        assert_eq!(
            fs::read_to_string(&store.path).unwrap(),
            "{ not json",
            "the corrupt file is left untouched for recovery"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rebind_prefix_translates_keychain_snapshots_with_the_binding() {
        // Review #484 M3: every lane of rebind_workspace_prefix must migrate
        // the keychain snapshot with the binding — stale `/from` roots would
        // be normalized into additional writable roots outside `to` on the
        // next resume. Roots outside the prefix stay untouched, in the index
        // record and in an off-index orphan sidecar alike.
        let root =
            std::env::temp_dir().join(format!("pinvou3-codex-rebind-roots-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let from = root.join("from");
        let to = root.join("to");
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&to).unwrap();
        let elsewhere = root.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();

        // Indexed native code session: the keychain payload is cwd-first
        // (§6) — primary root under `from`, an attached root under `from`,
        // and one outside it.
        store
            .bind_code_native_session(
                "s1",
                CodexWorkspaceKind::Project,
                Some(from.clone()),
                vec![from.clone(), from.join("extra"), elsewhere.clone()],
            )
            .unwrap();
        // The merged base's owner re-check (review #463 round-15 SF-B) skips
        // ownerless records in both lanes, so this PR-era fixture must
        // register live owners like its siblings do.
        touch_owner_record(&store.path, "s1");
        let outcome = store.rebind_workspace_prefix(&from, &to).unwrap();
        assert_eq!(outcome.affected.len(), 1);
        let record = store.get("s1");
        assert_eq!(record.workspace_path.as_deref(), Some(to.as_path()));
        assert_eq!(
            record.workspace_roots,
            vec![to.clone(), to.join("extra"), elsewhere.clone()],
            "prefix roots shift onto `to`, outside roots stay"
        );
        // The sidecar the bind wrote carries the same rebound set.
        let sidecar = read_code_session_sidecar(&store.path, "s1").unwrap();
        assert_eq!(
            sidecar.workspace_roots,
            vec![to.clone(), to.join("extra"), elsewhere.clone()]
        );

        // Off-index orphan sidecar: the orphan rewrite pass translates the
        // snapshot itself (no index record owns it).
        persist_code_session_sidecar(
            &code_session_sidecar_path(&store.path, "orphan"),
            &CodeSessionSidecar {
                version: CODE_SESSION_SIDECAR_VERSION,
                workspace_kind: CodexWorkspaceKind::Project,
                workspace_path: Some(from.join("deep")),
                workspace_roots: vec![from.join("deep"), elsewhere.clone()],
                explicit_ancestor_roots: Vec::new(),
                bound_at: None,
            },
        )
        .unwrap();
        touch_owner_record(&store.path, "orphan");
        let outcome = store.rebind_workspace_prefix(&from, &to).unwrap();
        assert!(outcome.affected.iter().any(|(sid, _)| sid == "orphan"));
        let orphan = read_code_session_sidecar(&store.path, "orphan").unwrap();
        assert_eq!(
            orphan.workspace_path.as_deref(),
            Some(to.join("deep").as_path())
        );
        assert_eq!(
            orphan.workspace_roots,
            vec![to.join("deep"), elsewhere],
            "the orphan's snapshot migrates with its binding"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rebind_repairs_index_sidecar_disagreement_on_a_rekeyed_rerun() {
        // review #463 round-8 minor 9: run 1 (from→to) delivers the index but
        // both sidecar passes fail; run 2 (from→to2) then moves only the
        // sidecar and the metadata, because the index record at `to` matches
        // neither lane of run 2. Without the divergence repair the index would
        // stay at `to` forever — no rerun key reaches it.
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-rebind-divergence-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let from = root.join("from");
        let to = root.join("to");
        let to2 = root.join("to2");
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&to).unwrap();
        fs::create_dir_all(&to2).unwrap();
        store
            .bind_code_native_session(
                "s1",
                CodexWorkspaceKind::Project,
                Some(from.clone()),
                Vec::new(),
            )
            .unwrap();
        touch_owner_record(&store.path, "s1");

        // Reproduce the interrupted run-1 end state directly: the index move
        // persisted but both sidecar passes failed, so the index sits at `to`
        // while the authoritative sidecar still holds `from`. (Running
        // `rebind_workspace_prefix(from, to)` here would NOT reproduce it — its
        // sidecar pass heals exactly this disagreement.)
        {
            let mut records = store.records.write();
            records.get_mut("s1").unwrap().workspace_path = Some(to.clone());
        }
        assert_eq!(
            read_code_session_sidecar(&store.path, "s1")
                .unwrap()
                .workspace_path
                .as_deref(),
            Some(from.as_path()),
            "precondition: index at `to`, sidecar still under `from`"
        );

        // Run 2 with a different target: the sidecar lane matches `from` and
        // the repair re-keys the stranded index record onto the same target.
        let run2 = store.rebind_workspace_prefix(&from, &to2).unwrap();
        assert!(run2.sidecar_final_stale.is_empty());
        assert_eq!(
            store
                .records
                .read()
                .get("s1")
                .unwrap()
                .workspace_path
                .as_deref(),
            Some(to2.as_path()),
            "index converges onto the sidecar's target instead of stranding at `to`"
        );
        let sidecar = read_code_session_sidecar(&store.path, "s1").unwrap();
        assert_eq!(sidecar.workspace_path.as_deref(), Some(to2.as_path()));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rebind_repairs_a_stranded_index_record_the_lanes_cannot_reach() {
        // review #463 round-10 Major 1: run 1 (from→to) delivered the index
        // but both sidecar passes failed; run 2 (from→to2) moved the sidecar
        // and its divergence re-key failed to persist — index@`to`,
        // sidecar@`to2`. Run 3 (from→to2) matches NEITHER prefix in either
        // store pass, so the lane alone reports an empty success while the
        // index keeps pointing at the vanished `to`. The command's to-lane
        // scan surfaces the sidecar, detects the disagreement, and drives
        // the repair — the convergence the old rollback comment promised.
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-rebind-stranded-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let from = root.join("from");
        let to = root.join("to");
        let to2 = root.join("to2");
        for dir in [&from, &to, &to2] {
            fs::create_dir_all(dir).unwrap();
        }
        store
            .bind_code_native_session(
                "s1",
                CodexWorkspaceKind::Project,
                Some(from.clone()),
                Vec::new(),
            )
            .unwrap();
        touch_owner_record(&store.path, "s1");

        // The run-2 persist-failure end state, constructed directly: the index
        // move rolled back to its pre-run value (`to`), the sidecar write
        // survived (`to2`). Persisted, because round-41's fresh reload reads
        // the DISK state — the real run-2 scenario had `to` on disk (run-1
        // delivered it), not just in memory.
        {
            let mut records = store.records.write();
            records.get_mut("s1").unwrap().workspace_path = Some(to.clone());
        }
        store.persist().unwrap();
        persist_code_session_sidecar(
            &code_session_sidecar_path(&store.path, "s1"),
            &CodeSessionSidecar {
                version: CODE_SESSION_SIDECAR_VERSION,
                workspace_kind: CodexWorkspaceKind::Project,
                workspace_path: Some(to2.clone()),
                workspace_roots: Vec::new(),
                explicit_ancestor_roots: Vec::new(),
                bound_at: None,
            },
        )
        .unwrap();

        // The lane alone cannot converge it: no index record and no sidecar
        // sits under `from`, and `to` is not under `to2`.
        let run3 = store.rebind_workspace_prefix(&from, &to2).unwrap();
        assert!(run3.affected.is_empty());
        assert!(run3.sidecar_final_stale.is_empty());
        assert_eq!(
            store.get("s1").workspace_path.as_deref(),
            Some(to.as_path()),
            "precondition: the lane leaves the stranded index record at `to`"
        );

        // The command-driven repair re-keys it onto the sidecar's target.
        let repaired = store
            .repair_stranded_index_records(&[("s1".to_string(), to2.clone())])
            .unwrap();
        assert_eq!(repaired, vec!["s1".to_string()]);
        assert_eq!(
            store.get("s1").workspace_path.as_deref(),
            Some(to2.as_path()),
            "the index converges onto the sidecar-authoritative target"
        );
        let sidecar = read_code_session_sidecar(&store.path, "s1").unwrap();
        assert_eq!(sidecar.workspace_path.as_deref(), Some(to2.as_path()));

        // Idempotent: an already-converged record is skipped, not rewritten.
        let again = store
            .repair_stranded_index_records(&[("s1".to_string(), to2.clone())])
            .unwrap();
        assert!(again.is_empty());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn stranded_index_repair_rolls_back_when_persist_fails() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-rebind-stranded-persist-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let from = root.join("from");
        let to = root.join("to");
        let to2 = root.join("to2");
        for dir in [&from, &to, &to2] {
            fs::create_dir_all(dir).unwrap();
        }
        store
            .bind_code_native_session(
                "s1",
                CodexWorkspaceKind::Project,
                Some(from.clone()),
                Vec::new(),
            )
            .unwrap();
        touch_owner_record(&store.path, "s1");
        {
            let mut records = store.records.write();
            records.get_mut("s1").unwrap().workspace_path = Some(to.clone());
        }

        // Same cross-platform fault as the rebind-prefix test above (round-21
        // SF-7 moved the persist onto atomic_write; no #[cfg] here — this
        // file carries zero target-cfg baseline debt).
        fs::remove_file(&store.path).unwrap();
        fs::create_dir(&store.path).unwrap();
        store
            .repair_stranded_index_records(&[("s1".to_string(), to2.clone())])
            .expect_err("a persist failure must be reported");
        assert_eq!(
            store.get("s1").workspace_path.as_deref(),
            Some(to.as_path()),
            "memory must roll back to the pre-repair binding, not claim a repair disk does not have"
        );

        // Clearing the obstruction converges on a same-process retry.
        fs::remove_dir(&store.path).unwrap();
        let repaired = store
            .repair_stranded_index_records(&[("s1".to_string(), to2.clone())])
            .unwrap();
        assert_eq!(repaired, vec!["s1".to_string()]);
        assert_eq!(
            store.get("s1").workspace_path.as_deref(),
            Some(to2.as_path())
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rebind_post_pass_fence_rule_flags_only_records_left_under_from() {
        // review #463 round-8 minor 8: the rebind gate serializes rebinds only,
        // so a concurrent bind/remove can land in the unlocked sidecar-IO
        // window and leave a record under `from` after the rewrite passes ran.
        // The post-pass re-scan must surface exactly those as finally stale.
        let from = Path::new("/work/alpha");
        let mut records: HashMap<String, SessionAgentRecord> = HashMap::new();
        records.insert(
            "moved".to_string(),
            SessionAgentRecord {
                workspace_kind: CodexWorkspaceKind::Project,
                workspace_path: Some(PathBuf::from("/vault/beta")),
                ..Default::default()
            },
        );
        records.insert(
            "left-behind".to_string(),
            SessionAgentRecord {
                workspace_kind: CodexWorkspaceKind::Project,
                workspace_path: Some(PathBuf::from("/work/alpha/sub")),
                ..Default::default()
            },
        );
        records.insert(
            "sibling-prefix".to_string(),
            SessionAgentRecord {
                workspace_kind: CodexWorkspaceKind::Project,
                workspace_path: Some(PathBuf::from("/work/alphax")),
                ..Default::default()
            },
        );
        records.insert(
            "temporary".to_string(),
            SessionAgentRecord {
                workspace_kind: CodexWorkspaceKind::Temporary,
                workspace_path: Some(PathBuf::from("/work/alpha/tmp")),
                ..Default::default()
            },
        );

        let mut flagged = SessionAgentStore::records_still_under_workspace(&records, from);
        flagged.sort_unstable();
        assert_eq!(
            flagged,
            vec!["left-behind".to_string()],
            "only project records genuinely under the prefix are stale"
        );
    }

    #[test]
    fn rebind_target_path_covers_from_prefix_to_side_and_no_match() {
        // round-7 m7: the cut helper was previously covered only indirectly.
        // Three arms: from-prefix shift, to-prefix (retry candidate) returned
        // as-is, and no match returns None.
        let from = Path::new("/work/alpha");
        let to = Path::new("/vault/beta");
        assert_eq!(
            SessionAgentStore::rebind_target_path(Path::new("/work/alpha"), from, to),
            Some(PathBuf::from("/vault/beta"))
        );
        assert_eq!(
            SessionAgentStore::rebind_target_path(Path::new("/work/alpha/sub"), from, to),
            Some(PathBuf::from("/vault/beta/sub"))
        );
        // Already under `to`: retry candidate returned as-is (the metadata
        // loop converges on this).
        assert_eq!(
            SessionAgentStore::rebind_target_path(Path::new("/vault/beta/keep"), from, to),
            Some(PathBuf::from("/vault/beta/keep"))
        );
        // A from-x sibling and unrelated paths are not candidates.
        assert_eq!(
            SessionAgentStore::rebind_target_path(Path::new("/work/alphax"), from, to),
            None
        );
        assert_eq!(
            SessionAgentStore::rebind_target_path(Path::new("/elsewhere"), from, to),
            None
        );
    }

    #[test]
    fn rebind_prefix_reports_orphan_sidecar_write_failures() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-rebind-sidecar-fail-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let from = root.join("from");
        let to = root.join("to");
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&to).unwrap();
        // Off-index orphan: the sidecar is the only authoritative carrier
        // (no index record, no rewrite-retry path).
        persist_code_session_sidecar(
            &code_session_sidecar_path(&store.path, "orphan-deny"),
            &CodeSessionSidecar {
                version: CODE_SESSION_SIDECAR_VERSION,
                workspace_kind: CodexWorkspaceKind::Project,
                workspace_path: Some(from.clone()),
                workspace_roots: Vec::new(),
                explicit_ancestor_roots: Vec::new(),
                bound_at: None,
            },
        )
        .unwrap();
        touch_owner_record(&store.path, "orphan-deny");
        // Indexed native code session: both the rewrite pass and the retry
        // pass try to rewrite its sidecar.
        store
            .bind_code_native_session(
                "indexed-deny",
                CodexWorkspaceKind::Project,
                Some(from.clone()),
                Vec::new(),
            )
            .unwrap();
        touch_owner_record(&store.path, "indexed-deny");
        // Occupy the temp-file path both sessions persist through: writing
        // code-session.json.tmp fails immediately and rename is never
        // reached — a cross-platform-stable simulation of a persist failure.
        // The orphan has only the rewrite pass; the indexed session hits the
        // same wall in both passes, which is "finally stale". Round-22
        // minor 4 moved the sidecar persist onto atomic_write (randomized
        // temp names defeat path-occupation injections), so the fault is the
        // crate's established cfg(test) injection seam — deterministic on
        // every platform, SESSION-ID-SCOPED so parallel tests never see each
        // other's faults, and both deny ids ride the same persist call.
        *INJECT_SIDECAR_PERSIST_FAIL_IDS
            .lock()
            .unwrap_or_else(|p| p.into_inner()) =
            Some(vec!["orphan-deny".to_string(), "indexed-deny".to_string()]);

        let outcome = store.rebind_workspace_prefix(&from, &to).unwrap();
        let mut stale = outcome.sidecar_final_stale.clone();
        stale.sort();
        assert_eq!(
            stale,
            vec!["indexed-deny".to_string(), "orphan-deny".to_string()]
        );
        // An orphan whose write failed must NOT enter affected — the sidecar
        // is its binding, so claiming it moved would be a false report
        // (review #463 nit); the indexed session's index was rewritten, so
        // affected honestly includes it.
        assert!(!outcome.affected.iter().any(|(sid, _)| sid == "orphan-deny"));
        assert!(
            outcome
                .affected
                .iter()
                .any(|(sid, _)| sid == "indexed-deny")
        );
        // The on-disk sidecars still hold the old paths: the command layer
        // counts both as failed rather than succeeded (Major 2), and on a
        // rerun after failure the old paths still match the from prefix, so
        // the retry converges.
        for sid in ["orphan-deny", "indexed-deny"] {
            let stale = read_code_session_sidecar(&store.path, sid).unwrap();
            assert_eq!(stale.workspace_path.as_deref(), Some(from.as_path()));
        }

        *INJECT_SIDECAR_PERSIST_FAIL_IDS
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        fs::remove_dir_all(&root).unwrap();
    }

    /// review #463 round-15 SF-D: the stranded-index detector's authority
    /// accessor must read the SIDECAR, not the index (round-14 R1's vehicle).
    /// A wiring swap back to code_project_workspace — verbatim the R1 bug —
    /// must fail red.
    #[test]
    fn code_sidecar_workspace_reads_the_sidecar_not_the_index() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-sidecar-accessor-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let sidecar_path = root.join("real-sidecar-target");
        let index_path = root.join("stale-index-target");
        fs::create_dir_all(&sidecar_path).unwrap();
        fs::create_dir_all(&index_path).unwrap();
        store
            .bind_code_native_session(
                "s1",
                CodexWorkspaceKind::Project,
                Some(sidecar_path.clone()),
                Vec::new(),
            )
            .unwrap();
        // Divergence shape: the index record disagrees with the sidecar.
        {
            let mut records = store.records.write();
            records.get_mut("s1").unwrap().workspace_path = Some(index_path.clone());
        }
        assert_eq!(
            store.code_sidecar_workspace("s1").as_deref(),
            Some(sidecar_path.as_path()),
            "the accessor must return the sidecar's path, not the index's"
        );
        assert_eq!(
            store.code_project_workspace("s1").as_deref(),
            Some(index_path.as_path()),
            "precondition: the index genuinely disagrees"
        );
        // A temporary binding carries no project workspace.
        store
            .bind_code_native_session("tmp-1", CodexWorkspaceKind::Temporary, None, Vec::new())
            .unwrap();
        assert_eq!(store.code_sidecar_workspace("tmp-1"), None);
        // An unknown id reads as absent.
        assert_eq!(store.code_sidecar_workspace("never-bound"), None);

        fs::remove_dir_all(&root).unwrap();
    }

    /// review #463 round-15 SF-B: retention deletes purge the SavedSession
    /// record and the session directory WITHOUT touching this index, so an
    /// index record can outlive its owner. The rebind scan and both write
    /// passes gate on the durable owner record: a dead session is never
    /// rewritten, never moved, and never reported — the plain lane's
    /// workspace_binding_owner_exists rule mirrored.
    #[test]
    fn rebind_skips_sessions_whose_owner_record_is_gone() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-rebind-owner-gate-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let from = root.join("from");
        let to = root.join("to");
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&to).unwrap();
        store
            .bind_code_native_session(
                "live",
                CodexWorkspaceKind::Project,
                Some(from.clone()),
                Vec::new(),
            )
            .unwrap();
        store
            .bind_code_native_session(
                "dead",
                CodexWorkspaceKind::Project,
                Some(from.clone()),
                Vec::new(),
            )
            .unwrap();
        touch_owner_record(&store.path, "live");
        touch_owner_record(&store.path, "dead");
        // Retention shape: the record (and the session directory) is gone,
        // the index entry survives.
        fs::remove_file(code_session_sidecar_root(&store.path).join("dead.json")).unwrap();

        let matched = store.sessions_under_workspace(&from);
        assert!(
            matched.iter().all(|(sid, _)| sid != "dead"),
            "the scan must not surface a session whose owner record is gone"
        );
        assert!(matched.iter().any(|(sid, _)| sid == "live"));

        let outcome = store.rebind_workspace_prefix(&from, &to).unwrap();
        assert!(outcome.sidecar_final_stale.is_empty());
        assert!(
            outcome.affected.iter().all(|(sid, _)| sid != "dead"),
            "a dead session is never rewritten or reported as moved"
        );
        assert_eq!(
            store.get("dead").workspace_path.as_deref(),
            Some(from.as_path()),
            "the dead record stays untouched"
        );
        assert_eq!(
            store.get("live").workspace_path.as_deref(),
            Some(to.as_path()),
        );

        fs::remove_dir_all(&root).unwrap();
    }

    /// Round-24 minor 2: a non-NotFound stat error must read as LIVE, not
    /// dead — an EACCES blip must not silently drop a live session from the
    /// scan, both write passes, the fence and the post-lane rescan while
    /// the run reports success. unix-only: the fault is a chmod-0000
    /// sessions directory; the test self-skips when the mode is not
    /// enforced (running as root).
    #[cfg(unix)]
    #[test]
    fn owner_probe_treats_stat_error_as_live() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-owner-probe-eacces-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let dir = code_session_sidecar_root(&store.path);
        fs::create_dir_all(&dir).unwrap();
        touch_owner_record(&store.path, "probe");

        let original_mode = fs::metadata(&dir).unwrap().permissions().mode();
        fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        // The fault must be INDUCED before the probe is meaningful: on a
        // host that does not enforce the mode (running as root) the stat
        // succeeds and nothing can be pinned here — skip via early return,
        // never by treating the un-induced probe as evidence.
        // Stat a file INSIDE the dir: the dir's own mode does not block a
        // stat of the dir itself, but it does block traversal to entries.
        let fault_induced = fs::metadata(&dir.join("probe.json")).is_err();
        let probed = store.binding_owner_exists("probe");
        fs::set_permissions(&dir, std::fs::Permissions::from_mode(original_mode)).unwrap();
        if !fault_induced {
            let _ = fs::remove_dir_all(&root);
            return;
        }
        assert!(
            probed,
            "a stat error (EACCES) must read as live so a transient blip cannot silently drop a live session from the rebind passes",
        );

        // The NotFound arm stays dead: a genuinely removed record is still
        // excluded.
        fs::remove_file(code_session_sidecar_root(&store.path).join("probe.json")).unwrap();
        assert!(!store.binding_owner_exists("probe"));

        let _ = fs::remove_dir_all(&root);
    }

    /// Round-24 minor 6: the write-pass owner gates must be pinned
    /// BEHAVIORALLY — the fixture above deletes the record before the scan,
    /// so a gate added to a write pass was invisible to it (deleting the
    /// gates kept this suite green; the deleted record failed the rewrites
    /// on reads either way). The owner-probe seam arms a session whose
    /// record file stays HEALTHY on disk while the probe reports it dead —
    /// the state a write pass sees when the owner is deleted between the
    /// command layer's scan and the store passes. Red-verified by deleting
    /// the index-rewrite gate (the id's index moved to `to`) and the
    /// sidecar-rewrite gate (its sidecar moved).
    #[test]
    fn rebind_write_passes_skip_owner_deleted_after_scan() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-rebind-owner-gate-midrun-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let from = root.join("from");
        let to = root.join("to");
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&to).unwrap();
        store
            .bind_code_native_session(
                "midrun",
                CodexWorkspaceKind::Project,
                Some(from.clone()),
                Vec::new(),
            )
            .unwrap();

        OWNER_PROBE_DEAD_IDS
            .lock()
            .unwrap()
            .push("midrun".to_string());
        let outcome = store.rebind_workspace_prefix(&from, &to).unwrap();
        OWNER_PROBE_DEAD_IDS.lock().unwrap().clear();

        // The scan saw the owner alive (first probe), so the session was a
        // candidate; both write passes then probed it dead and skipped it.
        assert!(
            outcome.affected.iter().all(|(sid, _)| sid != "midrun"),
            "a session whose owner died after the scan is never reported as moved",
        );
        assert!(
            outcome
                .sidecar_final_stale
                .iter()
                .all(|sid| sid != "midrun"),
            "the skipped session is not a stale sidecar either — the gate declined the write",
        );
        assert_eq!(
            store.get("midrun").workspace_path.as_deref(),
            Some(from.as_path()),
            "the index-rewrite owner gate must decline the dead owner (mutation: deleting the gate moves this to `to`)",
        );
        let sidecar =
            read_code_session_sidecar(&store.path, "midrun").expect("the sidecar survives the run");
        assert_eq!(
            sidecar.workspace_path.as_deref(),
            Some(from.as_path()),
            "the sidecar-rewrite owner gate must decline the dead owner (mutation: deleting the gate moves this to `to`)",
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rebind_prefix_rolls_back_memory_when_index_persist_fails() {
        // review #463 minor: persist() runs after the in-memory index
        // mutation; on failure the memory must be rolled back to the
        // on-disk state, otherwise the process believes the rebind happened
        // while disk says otherwise, and a retry would see no `from`
        // bindings left to move.
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-rebind-rollback-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let from = root.join("from");
        let to = root.join("to");
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&to).unwrap();
        store
            .bind_code_native_session(
                "s1",
                CodexWorkspaceKind::Project,
                Some(from.clone()),
                Vec::new(),
            )
            .unwrap();
        touch_owner_record(&store.path, "s1");

        // Round-21 SF-7 moved the index persist onto atomic_write, whose
        // randomized temp names defeat the old tmp-path occupation. The
        // cross-platform fault: occupy the index file's own path with a
        // DIRECTORY — the tmp write lands beside it, and the rename onto a
        // directory fails on every platform (no #[cfg] needed: this file
        // carries zero target-cfg baseline debt).
        fs::remove_file(&store.path).unwrap();
        fs::create_dir(&store.path).unwrap();
        let error = store
            .rebind_workspace_prefix(&from, &to)
            .expect_err("persist failure surfaces as an error");
        assert!(!error.to_string().is_empty());
        assert_eq!(
            store.get("s1").workspace_path.as_deref(),
            Some(from.as_path()),
            "memory rolled back to the on-disk binding"
        );

        // After clearing the obstacle a retry converges: the original `from`
        // binding is still there to be moved.
        fs::remove_dir(&store.path).unwrap();
        let outcome = store.rebind_workspace_prefix(&from, &to).unwrap();
        assert!(outcome.affected.iter().any(|(sid, _)| sid == "s1"));
        assert_eq!(
            store.get("s1").workspace_path.as_deref(),
            Some(to.as_path())
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn set_session_workspace_roots_rolls_back_memory_when_persist_fails() {
        // review #484 M2: the keychain replacement (align) mutates the
        // in-memory record before persist(); on failure the memory must be
        // rolled back to the on-disk snapshot, otherwise the process reports
        // roots that were never persisted and a retry diff against memory
        // would misjudge the state. Mirrors
        // rebind_prefix_rolls_back_memory_when_index_persist_fails.
        // Hermetic guard (review #484 round-10 M1 convention): the align write
        // now also converges a pre-existing acp-state.json under
        // sessions_root(), which honors PINVOU3_HOME — hold ENV_LOCK and
        // redirect it so this test can never touch a real store.
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev_home = std::env::var("PINVOU3_HOME").ok();
        let hermetic_home = std::env::temp_dir().join(format!(
            "pinvou3-codex-align-rollback-home-{}",
            std::process::id()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &hermetic_home) };
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-align-rollback-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let workspace = root.join("workspace");
        let extra = root.join("extra");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&extra).unwrap();
        store
            .bind_code_native_session(
                "s1",
                CodexWorkspaceKind::Project,
                Some(workspace.clone()),
                vec![workspace.clone()],
            )
            .unwrap();

        // Round-21 SF-7 moved the index persist onto atomic_write, whose
        // randomized temp names defeat tmp-path occupation: occupy the index
        // file's own path with a DIRECTORY — the rename onto it fails on
        // every platform after the in-memory mutation.
        fs::remove_file(&store.path).unwrap();
        fs::create_dir(&store.path).unwrap();
        let error = store
            .set_session_workspace_roots("s1", vec![workspace.clone(), extra.clone()])
            .expect_err("persist failure surfaces as an error");
        assert!(!error.to_string().is_empty());
        assert_eq!(
            store.session_workspace_roots("s1"),
            vec![workspace.clone()],
            "memory rolled back to the on-disk keychain"
        );

        // After clearing the obstacle a retry persists the same replacement.
        fs::remove_dir_all(&store.path).unwrap();
        let retried = store
            .set_session_workspace_roots("s1", vec![workspace.clone(), extra.clone()])
            .expect("retry persists");
        assert!(retried.applied);
        assert_eq!(
            retried.sidecar_committed,
            Some(true),
            "a healthy code-session rewrite reports the sidecar as committed"
        );
        assert_eq!(
            store.session_workspace_roots("s1"),
            vec![workspace.clone(), extra.clone()]
        );

        // skipped lanes: unknown session and a temporary (unbound) record
        // write nothing and never touch the disk.
        let ghost = store
            .set_session_workspace_roots("ghost", vec![workspace.clone()])
            .expect("unknown session");
        assert!(!ghost.applied);
        assert_eq!(ghost.sidecar_committed, None);
        store
            .bind_code_native_session("temp", CodexWorkspaceKind::Temporary, None, Vec::new())
            .unwrap();
        let temporary = store
            .set_session_workspace_roots("temp", vec![workspace])
            .expect("temporary session has no keychain");
        assert!(!temporary.applied);

        fs::remove_dir_all(&root).unwrap();
        // SAFETY: ENV_LOCK held for the whole test; restoring the caller's
        // environment.
        unsafe {
            match prev_home {
                Some(home) => std::env::set_var("PINVOU3_HOME", home),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
        }
    }

    #[test]
    fn set_session_workspace_roots_reports_a_failed_acp_state_convergence() {
        // Round-13 M3: a Project-kind ACP record with a pre-existing (here:
        // deliberately unwritable) acp-state.json must surface the failed
        // convergence as state_committed = Some(false). Swallowing it would
        // report a narrowing align as clean while the durable ACP copy keeps
        // the wider pre-align set that acp_recovery_record reads back after a
        // session-agents.json loss. Hermetic under ENV_LOCK + PINVOU3_HOME:
        // the state path resolves under the redirected home.
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev_home = std::env::var("PINVOU3_HOME").ok();
        let hermetic_home = std::env::temp_dir().join(format!(
            "pinvou3-codex-state-converge-home-{}",
            std::process::id()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &hermetic_home) };
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-state-converge-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let workspace = root.join("workspace");
        let extra = root.join("extra");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&extra).unwrap();
        store
            .set_acp_workspace(
                "s-acp",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(workspace.clone()),
                vec![workspace.clone()],
            )
            .unwrap();
        // The pre-existing state artifact as an unwritable directory: spawn
        // owns its creation, so a pre-existing-but-unusable copy is exactly
        // the convergence-failure shape.
        let state_dir = hermetic_home
            .join("sessions")
            .join("s-acp")
            .join("acp-state.json");
        fs::create_dir_all(&state_dir).unwrap();

        let write = store
            .set_session_workspace_roots("s-acp", vec![workspace.clone(), extra.clone()])
            .expect("the index commits");
        assert!(write.applied);
        assert_eq!(write.state_committed, Some(false), "{write:?}");
        assert_eq!(write.sidecar_committed, None, "ACP records have no sidecar");

        fs::remove_dir_all(&root).unwrap();
        // SAFETY: ENV_LOCK held for the whole test; restoring the caller's
        // environment.
        unsafe {
            match prev_home {
                Some(home) => std::env::set_var("PINVOU3_HOME", home),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
        }
    }

    #[test]
    fn set_session_workspace_roots_reports_a_corrupt_state_reset_as_failure() {
        // Round-20 SF1: an unparseable acp-state.json used to be silently
        // reset to {} and the convergence reported Some(true) — plain-success
        // toast while the durable artifact lost its adapter/session sections.
        // The reset still happens (recovering a writable state is right), but
        // the align outcome must surface it as sidecar_stale, so the write
        // reports Some(false) here too.
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev_home = std::env::var("PINVOU3_HOME").ok();
        let hermetic_home = std::env::temp_dir().join(format!(
            "pinvou3-codex-state-corrupt-home-{}",
            std::process::id()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &hermetic_home) };
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-state-corrupt-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let workspace = root.join("workspace");
        let extra = root.join("extra");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&extra).unwrap();
        store
            .set_acp_workspace(
                "s-acp",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(workspace.clone()),
                vec![workspace.clone()],
            )
            .unwrap();
        // A corrupt (unparseable) pre-existing state artifact: spawn owns
        // creation, so a corrupt copy is the reset-not-commit shape.
        let state_dir = hermetic_home
            .join("sessions")
            .join("s-acp")
            .join("acp-state.json");
        fs::create_dir_all(state_dir.parent().unwrap()).unwrap();
        fs::write(&state_dir, b"{ not json").unwrap();

        let write = store
            .set_session_workspace_roots("s-acp", vec![workspace.clone(), extra.clone()])
            .expect("the index commits");
        assert!(write.applied);
        assert_eq!(
            write.state_committed,
            Some(false),
            "a corrupt-state reset must surface as a convergence failure: {write:?}"
        );
        assert!(
            state_dir.is_file(),
            "the reset artifact was written back (recovery happened, only the report changed)"
        );

        fs::remove_dir_all(&root).unwrap();
        let _ = fs::remove_dir_all(&hermetic_home);
        // SAFETY: ENV_LOCK held for the whole test; restoring the caller's
        // environment.
        unsafe {
            match prev_home {
                Some(home) => std::env::set_var("PINVOU3_HOME", home),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
        }
    }

    #[test]
    fn set_session_workspace_roots_converges_acp_state_content() {
        // Round-21 should-fix 7: the happy path of the acp-state.json
        // convergence was never read back (only the failure legs were
        // pinned) — pin that a valid pre-existing state keeps its sibling
        // sections, reports Some(true), and lands the exact workspace
        // snapshot (kind/path/roots) the align lane promises.
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev_home = std::env::var("PINVOU3_HOME").ok();
        let hermetic_home = std::env::temp_dir().join(format!(
            "pinvou3-codex-state-happy-home-{}",
            std::process::id()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &hermetic_home) };
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-state-happy-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let workspace = root.join("workspace");
        let extra = root.join("extra");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&extra).unwrap();
        store
            .set_acp_workspace(
                "s-acp",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(workspace.clone()),
                vec![workspace.clone()],
            )
            .unwrap();
        let state_dir = hermetic_home
            .join("sessions")
            .join("s-acp")
            .join("acp-state.json");
        fs::create_dir_all(state_dir.parent().unwrap()).unwrap();
        fs::write(
            &state_dir,
            serde_json::to_vec_pretty(&serde_json::json!({
                "adapter": { "name": "codex" },
                "session": { "role": "durable" },
                "workspace": { "path": workspace.to_string_lossy() }
            }))
            .unwrap(),
        )
        .unwrap();

        let write = store
            .set_session_workspace_roots("s-acp", vec![workspace.clone(), extra.clone()])
            .expect("the index commits");
        assert!(write.applied);
        assert_eq!(write.state_committed, Some(true), "{write:?}");

        let state: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_dir).expect("read converged state"))
                .expect("parse converged state");
        assert_eq!(
            state["workspace"]["roots"],
            serde_json::json!([
                workspace.to_string_lossy().to_string(),
                extra.to_string_lossy().to_string()
            ]),
            "the durable artifact carries the aligned keychain"
        );
        // kebab-case serde on CodexWorkspaceKind.
        assert_eq!(state["workspace"]["kind"], serde_json::json!("project"));
        assert_eq!(
            state["workspace"]["path"],
            serde_json::json!(workspace.to_string_lossy().to_string())
        );
        assert_eq!(
            state["adapter"],
            serde_json::json!({ "name": "codex" }),
            "sibling sections survive the convergence patch"
        );
        assert_eq!(
            state["session"],
            serde_json::json!({ "role": "durable" }),
            "sibling sections survive the convergence patch"
        );

        fs::remove_dir_all(&root).unwrap();
        let _ = fs::remove_dir_all(&hermetic_home);
        // SAFETY: ENV_LOCK held for the whole test; restoring the caller's
        // environment.
        unsafe {
            match prev_home {
                Some(home) => std::env::set_var("PINVOU3_HOME", home),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
        }
    }

    #[test]
    fn recovered_acp_record_is_persisted_atomically() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-recovery-store-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session-agents.json");
        let store = SessionAgentStore {
            path: path.clone(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .restore_missing_acp_record(
                "session-1",
                SessionAgentRecord {
                    backend: AgentBackend::ClaudeAcp,
                    acp_session_id: Some("acp-session-1".to_string()),
                    acp_model_id: Some("gpt-test".to_string()),
                    acp_mode_id: Some("agent".to_string()),
                    acp_config_values: HashMap::from([(
                        "reasoning_effort".to_string(),
                        "high".to_string(),
                    )]),
                    workspace_kind: CodexWorkspaceKind::Project,
                    workspace_path: Some(root.clone()),
                    workspace_roots: Vec::new(),
                    explicit_ancestor_roots: Vec::new(),
                    mode: SessionMode::Plain,
                },
                || true,
            )
            .unwrap();

        let persisted: AgentStoreFile = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let recovered = &persisted.sessions["session-1"];
        assert_eq!(recovered.backend, AgentBackend::ClaudeAcp);
        assert_eq!(recovered.acp_session_id.as_deref(), Some("acp-session-1"));
        assert_eq!(recovered.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(recovered.workspace_path.as_deref(), Some(root.as_path()));
        assert_eq!(
            recovered.acp_config_values.get("reasoning_effort"),
            Some(&"high".to_string())
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// Round-44 review: a `sessions delete` landing between the boot pass's
    /// recovery read and the section lock removes the acp-state source; the
    /// restore must report "nothing recovered" instead of minting an orphan
    /// ACP index record for the deleted session.
    #[test]
    fn restore_missing_acp_record_refuses_when_the_source_vanished_under_the_lock() {
        let (root, store) = store_fixture("pinvou3-acp-restore-source-gone-test-");
        let path = store.path.clone();
        let recovered = || SessionAgentRecord {
            backend: AgentBackend::ClaudeAcp,
            acp_session_id: Some("acp-session-1".to_string()),
            ..Default::default()
        };
        assert_eq!(
            store
                .restore_missing_acp_record("session-1", recovered(), || false)
                .unwrap(),
            false,
            "a vanished recovery source must not restore"
        );
        assert!(
            !path.exists(),
            "a refused restore must not persist anything: {:?}",
            fs::read_to_string(&path)
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn generic_acp_config_is_persisted_with_the_session_record() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-acp-config-store-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session-agents.json");
        let store = SessionAgentStore {
            path: path.clone(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .set_acp_config_value("session-1", "reasoning_effort", "high")
            .unwrap();

        let persisted: AgentStoreFile = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(
            persisted.sessions["session-1"].acp_config_values["reasoning_effort"],
            "high"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn acp_defaults_are_isolated_by_agent_and_not_overwritten_by_migration() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-acp-defaults-store-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("acp-agent-defaults.json");
        let store = AcpConfigDefaultsStore {
            path: path.clone(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .set(AgentBackend::CodexAcp, "mode", "agent-full-access")
            .unwrap();
        store
            .set(AgentBackend::ClaudeAcp, "mode", "default")
            .unwrap();
        assert!(
            !store
                .set_all_if_absent(
                    AgentBackend::CodexAcp,
                    HashMap::from([("mode".to_string(), "agent".to_string())]),
                )
                .unwrap()
        );

        let persisted: AcpConfigDefaultsFile =
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(
            persisted.agents[&AgentBackend::CodexAcp]["mode"],
            "agent-full-access"
        );
        assert_eq!(
            persisted.agents[&AgentBackend::ClaudeAcp]["mode"],
            "default"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_binding_writes_authoritative_sidecar() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-sidecar-write-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session(
                "session-1",
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        let sidecar = read_code_session_sidecar(store.path(), "session-1")
            .expect("sidecar should exist after binding");
        assert_eq!(sidecar.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(sidecar.workspace_path.as_deref(), Some(root.as_path()));
        assert!(sidecar.bound_at.is_some());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_sidecar_recovers_missing_index_record() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-sidecar-recover-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session(
                "session-1",
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        // 模拟辅助索引丢失：清空记录并从磁盘重建（empty store）。
        let recovered_store = SessionAgentStore {
            path: store.path().to_path_buf(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        assert!(!recovered_store.is_code_session("session-1"));
        recovered_store
            .restore_missing_code_session_record("session-1")
            .unwrap();
        let record = recovered_store.get("session-1");
        assert!(record.mode.is_code());
        assert_eq!(record.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(record.workspace_path.as_deref(), Some(root.as_path()));
        // 恢复持久化回索引文件，后续读取不再依赖 sidecar。
        let persisted: AgentStoreFile =
            serde_json::from_slice(&fs::read(recovered_store.path()).unwrap()).unwrap();
        assert!(persisted.sessions["session-1"].mode.is_code());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_sidecar_restore_rejects_acp_owned_session() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-sidecar-acp-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session(
                "session-1",
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        // ACP 会话已占用该 session：恢复必须拒绝，不能覆盖。
        store
            .set_acp_workspace(
                "session-1",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        // Round-43 review: `set_acp_workspace` cleans the sidecar on success;
        // the ACP-ownership refusal exists for the residual left behind when
        // that cleanup failed (the boot scan cleans it later), so re-create
        // the sidecar to exercise the refusal against the fresh table.
        assert!(write_code_session_sidecar(
            store.path(),
            "session-1",
            CodexWorkspaceKind::Project,
            Some(root.clone()),
            Vec::new(),
            Vec::new(),
        ));
        assert!(
            store
                .restore_missing_code_session_record("session-1")
                .is_err()
        );
        assert!(store.get("session-1").backend.is_acp());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn remove_cleans_up_code_native_sidecar() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-sidecar-remove-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Temporary, None, Vec::new())
            .unwrap();
        assert!(read_code_session_sidecar(store.path(), "session-1").is_some());
        store.remove("session-1").unwrap();
        assert!(read_code_session_sidecar(store.path(), "session-1").is_none());
        assert!(!store.is_code_session("session-1"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn remove_retries_durable_cleanup_after_a_failed_persist() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-remove-persist-retry-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Temporary, None, Vec::new())
            .unwrap();
        store
            .bind_code_native_session("session-2", CodexWorkspaceKind::Temporary, None, Vec::new())
            .unwrap();

        // Review #690 (discussion_r4258007878): a persist failure between
        // the in-memory removal and the durable rewrite leaves exactly this
        // residue — the map already dropped the entry while the index file
        // and the sidecar still hold it. The failing write itself is not
        // fault-injected here (the fault boundary lives below the store);
        // the residue state it leaves is arranged directly. The next
        // remove — the purge flow's retry — must not accept memory absence
        // as durable success.
        store.records.write().remove("session-1");
        assert!(read_code_session_sidecar(store.path(), "session-1").is_some());
        store.remove("session-1").unwrap();

        // The retry performed the full durable cleanup: the reloaded index
        // no longer holds the removed entry ...
        let index: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert!(index["sessions"].get("session-1").is_none());
        assert!(read_code_session_sidecar(store.path(), "session-1").is_none());
        // ... and the unaffected survivor is intact on disk and in memory.
        assert!(index["sessions"].get("session-2").is_some());
        assert!(store.is_code_session("session-2"));
        assert!(read_code_session_sidecar(store.path(), "session-2").is_some());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_sidecar_rejects_malformed_kind_path_combination() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-sidecar-malformed-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        // Round-44 review: the restore validates the combination it reads
        // from the on-disk sidecar (the passed-in copy is gone from the
        // signature), so the malformed fixtures must be written to disk —
        // a corrupt/foreign-written sidecar is exactly what the guard
        // protects against now.
        // 项目 kind 但缺路径：恢复必须拒绝。
        assert!(write_code_session_sidecar(
            store.path(),
            "session-1",
            CodexWorkspaceKind::Project,
            None,
            Vec::new(),
            Vec::new(),
        ));
        assert!(
            store
                .restore_missing_code_session_record("session-1")
                .is_err()
        );
        // 临时 kind 但带路径：恢复必须拒绝。
        assert!(write_code_session_sidecar(
            store.path(),
            "session-2",
            CodexWorkspaceKind::Temporary,
            Some(root.clone()),
            Vec::new(),
            Vec::new(),
        ));
        assert!(
            store
                .restore_missing_code_session_record("session-2")
                .is_err()
        );
        assert!(!store.is_code_session("session-1"));
        assert!(!store.is_code_session("session-2"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn set_acp_workspace_forces_empty_keychain_for_temporary_sessions() {
        // review #484 m3: a Temporary record must carry an EMPTY keychain
        // (single-root semantics), matching bind_code_native_session — before
        // the alignment, a Temporary ACP session bound with a non-empty
        // workspace_roots payload stored it verbatim, contradicting the
        // session_workspace_roots contract ("temporary session = empty").
        let root = std::env::temp_dir().join(format!(
            "pinvou3-acp-temporary-keychain-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .set_acp_workspace(
                "session-1",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Temporary,
                None,
                vec![root.clone()],
            )
            .unwrap();
        assert!(
            store.session_workspace_roots("session-1").is_empty(),
            "a temporary ACP session never stores a keychain snapshot"
        );

        // The Project arm is unchanged: the full creation-time snapshot rides.
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        store
            .set_acp_workspace(
                "session-2",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(workspace.clone()),
                vec![workspace.clone(), root.clone()],
            )
            .unwrap();
        assert_eq!(
            store.session_workspace_roots("session-2"),
            vec![workspace, root.clone()]
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn project_keychain_persists_cwd_first_and_deduped() {
        // review #484 round-5 M2: the create channel sends storage-order
        // roots plus a separate cwd; both agent-lane bind entry points must
        // persist the base-normalized shape (cwd first, the rest in order,
        // duplicates removed) so the stored order matches the effective one.
        let root = std::env::temp_dir().join(format!(
            "pinvou3-acp-cwd-first-keychain-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let workspace = root.join("workspace");
        let extra = root.join("extra");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&extra).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };

        // ACP lane: storage order [extra, workspace] with cwd = workspace.
        store
            .set_acp_workspace(
                "session-acp",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(workspace.clone()),
                vec![extra.clone(), workspace.clone(), extra.clone()],
            )
            .unwrap();
        assert_eq!(
            store.session_workspace_roots("session-acp"),
            vec![workspace.clone(), extra.clone()],
            "cwd is promoted to the primary slot and duplicates collapse"
        );

        // Native code lane: cwd already in slot 0 stays unchanged.
        store
            .bind_code_native_session(
                "session-native",
                CodexWorkspaceKind::Project,
                Some(workspace.clone()),
                vec![workspace.clone(), extra.clone()],
            )
            .unwrap();
        assert_eq!(
            store.session_workspace_roots("session-native"),
            vec![workspace.clone(), extra.clone()],
            "an already cwd-first keychain is persisted unchanged"
        );

        // Empty stays empty (single-root contract), for both lanes.
        store
            .set_acp_workspace(
                "session-empty",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(workspace.clone()),
                Vec::new(),
            )
            .unwrap();
        assert!(store.session_workspace_roots("session-empty").is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_restore_reports_real_recovery_only() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-restore-count-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session(
                "session-1",
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        // 模拟辅助索引丢失后的首次恢复：真实恢复，返回 true。Round-43 review:
        // the restore lane now reloads the disk table under the section lock,
        // so the lost-index premise must hold on disk (record removed from
        // the file, authoritative sidecar kept).
        let mut lost_index: AgentStoreFile =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        lost_index.sessions.remove("session-1");
        fs::write(store.path(), serde_json::to_string(&lost_index).unwrap()).unwrap();
        let recovered_store = SessionAgentStore {
            path: store.path().to_path_buf(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        assert!(
            recovered_store
                .restore_missing_code_session_record("session-1")
                .unwrap()
        );
        // 索引已完好：再次调用是早退，不得被误计为恢复信号。
        assert!(
            !recovered_store
                .restore_missing_code_session_record("session-1")
                .unwrap()
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_code_native_sidecar_is_backfilled_from_index() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-sidecar-backfill-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session(
                "session-1",
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        // session-1's durable record is still present (not evicted by
        // retention): a missing sidecar is still backfilled from the index.
        touch_owner_record(store.path(), "session-1");
        // 非代码会话不参与回填。
        store
            .set_acp_workspace(
                "session-2",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Temporary,
                None,
                Vec::new(),
            )
            .unwrap();
        // Replicate the leftover shape after a retention eviction: the index
        // says code_session=true, but both the durable record <id>.json and
        // the session dir <id>/ are gone (retention cleanup never touches
        // the agents index).
        store
            .bind_code_native_session(
                "session-3",
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        fs::remove_dir_all(code_session_sidecar_root(store.path()).join("session-3")).unwrap();
        // 模拟存量会话/绑定时写失败：索引记录 code_session=true 但 sidecar 缺失。
        fs::remove_file(code_session_sidecar_path(store.path(), "session-1")).unwrap();
        // Only session-1, whose durable record is still present, is
        // backfilled; session-3 is a ghost entry and must be skipped — no
        // rebuilt dir under sessions/<id>/, no fabricated sidecar (the very
        // source of the boot-time mass resurrection of evicted sessions).
        // Removing the leftover entry belongs to the boot scan's prune.
        assert_eq!(store.backfill_missing_code_session_sidecars().unwrap(), 1);
        let sidecar = read_code_session_sidecar(store.path(), "session-1")
            .expect("sidecar should be backfilled");
        assert_eq!(sidecar.version, CODE_SESSION_SIDECAR_VERSION);
        assert_eq!(sidecar.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(sidecar.workspace_path.as_deref(), Some(root.as_path()));
        // The ghost entry stays dead: no sidecar, no resurrected session dir.
        assert!(
            !code_session_sidecar_root(store.path())
                .join("session-3")
                .exists()
        );
        assert!(read_code_session_sidecar(store.path(), "session-3").is_none());
        // Idempotent: intact sidecars, non-code sessions and ghost entries
        // are all left alone.
        assert_eq!(store.backfill_missing_code_session_sidecars().unwrap(), 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn backfill_skips_a_retention_purged_session() {
        // Round-33 MAJOR-2, ghost leg: an index record whose owner marker is
        // gone must NOT be backfilled — the write's create_dir_all would
        // resurrect the purged directory at every boot. Deleting the owner
        // gate reds this.
        let root = std::env::temp_dir().join(format!(
            "pinvou3-backfill-ghost-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session(
                "s-ghost",
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        fs::remove_file(code_session_sidecar_path(&store.path, "s-ghost")).unwrap();
        // NO owner marker seeded: retention-purged.
        assert_eq!(
            store.backfill_missing_code_session_sidecars().unwrap(),
            0,
            "the purged session is not backfilled"
        );
        assert!(
            !code_session_sidecar_path(&store.path, "s-ghost").exists(),
            "no ghost sidecar was written"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_sidecar_rejects_newer_schema_version() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-sidecar-version-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session(
                "session-1",
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        // Durable record still present: only an old-version sidecar treated
        // as missing goes through the backfill rewrite.
        touch_owner_record(store.path(), "session-1");
        // 写入高于当前支持版本的 sidecar：拒读并按缺失处理，不能静默按 v1 解析。
        let future = CodeSessionSidecar {
            version: CODE_SESSION_SIDECAR_VERSION + 1,
            workspace_kind: CodexWorkspaceKind::Project,
            workspace_path: Some(root.join("future-workspace")),
            workspace_roots: Vec::new(),
            explicit_ancestor_roots: Vec::new(),
            bound_at: None,
        };
        fs::write(
            code_session_sidecar_path(store.path(), "session-1"),
            serde_json::to_vec(&future).unwrap(),
        )
        .unwrap();
        assert!(read_code_session_sidecar(store.path(), "session-1").is_none());
        // Treated-as-missing → the backfill self-heals by rewriting from
        // the index. Round-31 minor 5: seed the live owner the backfill's
        // gate stats first.
        fs::create_dir_all(root.join("sessions")).unwrap();
        fs::write(root.join("sessions").join("session-1.json"), b"{}").unwrap();
        // 按缺失处理 → 回填自愈按索引重写为当前版本。
        assert_eq!(store.backfill_missing_code_session_sidecars().unwrap(), 1);
        let sidecar = read_code_session_sidecar(store.path(), "session-1").unwrap();
        assert_eq!(sidecar.version, CODE_SESSION_SIDECAR_VERSION);
        assert_eq!(sidecar.workspace_path.as_deref(), Some(root.as_path()));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn index_schema_gate_covers_load_and_both_reload_paths() {
        // Round-46 M2: a newer-schema index must be refused by EVERY reader —
        // load() and both reload paths. Historically only load() checked; the
        // reload paths adopted the table and the next whole-table persist
        // stamped it with the CURRENT version, a silent schema downgrade that
        // defeated the gate's refusing-to-overwrite purpose.
        let root = std::env::temp_dir().join(format!(
            "pinvou3-index-schema-gate-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session(
                "session-1",
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        // 同一索引文件仅把版本戳改为未来值:三门都必须因版本门拒绝,而非解析失败。
        let raw = fs::read_to_string(store.path()).unwrap();
        let mut bumped: serde_json::Value = serde_json::from_str(&raw).unwrap();
        bumped["version"] = serde_json::json!(STORE_VERSION + 1);
        fs::write(store.path(), bumped.to_string()).unwrap();

        assert!(
            SessionAgentStore::load_from(store.path().to_path_buf()).is_err(),
            "load must refuse a newer-schema index"
        );
        assert!(
            store.lock_and_reload().is_err(),
            "lock_and_reload must refuse instead of adopting the newer table"
        );
        let before = fs::read_to_string(store.path()).unwrap();
        assert!(
            store.remove_for_second_process("session-1").is_err(),
            "the CLI removal lane must refuse a newer-schema index"
        );
        assert_eq!(
            fs::read_to_string(store.path()).unwrap(),
            before,
            "a refused removal leaves the index untouched"
        );

        // 版本戳回到当前值后三门全部通过——版本门是唯一拒因。
        let mut current: serde_json::Value = serde_json::from_str(&raw).unwrap();
        current["version"] = serde_json::json!(STORE_VERSION);
        fs::write(store.path(), current.to_string()).unwrap();
        assert!(SessionAgentStore::load_from(store.path().to_path_buf()).is_ok());
        assert!(store.lock_and_reload().is_ok());
        assert!(store.remove_for_second_process("session-1").is_ok());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn failed_index_persist_keeps_code_native_sidecar() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-persist-order-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session-agents.json");
        let store = SessionAgentStore {
            path: path.clone(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session(
                "session-1",
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        assert!(read_code_session_sidecar(&path, "session-1").is_some());
        // 让索引 persist 必失败：索引路径被同名目录占用，rename 无法覆盖。
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(
            store
                .set_acp_workspace(
                    "session-1",
                    AgentBackend::CodexAcp,
                    CodexWorkspaceKind::Temporary,
                    None,
                    Vec::new(),
                )
                .is_err()
        );
        // persist 先失败则 sidecar 不得先删，与磁盘索引（仍是绑定时的内容）保持一致。
        let sidecar = read_code_session_sidecar(&path, "session-1")
            .expect("sidecar must survive failed index persist");
        assert_eq!(sidecar.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(sidecar.workspace_path.as_deref(), Some(root.as_path()));
        fs::remove_dir_all(&root).unwrap();
    }

    /// Round-32 minor 7: the codex lane's three keychain-bearing artifacts
    /// persist 0600 (the plain lane pins the same property for its sidecar
    /// — the sibling this PR itself regressed acp-state.json against before
    /// round-31 minor 8).
    #[cfg(unix)]
    #[test]
    fn codex_keychain_artifacts_persist_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "pinvou3-keychain-private-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session(
                "s-private",
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        // POSIX mode bits are unix-only at COMPILE time (PermissionsExt,
        // imported above); the whole test is cfg(unix)-gated for that
        // reason (round-32 follow-up: a runtime-gated spelling failed the
        // Windows test build — the import itself does not exist there).
        let mode = |path: &std::path::Path| fs::metadata(path).unwrap().permissions().mode();
        let index_mode = mode(&store.path);
        let sidecar_mode = mode(&code_session_sidecar_path(&store.path, "s-private"));
        assert_eq!(
            index_mode & 0o777,
            0o600,
            "session-agents.json carries the keychain snapshot"
        );
        assert_eq!(
            sidecar_mode & 0o777,
            0o600,
            "code-session.json is the authoritative native keychain artifact"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
