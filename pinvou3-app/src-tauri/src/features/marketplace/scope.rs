//! 包 id × SessionMode 的单一禁用集（`~/.pinvou3/disabled_bundles.json`）。
//!
//! 这是「工具市场统一治理」scope 收敛的单一真相源：取代原先
//! `disabled_connectors.json`（连接器 id）与 `disabled_skills.json`（技能 id）两份
//! 文件。开关粒度收敛为**包 id**（= `bundle.rs` 里 `BundleInfo.id`，即 MCP 工具 id /
//! 技能 id / CLI 连接器 id），一个包 = 一个开关，包内技能（companion skills）可见性
//! 唯一跟随所属包（§5.2 不变量）。
//!
//! 落盘格式与 #287 泛化后的两份旧文件同构：`{scopes: {"<mode>": [...]},
//! "initialized": ["<mode>"], project_skills_enabled}`，scope 键即 `SessionMode` 的
//! kebab-case 名。首个版本读取时把两份旧文件迁移到本文件（读到即迁移）：
//! 旧连接器 id 原样进包 id（连接器 id 即包 id）；旧技能 id 经 `bundle::skill_owner_package`
//! 映射到所属包（companion → MCP/CLI 包，独立技能 → 自身）；`skill:` 前缀跨文件借道
//! 残留统一剥除并清出连接器文件。迁移幂等，失败回退默认值（安全兜底）。
//!
//! 依赖方向：本模块与 `bundle` / `skill_marketplace` 同属 marketplace 领域，只依赖
//! `platform::paths` 与 marketplace 内既有类型，不反向依赖 assistant 运行时。

use std::path::PathBuf;
use std::sync::Mutex;

use crate::core::session_mode::{PackDefaultPolicy, SessionMode};
use crate::features::marketplace::bundle::{
    builtin_cli_bundle_ids, bundle_installed, skill_owner_package,
};
use crate::features::marketplace::skill_marketplace::SkillMarketplaceManager;
use crate::features::marketplace::{ConnectorScope, MarketplaceManager};
use crate::platform::paths;

/// 按模式 scope 键控的禁用**包**列表（包 id 集合）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DisabledBundlesFile {
    /// scope（模式 kebab-case 名）→ 该 scope 被禁用（开关关）的包 id 列表。
    #[serde(default)]
    pub scopes: std::collections::BTreeMap<String, Vec<String>>,
    /// scope → 该 scope 被「不可见」（可见性过滤，从 composer 列表消失）的包 id 列表。
    /// 与 `scopes`（开关）正交：开关控制 on/off，可见性控制是否出现在列表。
    #[serde(default)]
    pub hidden_scopes: std::collections::BTreeMap<String, Vec<String>>,
    /// 已被用户显式初始化（改过开关）的 scope 集合。
    #[serde(default)]
    pub initialized: std::collections::BTreeSet<String>,
    /// Whether project-level skills are enabled (default off; effective only
    /// when the session is bound to a project/work directory — not code-only;
    /// see skill_materialization.rs). Moved here with the skills side.
    #[serde(default)]
    pub project_skills_enabled: bool,
    /// 未知键原样保留（前向兼容）。
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

fn disabled_bundles_path() -> PathBuf {
    paths::pinvou3_home().join("disabled_bundles.json")
}

/// In-process serialization for the `disabled_bundles.json` read-modify-write.
///
/// #515: an in-process mutex alone cannot stop cross-process races — the GUI
/// and headless hosts can share one `~/.pinvou3` home and both install/toggle
/// packs, so two concurrent load→save sections silently drop each other's
/// writes (a lost update; the lost side is the user's explicit off, which is
/// fail-open on the DenyAll gate). Every write critical section must go
/// through `with_scope_file_lock`: take this mutex first, then the OS-level
/// file lock (flock / LockFileEx via `fd-lock`, the same primitive and crate
/// as the remote-control process-ownership lock). Each acquisition opens a
/// fresh file, so the OS lock actually excludes other threads of this process
/// too; the in-process mutex stays in front of it so the read path's
/// `try_write` can only ever be beaten by a *peer* process, and so a write's
/// load→modify→save is exclusive before the OS lock is even attempted.
static DISABLED_BUNDLES_FILE_LOCK: Mutex<()> = Mutex::new(());

/// Cross-process lock file path (same directory as the data file; holds no
/// user data).
fn disabled_bundles_lock_path() -> PathBuf {
    paths::pinvou3_home().join("disabled_bundles.lock")
}

/// Opens (creating if missing) the cross-process lock file. Shared by the
/// blocking write path and the try-lock read path.
fn open_scope_lock_file() -> Result<std::fs::File, String> {
    let lock_path = disabled_bundles_lock_path();
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .map_err(|error| format!("open {}: {error}", lock_path.display()))
}

/// Runs a write critical section `f` while holding the combined in-process
/// mutex + OS file lock that serializes every `disabled_bundles.json`
/// load→save against the peer process (GUI / headless sharing the home)
/// (#515). Fallible, not non-blocking: the wait is unbounded by design.
///
/// Returns `Err` when cross-process serialization cannot be established (the
/// lock file cannot be opened or locked, e.g. a filesystem without lock
/// support). Callers must then refuse the read-modify-write instead of running
/// it unsynchronized — an unlocked RMW is exactly the cross-process lost
/// update this module guards against.
///
/// Blocking has no timeout (fd-lock v4 has no timeout API): the OS releases
/// the lock when the peer process exits or crashes (flock / LockFileEx die
/// with the fd), but a frozen peer (SIGSTOP / debugger) makes this process
/// wait indefinitely. The critical section is a local JSON read-modify-write
/// (the widest variant, the connector-switch sync, additionally enumerates
/// installed ids), so that fail-stop hang (frozen peer only) is accepted over
/// a fail-open lost update. Hot readers are immune to that hang via
/// `try_lock` degradation (see `load_disabled_bundles_file`).
///
/// Lock order is uniform module-wide: in-process mutex → OS file lock, and
/// the only other lock reachable inside a critical section is the bundle
/// store's own mutex (via `resolve_scope_disabled_ids` → installed-ids
/// enumeration). That ordering is never reversed — no store method enters
/// this module's critical sections — so no deadlock class exists.
fn with_scope_file_lock<F, R>(f: F) -> Result<R, String>
where
    F: FnOnce() -> R,
{
    let _process_guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let file = open_scope_lock_file()?;
    let mut lock = fd_lock::RwLock::new(file);
    // The in-process mutex is already held while the OS lock is taken, and
    // the store mutex (see the lock-order note above) is never held by
    // another thread waiting on this one, so deadlock is impossible. A
    // signal-interrupted flock retries instead of surfacing as a spurious
    // write refusal.
    let _os_guard = loop {
        match lock.write() {
            Ok(guard) => break guard,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(format!(
                    "lock {}: {error}",
                    disabled_bundles_lock_path().display()
                ));
            }
        }
    };
    Ok(f())
}

/// Loads the file for policy reads. Bounded by construction against *both*
/// contention dimensions: the in-process mutex is only *tried* (a local write
/// parked on a frozen peer's OS lock holds it, and hot readers must not hang
/// behind that), and the OS lock is only *tried* as well. When either is
/// unavailable the read degrades to a never-persisting unlocked snapshot —
/// `write_atomic` replaces the file atomically, so the snapshot is always a
/// complete (possibly just-superseded) state, and the next uncontended read
/// converges the file.
///
/// When the read runs fully locked, a read-time repair (legacy migration,
/// `skill:` strip) persists serialized with writers.
///
/// The bounded degrade is what keeps the engine-side hot readers (per-turn
/// inventory reminders, deny rulesets, engine spawn config) safe to call
/// directly: unlike the write path no operation needs to be kept off the
/// Tokio executor for them. Contention on either lock is a normal, silent
/// degradation; an unexpected error (unreadable or corrupt data file, broken
/// lock probe) is logged and degrades to the default state rather than
/// fabricating a snapshot that a later write could persist over the user's
/// real state.
pub(crate) fn load_disabled_bundles_file() -> DisabledBundlesFile {
    let _process_guard = match DISABLED_BUNDLES_FILE_LOCK.try_lock() {
        Ok(guard) => guard,
        // A local writer is inside its critical section (possibly parked on a
        // frozen peer's OS lock): degrade exactly like peer contention.
        Err(std::sync::TryLockError::WouldBlock) => {
            return read_disabled_bundles_file_degraded();
        }
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
    };
    match open_scope_lock_file() {
        Ok(file) => match fd_lock::RwLock::new(file).try_write() {
            Ok(_guard) => match load_disabled_bundles_file_locked() {
                Ok(file) => file,
                Err(error) => {
                    log_scope_read_failure(LOG_READ_DATA, &error);
                    DisabledBundlesFile::default()
                }
            },
            // Peer contention is the designed, silent degradation.
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                read_disabled_bundles_file_degraded()
            }
            Err(error) => {
                log_scope_read_failure(
                    LOG_LOCK_PROBE,
                    &format!("unlocked read without persist: {error}"),
                );
                read_disabled_bundles_file_degraded()
            }
        },
        Err(error) => {
            log_scope_read_failure(
                LOG_LOCK_OPEN,
                &format!("unlocked read without persist: {error}"),
            );
            read_disabled_bundles_file_degraded()
        }
    }
}

/// The bounded degrade: unlocked, never-persisting read of the data file.
/// A corrupt or unreadable file degrades loudly to the default state —
/// never to a fabricated snapshot, and never with a write: uninitialized
/// DenyAll scopes then re-derive the fail-closed full-deny default.
fn read_disabled_bundles_file_degraded() -> DisabledBundlesFile {
    match read_disabled_bundles_file(false) {
        Ok(file) => file,
        Err(error) => {
            log_scope_read_failure(LOG_READ_DATA, &error);
            DisabledBundlesFile::default()
        }
    }
}

/// Per-mode once-only logging for scope-read failures. The engine-side hot
/// readers hit these paths on every turn, so a persistently unavailable lock
/// or an unreadable data file must not print a line per read — the first
/// occurrence per failure mode is enough to make the degradation diagnosable.
fn log_scope_read_failure(mode: u8, detail: &str) {
    use std::sync::atomic::{AtomicU8, Ordering};
    static LOGGED: AtomicU8 = AtomicU8::new(0);
    if LOGGED.fetch_or(mode, Ordering::Relaxed) & mode == 0 {
        eprintln!("[scope] {detail}");
    }
}

const LOG_LOCK_OPEN: u8 = 1 << 0;
const LOG_LOCK_PROBE: u8 = 1 << 1;
const LOG_READ_DATA: u8 = 1 << 2;

/// Read implementation shared by the locked and degraded paths:
/// - Missing file → first-version migration of the two legacy files
///   (idempotent, read-only inputs).
/// - Unreadable file (anything but a missing file) → `Err`. Fabricating a
///   state here would silently drop the user's recorded denies — fail-open
///   on the DenyAll consent gate — and a locked caller persisting that
///   fabrication would destroy the on-disk state. The write path refuses;
///   policy reads degrade loudly to the default.
/// - Corrupt JSON → the corrupt bytes are moved aside
///   (`disabled_bundles.json.corrupt.<unix-seconds>`, locked path only — the
///   degraded path never writes, mirroring the installed.json backup
///   convention), then `Err`. The next locked load starts from the migration
///   default, so DenyAll scopes re-derive fail-closed, and the evidence
///   survives.
/// - Valid file → parsed with a defensive `skill:` prefix strip (new write
///   paths no longer produce it). Read-time repairs persist only when
///   `persist_repairs` is set — i.e. only under the full lock, so the
///   on-disk file converges to the new format without any unsynchronized
///   write.
fn read_disabled_bundles_file(persist_repairs: bool) -> Result<DisabledBundlesFile, String> {
    let path = disabled_bundles_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let file = migrate_from_legacy_files();
            if persist_repairs
                && (!file.scopes.is_empty() || file.initialized.iter().any(|k| !k.is_empty()))
            {
                if let Err(error) = save_disabled_bundles_file(&file) {
                    eprintln!("[scope] read-repair persist failed: {error}");
                }
            }
            return Ok(file);
        }
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    match serde_json::from_str::<DisabledBundlesFile>(&content) {
        Ok(mut file) => {
            if strip_skill_prefixes(&mut file) && persist_repairs {
                if let Err(error) = save_disabled_bundles_file(&file) {
                    eprintln!("[scope] read-repair persist failed: {error}");
                }
            }
            Ok(file)
        }
        Err(error) => {
            if persist_repairs {
                backup_corrupt_file(&path);
            }
            Err(format!("parse {}: {error}", path.display()))
        }
    }
}

/// Moves a corrupt data file aside (`<name>.corrupt.<unix-seconds>`) so the
/// evidence survives AND the next locked load starts from the migration
/// default — leaving the corrupt bytes in place would trip (and refuse) every
/// later write forever. Best effort: if the rename fails, the parse error
/// still refuses the caller and the file stays as found. Locked path only.
fn backup_corrupt_file(path: &std::path::Path) {
    let file_name = match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => return,
    };
    let Some(parent) = path.parent() else {
        return;
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = parent.join(format!("{file_name}.corrupt.{ts}"));
    if let Err(error) = std::fs::rename(path, &backup) {
        eprintln!(
            "[scope] failed to move corrupt {} aside to {}: {error}",
            path.display(),
            backup.display()
        );
    }
}

/// Read under the full write lock: repairs persist (serialized with every
/// other lock holder) and a corrupt file is quarantined. Write critical
/// sections load through this and refuse on `Err` — an unreadable or corrupt
/// file must stop the read-modify-write, never feed it a fabricated state.
fn load_disabled_bundles_file_locked() -> Result<DisabledBundlesFile, String> {
    read_disabled_bundles_file(true)
}

/// 防御：剥除所有 scope 禁用集与不可见集里的 `skill:` 前缀（旧前端 bug 窗口期
/// 误写入的带前缀 id；本文件按裸包 id 匹配，读者在此统一归一）。返回是否剥出过前缀。
fn strip_skill_prefixes(file: &mut DisabledBundlesFile) -> bool {
    let mut stripped = false;
    for ids in file
        .scopes
        .values_mut()
        .chain(file.hidden_scopes.values_mut())
    {
        for id in ids.iter_mut() {
            if let Some(s) = id.strip_prefix("skill:") {
                *id = s.to_string();
                stripped = true;
            }
        }
    }
    stripped
}

/// 原始条目 → 包 id。连接器/CLI id 原样保留（`skill_owner_package` 对它们恒等）；
/// `skill:` 前缀剥除后按技能名映射到所属包（companion → MCP/CLI 包，独立技能 → 自身）。
fn to_package_id(raw: &str) -> String {
    let stripped = raw.strip_prefix("skill:").unwrap_or(raw);
    skill_owner_package(stripped)
}

/// 读时归一：存储条目按**当前**认领状态重映射为包 id 并去重（保序）。
/// 认领（`skill_owner_package`）随安装态时变：条目可能在 companion MCP 未装时
/// 按独立技能 id 落库，MCP 后装则认领翻转到包 id——只在写时归一会让用户的
/// 「关/隐藏」在认领翻转后静默失效（F4）；读时归一让门控跟随技能本体。
fn normalize_stored_pkg_ids(ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(ids.len());
    for id in ids {
        let pkg = to_package_id(id);
        if !out.iter().any(|x| x == &pkg) {
            out.push(pkg);
        }
    }
    out
}

/// First-boot migration: reads the legacy `disabled_connectors.json` and
/// `disabled_skills.json` (each tolerant of three legacy shapes), maps the
/// entries to bundle ids, and merges them into the new file — connector
/// scopes overwrite while skill scopes union, with `project_skills_enabled`
/// taken from the skill file. The legacy files are not deleted (kept as
/// read-only history for this release cycle; retired alongside the legacy
/// layout in a later cycle).
fn migrate_from_legacy_files() -> DisabledBundlesFile {
    let mut file = DisabledBundlesFile::default();
    merge_connector_scopes_into(&mut file);
    merge_skill_scopes_into(&mut file);
    file
}

/// 把旧 `disabled_connectors.json` 的各 scope 条目映射为包 id 并并进 `file`
/// （scope 条目按旧文件**覆盖写**）。
fn merge_connector_scopes_into(file: &mut DisabledBundlesFile) {
    merge_legacy_scope_file_into(
        file,
        &paths::pinvou3_home().join("disabled_connectors.json"),
        |file, key, ids| {
            file.scopes.insert(key.to_string(), ids);
        },
        false,
    );
}

/// 把旧 `disabled_skills.json` 的各 scope 条目映射为包 id 并并进 `file`（取并集），
/// 并继承 `project_skills_enabled`。
fn merge_skill_scopes_into(file: &mut DisabledBundlesFile) {
    merge_legacy_scope_file_into(
        file,
        &paths::pinvou3_home().join("disabled_skills.json"),
        |file, key, ids| merge_ids_into_scope(file, key, ids),
        true,
    );
}

/// 旧 scope 文件（`disabled_connectors.json` / `disabled_skills.json`）的共用解析
/// 骨架：裸数组 → plain scope、新版 `{scopes, initialized}` 对象、旧双 scope 对象
/// `{plain, code, code_initialized}` 三种形态，条目经 `to_package_id` 归一为包 id，
/// 并迁移 `initialized` / `code_initialized`。
///
/// `merge_ids` 决定 scope 条目的落库语义（连接器文件 = 覆盖写，技能文件 = 并集
/// 合并）；`inherit_project_flag` 为真时继承 `project_skills_enabled`（仅技能文件）。
fn merge_legacy_scope_file_into(
    file: &mut DisabledBundlesFile,
    path: &std::path::Path,
    merge_ids: impl Fn(&mut DisabledBundlesFile, &str, Vec<String>),
    inherit_project_flag: bool,
) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    // 裸数组 → plain scope
    if let Ok(list) = serde_json::from_str::<Vec<String>>(&content) {
        let ids: Vec<String> = list.iter().map(|id| to_package_id(id)).collect();
        if !ids.is_empty() {
            merge_ids(file, SessionMode::Plain.as_str(), ids);
        }
        return;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };
    let Some(obj) = value.as_object() else {
        return;
    };
    if let Some(scopes) = obj.get("scopes").and_then(|v| v.as_object()) {
        for (key, arr) in scopes {
            if let Some(arr) = arr.as_array() {
                let ids: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(to_package_id))
                    .collect();
                if !ids.is_empty() {
                    merge_ids(file, key, ids);
                }
            }
        }
        if let Some(initialized) = obj.get("initialized").and_then(|v| v.as_array()) {
            for key in initialized.iter().filter_map(|v| v.as_str()) {
                file.initialized.insert(key.to_string());
            }
        }
    } else {
        // 旧双 scope 对象 {plain, code, code_initialized}
        for key in ["plain", "code"] {
            if let Some(arr) = obj.get(key).and_then(|v| v.as_array()) {
                let ids: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(to_package_id))
                    .collect();
                if !ids.is_empty() {
                    merge_ids(file, key, ids);
                }
            }
        }
        if obj
            .get("code_initialized")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            file.initialized
                .insert(SessionMode::Code.as_str().to_string());
        }
    }
    if inherit_project_flag {
        if let Some(enabled) = obj.get("project_skills_enabled").and_then(|v| v.as_bool()) {
            file.project_skills_enabled = enabled;
        }
    }
}

/// 并集合并到某 scope（去重、保序）。
fn merge_ids_into_scope(file: &mut DisabledBundlesFile, key: &str, ids: Vec<String>) {
    if ids.is_empty() {
        return;
    }
    let entry = file.scopes.entry(key.to_string()).or_default();
    for id in ids {
        if !entry.iter().any(|e| e == &id) {
            entry.push(id);
        }
    }
}

/// 写完整文件（原子替换，与旧文件同范式）。The write is part of the locked
/// critical section, so a failure is returned and the caller's critical
/// section reports `Err` — an `Ok` must mean the caller's change landed,
/// otherwise the DenyAll consent gate could still fail open (silently
/// dropping the write) on a full disk or an unwritable home.
fn save_disabled_bundles_file(file: &DisabledBundlesFile) -> Result<(), String> {
    let json = serde_json::to_string(file)
        .map_err(|error| format!("serialize disabled bundles: {error}"))?;
    deepseek_tui::utils::write_atomic(&disabled_bundles_path(), json.as_bytes())
        .map_err(|error| format!("write {}: {error}", disabled_bundles_path().display()))
}

/// 读某 scope 被禁用的**包 id** 列表（读不到/空 → 空）。
///
/// 已初始化的 scope 以落盘列表为准；未初始化的 scope 按其模式的包默认策略兜底：
/// DenyAll（如 code）返回全部已安装包 id ∪ 全部内置 CLI 包 id ——「默认全关，外部
/// 能力显式开启」；AllowAll（如 plain）返回落盘列表（缺省空 = 全开）。CLI 包未连接时
/// 纳入无害（配套技能不在盘上，排除为空操作），且「后才连接」也自动默认关。
///
/// When skill enumeration fails (permissions / transient IO, #531) the freshly
/// computed default degrades toward over-denial: owner packages of all preset
/// skills and of all store-known upload records are blanket-unioned into the
/// deny set (a superset of anything the failed enumeration could have named).
/// Only the computed default of an uninitialized scope is affected; initialized
/// scopes keep their persisted list.
pub fn load_disabled_bundles_for(scope: ConnectorScope) -> Vec<String> {
    let file = load_disabled_bundles_file();
    resolve_scope_disabled_ids(&file, scope)
}

/// 已加载文件 → 某 scope 的有效禁用包 id 列表（含 DenyAll 默认兜底）。供
/// `load_disabled_bundles_for` 与持锁写方（单临界区 RMW）共用，口径一致。
fn resolve_scope_disabled_ids(file: &DisabledBundlesFile, scope: ConnectorScope) -> Vec<String> {
    let key = scope.as_str();
    if file.initialized.contains(key) {
        return normalize_stored_pkg_ids(&file.scopes.get(key).cloned().unwrap_or_default());
    }
    match scope.pack_default_policy() {
        PackDefaultPolicy::AllowAll => {
            normalize_stored_pkg_ids(&file.scopes.get(key).cloned().unwrap_or_default())
        }
        PackDefaultPolicy::DenyAll => {
            // 现算分支：已按当前认领推导包 id，无需再归一。
            let mut ids: Vec<String> = MarketplaceManager::new().installed_ids();
            ids.extend(builtin_cli_bundle_ids().map(str::to_string));
            let skill_market = SkillMarketplaceManager::new();
            let (skill_ids, skill_scan_degraded) = skill_market.installed_skill_ids_strict();
            if skill_scan_degraded {
                // Enumeration degraded (#531): a failed probe can masquerade an
                // installed skill as absent, so the default deny set must not
                // shrink because of it. Blanket-union the owner packages of
                // every preset skill (compile-time manifests) and of every
                // known upload record — an uninitialized DenyAll scope would
                // rather have the user enable a package explicitly than hand
                // the consent gate a default silently narrowed by an
                // enumeration failure. Owner mapping is the same
                // `skill_owner_package` path as the normal loop below.
                // Accepted residual: with an unreadable bundle store the upload
                // population itself is unknowable (the lenient upload read
                // yields nothing to union); with an unreadable packages root,
                // straggler-copy-only skills of neither kind can be seen.
                eprintln!(
                    "[scope] DenyAll default deny list degraded (skill enumeration failed); biasing to over-deny"
                );
                let mut blanket: Vec<String> =
                    SkillMarketplaceManager::preset_skill_ids().collect();
                blanket.extend(skill_market.uploaded_skill_ids());
                for skill_id in blanket {
                    let pkg = skill_owner_package(&skill_id);
                    if !ids.iter().any(|id| id == &pkg) {
                        ids.push(pkg);
                    }
                }
            }
            for skill_id in skill_ids {
                let pkg = skill_owner_package(&skill_id);
                if !ids.iter().any(|id| id == &pkg) {
                    ids.push(pkg);
                }
            }
            ids
        }
    }
}

/// 写某 scope 被禁用的包 id 列表（写入即标记该 scope 已初始化）。入参统一归一为包
/// id（剥 `skill:` 前缀 + companion 映射），防御历史版本误写入的带前缀条目。
/// Cross-process lock unavailable → `Err`: the write is refused, never
/// performed unsynchronized (#515).
pub fn save_disabled_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    with_scope_file_lock(|| {
        let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
        let mut file = load_disabled_bundles_file_locked()?;
        let key = scope.as_str().to_string();
        file.scopes.insert(key.clone(), normalized);
        file.initialized.insert(key);
        save_disabled_bundles_file(&file)
            .map_err(|error| format!("save disabled bundles ({}): {error}", scope.as_str()))
    })?
}

/// 读某 scope 被「不可见」（可见性过滤）的包 id 列表。缺省空 = 全可见。
/// 与 `load_disabled_bundles_for`（开关）正交：可见性只决定是否出现在 composer 列表，
/// 不决定 on/off。
pub fn load_hidden_bundles_for(scope: ConnectorScope) -> Vec<String> {
    let file = load_disabled_bundles_file();
    resolve_scope_hidden_ids(&file, scope)
}

/// 已加载文件 → 某 scope 的有效不可见包 id 列表（读时归一，与开关集同口径；
/// 无默认兜底，显式写入才隐藏）。
/// 供 `load_hidden_bundles_for` 与 `unavailable_bundles_for` 的单快照合并读共用。
fn resolve_scope_hidden_ids(file: &DisabledBundlesFile, scope: ConnectorScope) -> Vec<String> {
    normalize_stored_pkg_ids(
        &file
            .hidden_scopes
            .get(scope.as_str())
            .cloned()
            .unwrap_or_default(),
    )
}

/// 写某 scope 被「不可见」的包 id 列表（不参与 DenyAll 默认，显式写入才隐藏）。
/// Cross-process lock unavailable → `Err`: the write is refused, never
/// performed unsynchronized (#515).
pub fn save_hidden_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    with_scope_file_lock(|| {
        let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
        let mut file = load_disabled_bundles_file_locked()?;
        file.hidden_scopes
            .insert(scope.as_str().to_string(), normalized);
        save_disabled_bundles_file(&file)
            .map_err(|error| format!("save hidden bundles ({}): {error}", scope.as_str()))
    })?
}

/// 该 scope 对底座「不可用」的包 id 并集 = 开关关（disabled）+ 不可见（hidden）。
/// 物化/工具白名单按此并集排除，两套门控对模型都是「调不到」。单次持锁读出
/// 两套集合（同一文件快照）：每次并集解析只取一次锁、只解析一次文件，也不会
/// 混读两个时刻的 disabled/hidden（同一次刷新内多次调用之间的跨调用快照窗口
/// 仍在，由各调用方自行取舍）。
pub fn unavailable_bundles_for(scope: ConnectorScope) -> Vec<String> {
    let file = load_disabled_bundles_file();
    let mut ids = resolve_scope_disabled_ids(&file, scope);
    for id in resolve_scope_hidden_ids(&file, scope) {
        if !ids.iter().any(|x| x == &id) {
            ids.push(id);
        }
    }
    ids
}

/// 读全局（plain）被禁用的包 id 列表。兼容既有调用方。
pub fn load_disabled_bundles() -> Vec<String> {
    load_disabled_bundles_for(ConnectorScope::Plain)
}

/// 写全局（plain）被禁用的包 id 列表。测试专用（生产写一律走
/// [`save_disabled_bundles_for`] 显式给 scope）。启动期 best-effort，
/// 写失败降级为日志（调用方无法处理治理写失败）。
#[cfg(test)]
pub fn save_disabled_bundles(ids: &[String]) {
    if let Err(error) = save_disabled_bundles_for(ConnectorScope::Plain, ids) {
        eprintln!("[scope] write disabled_bundles.json failed: {error}");
    }
}

/// Whether the normalized consent-gate id is already installed in this home:
/// a tool/CLI bundle with an install record (or an `installed.json` entry),
/// or a skill on disk (standalone, or claimed by its installed owner).
/// Built-in connectors deliberately stay NOT-known (they have no install
/// record), so the connector channels keep their deny-first registration and
/// refusal boundary.
fn consent_gate_bundle_already_known(package_id: &str) -> bool {
    if bundle_installed(package_id) {
        return true;
    }
    SkillMarketplaceManager::new()
        .installed_skill_ids()
        .iter()
        .any(|skill| skill_owner_package(skill) == package_id)
}

/// 包安装/连接后同步所有 DenyAll 且已初始化的 scope：**新装**的包默认保持关闭
/// （加入该 scope 禁用集）；未初始化时无需处理（load 会按「默认全禁已装包」兜底）。
/// AllowAll 模式无需同步（默认全开）。连接器与技能安装共用本入口：入参可为连接器
/// id / 技能 id / 包 id，统一归一为包 id。
/// This is the safety-default write for the DenyAll consent gate: refused with
/// `Err` when the cross-process lock is unavailable, never unsynchronized.
///
/// Reinstall/update of an ALREADY-installed bundle skips the write entirely
/// and returns `Ok`: every initialized scope's current entry is the user's
/// recorded consent, so re-registering would both reset an explicit enable on
/// the success path and — worse — silently disable the previously working
/// installation whenever any post-gate step fails (pip deps, disk full,
/// content conflict, remote validation) with no recovery path. This is the
/// same preserve-state contract as `update_marketplace_skill`. Only unknown
/// (fresh) ids register deny-first, so the boundary this gate exists to close
/// — a new package is never exposed outside the deny lists of the initialized
/// DenyAll scopes — is unchanged, and a leftover entry from a failed fresh
/// install stays fail-closed and converges on the next successful install.
pub fn sync_deny_all_scopes_after_install(raw_id: &str) -> Result<(), String> {
    let package_id = to_package_id(raw_id);
    if consent_gate_bundle_already_known(&package_id) {
        return Ok(());
    }
    with_scope_file_lock(|| {
        let mut file = load_disabled_bundles_file_locked()?;
        let mut changed = false;
        for mode in SessionMode::ALL {
            if mode.pack_default_policy() != PackDefaultPolicy::DenyAll {
                continue;
            }
            let key = mode.as_str();
            if !file.initialized.contains(key) {
                continue;
            }
            let ids = file.scopes.entry(key.to_string()).or_default();
            if !ids.iter().any(|id| id == &package_id) {
                ids.push(package_id.clone());
                changed = true;
            }
        }
        if changed {
            save_disabled_bundles_file(&file).map_err(|error| {
                format!("sync DenyAll scopes after install ({package_id}): {error}")
            })?;
        }
        Ok(())
    })?
}

/// Sync every scope after a bundle uninstall/disconnect: drop the id from each
/// scope's disabled and visibility sets so no stale entry keeps pointing at a
/// missing package. Shared entry point for connector, skill, and package
/// teardown: the argument may be a connector id / skill id / package id and is
/// normalized to the package id.
pub fn remove_bundle_from_disabled_scopes(raw_id: &str) -> Result<(), String> {
    let package_id = to_package_id(raw_id);
    with_scope_file_lock(|| {
        let mut file = load_disabled_bundles_file_locked()?;
        let mut changed = false;
        for ids in file.scopes.values_mut() {
            let before = ids.len();
            ids.retain(|id| id != &package_id);
            changed |= ids.len() != before;
        }
        // 可见性集同样清理：卸载后残留 hidden 会误隐藏未来同名重装。
        for ids in file.hidden_scopes.values_mut() {
            let before = ids.len();
            ids.retain(|id| id != &package_id);
            changed |= ids.len() != before;
        }
        if changed {
            save_disabled_bundles_file(&file)
                .map_err(|error| format!("remove {package_id} from disabled scopes: {error}"))?;
        }
        Ok(())
    })?
}

/// 项目级 skills 开关（默认关）。
pub fn project_skills_enabled() -> bool {
    load_disabled_bundles_file().project_skills_enabled
}

/// 写项目级 skills 开关。落盘后由调用方重写在线会话组合目录。
/// Cross-process lock unavailable → `Err`: the write is refused, never
/// performed unsynchronized (#515).
pub fn set_project_skills_enabled(enabled: bool) -> Result<(), String> {
    with_scope_file_lock(|| {
        let mut file = load_disabled_bundles_file_locked()?;
        if file.project_skills_enabled == enabled {
            return Ok(());
        }
        file.project_skills_enabled = enabled;
        save_disabled_bundles_file(&file)
            .map_err(|error| format!("set project skills enabled: {error}"))
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::test_support::{make_dir_unreadable_for_test, with_temp_home};

    #[test]
    fn bundles_roundtrip_per_scope() {
        with_temp_home("pinvou3-scope", || {
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            save_disabled_bundles_for(ConnectorScope::Code, &["feishu".to_string()]).unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["feishu".to_string()]
            );
        });
    }

    /// Unavailable = disabled + hidden, deduped; visibility writes must not
    /// pollute the disabled set (the two sets stay orthogonal).
    #[test]
    fn unavailable_is_union_deduped() {
        with_temp_home("pinvou3-scope", || {
            // Initially both sets are empty.
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            assert!(load_hidden_bundles_for(ConnectorScope::Plain).is_empty());

            // Disable weather, hide weather + pptx (weather appears in both sets).
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            save_hidden_bundles_for(
                ConnectorScope::Plain,
                &["weather".to_string(), "pptx".to_string()],
            )
            .unwrap();

            // Visibility writes do not pollute the disabled set.
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            assert_eq!(
                load_hidden_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string(), "pptx".to_string()]
            );

            // Union dedup: weather appears exactly once.
            let mut u = unavailable_bundles_for(ConnectorScope::Plain);
            u.sort();
            assert_eq!(u, vec!["pptx".to_string(), "weather".to_string()]);
        });
    }

    /// DenyAll（未初始化）scope 的不可用并集 = 开关集默认兜底（已装包 ∪ 内置 CLI
    /// 包）∪ 显式 hidden：hidden 不参与默认策略，但并集仍须包含它，且与默认集
    /// 相交时按并集去重（feishu 既是内置 CLI 包又是显式 hidden，只出现一次）。
    /// 合并读与两套集合各自的读入口同口径。
    #[test]
    fn unavailable_includes_hidden_in_uninitialized_deny_all_scope() {
        with_temp_home("pinvou3-scope", || {
            save_hidden_bundles_for(
                ConnectorScope::Code,
                &["weather".to_string(), "feishu".to_string()],
            )
            .unwrap();

            // Code 未初始化：开关集走 DenyAll 兜底；hidden 只追加上显式条目。
            let mut expected = load_disabled_bundles_for(ConnectorScope::Code);
            if !expected.iter().any(|id| id == "weather") {
                expected.push("weather".to_string());
            }
            assert_eq!(unavailable_bundles_for(ConnectorScope::Code), expected);
        });
    }

    /// 卸载/断开后清理残留：同时清 disabled 与 hidden 两套集合。
    #[test]
    fn remove_bundle_clears_both_sets() {
        with_temp_home("pinvou3-scope", || {
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            save_hidden_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            remove_bundle_from_disabled_scopes("weather").unwrap();
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            assert!(load_hidden_bundles_for(ConnectorScope::Plain).is_empty());
        });
    }

    /// 一次助手调用覆盖**所有** scope 的 disabled + hidden 两套集合:退役工具清理等
    /// 调用方依赖「单次调用 = 全清理面」,无需逐 scope 手工 load/retain/save
    /// (#522:逐 scope 两段式各自取锁,会在 load 与 save 之间丢并发更新)。
    #[test]
    fn remove_bundle_clears_every_scope_and_hidden_set() {
        with_temp_home("pinvou3-scope", || {
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            save_disabled_bundles_for(
                ConnectorScope::Code,
                &["weather".to_string(), "pptx".to_string()],
            )
            .unwrap();
            save_hidden_bundles_for(ConnectorScope::Code, &["weather".to_string()]).unwrap();

            remove_bundle_from_disabled_scopes("weather");

            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["pptx".to_string()],
                "同 scope 其它包的条目必须原样保留"
            );
            assert!(load_hidden_bundles_for(ConnectorScope::Code).is_empty());
            // helper 不得动 scope 初始化登记:退役清理依赖该契约保留用户的
            // 初始化状态(上面三次 save 已把 plain/code 标记为 initialized)。
            let file = load_disabled_bundles_file();
            assert!(
                file.initialized.contains("plain") && file.initialized.contains("code"),
                "helper 必须保留 scope 初始化登记: {:?}",
                file.initialized
            );
        });
    }

    /// 保存路径统一归一为包 id：剥 `skill:` 前缀 + companion 映射到所属包。
    #[test]
    fn save_normalizes_to_package_id() {
        with_temp_home("pinvou3-scope", || {
            // gongwen 未装：companion 技能保留独立纯技能包形态 → 归一为自身 id
            // （与 list_bundles 的 V5 认领展示一致，开关不回弹）。
            save_disabled_bundles_for(
                ConnectorScope::Plain,
                &["skill:government-writing".to_string()],
            )
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["government-writing".to_string()]
            );

            // 登记 gongwen 安装态后：companion 技能归属到包 → 归一为 gongwen。
            crate::features::marketplace::store::BundleStore::new()
                .upsert(
                    crate::features::marketplace::store::BundleRecord::installed_now(
                        "gongwen".to_string(),
                        crate::features::marketplace::store::BundleSource::Preset,
                    ),
                )
                .unwrap();
            save_disabled_bundles_for(
                ConnectorScope::Plain,
                &[
                    "skill:visualizer".to_string(),
                    "government-writing".to_string(),
                ],
            )
            .unwrap();
            // government-writing 是内嵌 gongwen manifest 的 companion 技能 → gongwen 包。
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["visualizer".to_string(), "gongwen".to_string()]
            );
        });
    }

    /// F4 回归：条目在 companion MCP 未装时按独立技能 id 落库；MCP 后装 →
    /// 认领翻转到包 id。读时归一（`normalize_stored_pkg_ids`）须让「关/隐藏」
    /// 跟随技能本体，否则用户的禁用/隐藏态在认领翻转后静默失效。
    #[test]
    fn load_normalizes_stale_skill_id_after_claim_flip() {
        with_temp_home("pinvou3-scope", || {
            save_disabled_bundles_for(ConnectorScope::Plain, &["government-writing".to_string()])
                .unwrap();
            save_hidden_bundles_for(ConnectorScope::Plain, &["government-writing".to_string()])
                .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["government-writing".to_string()]
            );
            assert_eq!(
                load_hidden_bundles_for(ConnectorScope::Plain),
                vec!["government-writing".to_string()]
            );

            // 后装 gongwen → 认领翻转 government-writing → gongwen
            crate::features::marketplace::store::BundleStore::new()
                .upsert(
                    crate::features::marketplace::store::BundleRecord::installed_now(
                        "gongwen".to_string(),
                        crate::features::marketplace::store::BundleSource::Preset,
                    ),
                )
                .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["gongwen".to_string()],
                "认领翻转后读时归一应把禁用条目重映射到包 id"
            );
            assert_eq!(
                load_hidden_bundles_for(ConnectorScope::Plain),
                vec!["gongwen".to_string()],
                "认领翻转后读时归一应把隐藏条目重映射到包 id"
            );
        });
    }

    /// 项目级 skills 开关往返。
    #[test]
    fn project_skills_roundtrip() {
        with_temp_home("pinvou3-scope", || {
            assert!(!project_skills_enabled(), "项目技能默认关");
            set_project_skills_enabled(true).unwrap();
            assert!(project_skills_enabled());
            set_project_skills_enabled(false).unwrap();
            assert!(!project_skills_enabled());
        });
    }

    /// The read path's read-then-migrate-persist must serialize with lock
    /// holders: while the test thread holds `DISABLED_BUNDLES_FILE_LOCK`, a
    /// concurrent load must neither block unbounded nor land its migration
    /// write on disk (it degrades to the unlocked, never-persisting view).
    /// The worker signals readiness before calling load, and the assertion is
    /// a bounded poll window: if serialization were broken, the migration
    /// write would land inside the window and be caught. Convergence of the
    /// on-disk format happens on the next *uncontended* read, which the test
    /// performs after releasing the guard.
    #[test]
    fn read_path_migration_serializes_with_file_lock() {
        with_temp_home("pinvou3-scope", || {
            let legacy = r#"["weather"]"#;
            let conn = paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(&conn, legacy).unwrap();
            let guard = DISABLED_BUNDLES_FILE_LOCK
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                ready_tx
                    .send(())
                    .expect("reader start signal before taking the lock");
                load_disabled_bundles_for_plain_for_lock_test()
            });
            ready_rx
                .recv()
                .expect("reader thread should signal it started");
            // The lock is held here, so a working lock keeps the migration
            // write pending; broken serialization makes it land immediately.
            assert_data_file_absent_within();
            drop(guard);
            // The contended read degrades (in-process mutex held by this
            // thread) and returns the migrated view without persisting.
            assert_eq!(reader.join().unwrap(), vec!["weather".to_string()]);
            assert!(!disabled_bundles_path().exists());
            // The next uncontended read runs fully locked and converges the
            // file to the new format.
            let got = load_disabled_bundles_for_plain_for_lock_test();
            assert_eq!(got, vec!["weather".to_string()]);
            let content = std::fs::read_to_string(disabled_bundles_path()).unwrap();
            assert!(
                content.contains("\"scopes\""),
                "migration should land on the next uncontended read: {content}"
            );
        });
    }

    fn load_disabled_bundles_for_plain_for_lock_test() -> Vec<String> {
        load_disabled_bundles_for(ConnectorScope::Plain)
    }

    /// Physically install the preset skill government-writing (its owner is
    /// claimed as gongwen: once the gongwen record is registered as installed,
    /// `skill_owner_package` follows the package claim).
    fn install_preset_skill_under_claimed_owner() -> PathBuf {
        let skill_dir = paths::bundles_root()
            .join("gongwen")
            .join("skills")
            .join("government-writing");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# government-writing").unwrap();
        crate::features::marketplace::store::BundleStore::new()
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "gongwen".to_string(),
                    crate::features::marketplace::store::BundleSource::Preset,
                ),
            )
            .unwrap();
        skill_dir
    }

    /// Physically install an upload skill `<name>` (bundle store record with an
    /// Upload source + `bundles/<name>/skills/<name>/SKILL.md`). Upload owner
    /// packages self-map, so `<name>` itself is the package id in the deny set.
    fn install_upload_skill(name: &str) -> PathBuf {
        let skill_dir = paths::bundles_root().join(name).join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), format!("# {name}")).unwrap();
        crate::features::marketplace::store::BundleStore::new()
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    name.to_string(),
                    crate::features::marketplace::store::BundleSource::Upload(name.to_string()),
                ),
            )
            .unwrap();
        skill_dir
    }

    /// Uninitialized DenyAll (code) default deny set = installed connector
    /// packages ∪ builtin CLI packages ∪ installed skill owner packages; under
    /// a clean scan it must equal the expected list exactly (no degraded
    /// blanket may leak in — equivalently asserts "the normal path never
    /// misreports degradation").
    #[test]
    fn denyall_default_clean_scan_exact_list() {
        with_temp_home("pinvou3-scope-denyall", || {
            // Empty install state: only builtin CLI package ids (manifest order).
            let builtin: Vec<String> = builtin_cli_bundle_ids().map(str::to_string).collect();
            assert_eq!(load_disabled_bundles_for(ConnectorScope::Code), builtin);

            // Physically install one preset skill → its owner package joins the
            // default deny set.
            install_preset_skill_under_claimed_owner();
            let mut expected = builtin;
            expected.push("gongwen".to_string());
            assert_eq!(load_disabled_bundles_for(ConnectorScope::Code), expected);
        });
    }

    /// #531: with an unscannable skill directory (permissions) the enumeration
    /// degrades and the DenyAll default biases toward over-denial — an
    /// installed skill's owner package must not drop out of the default deny
    /// set, and not-installed preset owners join as conservative fallback (the
    /// user can enable them explicitly). Lenient display paths are unaffected.
    #[test]
    fn denyall_default_degraded_scan_biases_to_overdeny() {
        with_temp_home("pinvou3-scope-denyall-degraded", || {
            let skill_dir = install_preset_skill_under_claimed_owner();

            // Clean baseline: the claimed owner package is denied by default,
            // the not-installed preset owner (pptx) is not.
            let clean = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(clean.contains(&"gongwen".to_string()));
            assert!(!clean.contains(&"pptx".to_string()));

            // Make the skill dir unreadable → the SKILL.md probe hits EACCES:
            // the lenient path would read "not installed" (fail-open), the
            // strict enumeration reports degradation and biases to over-deny.
            // Skip the assertions when the platform/environment cannot simulate
            // unreadable dirs (Windows, root).
            let Some(_unreadable) = make_dir_unreadable_for_test(&skill_dir) else {
                return;
            };
            let degraded = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(
                degraded.contains(&"gongwen".to_string()),
                "degraded enumeration must keep the installed skill's owner package denied: {degraded:?}"
            );
            assert!(
                degraded.contains(&"pptx".to_string()),
                "degradation must bias to over-deny (not-installed preset owner joins): {degraded:?}"
            );
        });
    }

    /// #531 for the upload half: an installed upload skill with an unscannable
    /// directory must not drop out of the default deny set either. The probe
    /// failure degrades the enumeration, the degraded blanket union carries the
    /// upload's own owner package (store readable), and the preset owners (pptx)
    /// join as conservative fallback.
    #[test]
    fn denyall_default_degraded_upload_scan_biases_to_overdeny() {
        with_temp_home("pinvou3-scope-upload-degraded", || {
            let skill_dir = install_upload_skill("my-weather");

            // Clean baseline: the upload's self-mapped owner package is denied.
            let clean = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(
                clean.contains(&"my-weather".to_string()),
                "installed upload owner package must be denied by default: {clean:?}"
            );

            let Some(_unreadable) = make_dir_unreadable_for_test(&skill_dir) else {
                return;
            };
            let degraded = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(
                degraded.contains(&"my-weather".to_string()),
                "degraded enumeration must keep the installed upload owner package denied: {degraded:?}"
            );
            assert!(
                degraded.contains(&"pptx".to_string()),
                "degradation must bias to over-deny (not-installed preset owner joins): {degraded:?}"
            );
        });
    }

    /// A corrupt bundle store (fail-loud read) makes the upload population
    /// unknowable: the enumeration must degrade into the preset-owner blanket
    /// union (pptx joins) instead of silently passing as an empty install set.
    #[test]
    fn denyall_default_corrupt_store_biases_to_overdeny() {
        with_temp_home("pinvou3-scope-store-corrupt", || {
            install_upload_skill("my-weather");
            let store_file = paths::pinvou3_home()
                .join("marketplace")
                .join("bundles.json");
            std::fs::write(&store_file, "{not json").unwrap();

            let degraded = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(
                degraded.contains(&"pptx".to_string()),
                "corrupt store must degrade into the preset-owner blanket union: {degraded:?}"
            );
        });
    }

    /// An unscannable packages root blinds both the claimed-dir context and the
    /// straggler scan: the enumeration must degrade, and the preset-owner
    /// blanket union must keep the installed preset's owner (gongwen) denied
    /// alongside the not-installed preset owners (pptx).
    #[test]
    fn denyall_default_packages_root_failure_biases_to_overdeny() {
        with_temp_home("pinvou3-scope-root-degraded", || {
            install_preset_skill_under_claimed_owner();
            let bundles_root = paths::bundles_root();

            let Some(_unreadable) = make_dir_unreadable_for_test(&bundles_root) else {
                return;
            };
            let degraded = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(
                degraded.contains(&"gongwen".to_string()),
                "root-scan failure must keep the installed preset owner denied: {degraded:?}"
            );
            assert!(
                degraded.contains(&"pptx".to_string()),
                "root-scan failure must degrade into the blanket union: {degraded:?}"
            );
        });
    }

    /// A stray regular file in the packages root must not read as degradation:
    /// ENOTDIR on a joined candidate structurally answers "not there" (same
    /// family as NotFound), so the clean exact list holds without the degraded
    /// blanket.
    #[test]
    fn denyall_default_tolerates_stray_file_without_degradation() {
        with_temp_home("pinvou3-scope-stray-file", || {
            install_preset_skill_under_claimed_owner();
            let stray = paths::bundles_root().join("stray-file.txt");
            std::fs::write(&stray, "not a package").unwrap();

            let mut expected: Vec<String> = builtin_cli_bundle_ids().map(str::to_string).collect();
            expected.push("gongwen".to_string());
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                expected,
                "stray file must not flip the default into degraded over-deny"
            );
        });
    }

    /// #531 boundary: an initialized scope sticks to its persisted list; skill
    /// enumeration degradation must not affect it (the over-denial fallback
    /// only applies to the freshly computed default of an uninitialized scope).
    #[test]
    fn initialized_scope_ignores_degraded_skill_scan() {
        with_temp_home("pinvou3-scope-initialized", || {
            save_disabled_bundles_for(ConnectorScope::Code, &["weather".to_string()]).unwrap();
            let skill_dir = install_preset_skill_under_claimed_owner();
            let Some(_unreadable) = make_dir_unreadable_for_test(&skill_dir) else {
                return;
            };
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["weather".to_string()],
                "an initialized scope sticks to its persisted list, unaffected by degradation"
            );
        });
    }

    /// Blocks until some thread holds the in-process scope mutex, i.e. the
    /// worker has provably reached the critical section and its OS-lock
    /// attempt has started. Deterministic handshake for the cross-process
    /// lock regressions (no sleep-before-assert races).
    fn wait_until_scope_mutex_held() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match DISABLED_BUNDLES_FILE_LOCK.try_lock() {
                // Not held by the worker yet — retry shortly.
                Ok(guard) => drop(guard),
                // Held by the worker: with the foreign lock held, it is now
                // either blocked on the OS lock or its acquisition failed.
                Err(std::sync::TryLockError::WouldBlock) => return,
                Err(std::sync::TryLockError::Poisoned(p)) => {
                    drop(p.into_inner());
                    return;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker never reached the scope lock acquisition point"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Asserts the data file does not appear within the poll window. Must run
    /// after the worker provably reached the acquisition point (handshake
    /// above or a start signal): a broken lock then makes the write land
    /// within milliseconds and fails the assertion deterministically, while a
    /// working lock simply runs out the window.
    fn assert_data_file_absent_within() {
        let window = std::time::Duration::from_secs(2);
        let deadline = std::time::Instant::now() + window;
        while std::time::Instant::now() < deadline {
            assert!(
                !disabled_bundles_path().exists(),
                "data file written while the lock was still held — serialization is broken"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Harness for the cross-process WRITE regression: holds the lock with a
    /// foreign fd, runs `worker` on a fresh thread, deterministically proves
    /// the worker cannot write the data file while the foreign lock is held,
    /// releases the lock, and joins. Returns the worker's result.
    ///
    /// The foreign fd stands in for the peer process: flock ownership is per
    /// open file description, so a second fd inside this process contends
    /// exactly like another process. The write guard borrows the local
    /// `RwLock`, so both stay plain locals of this function.
    fn assert_blocked_until_foreign_lock_release<R, F>(worker: F) -> R
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(disabled_bundles_lock_path())
            .expect("test should be able to open the lock file");
        let mut foreign = fd_lock::RwLock::new(file);
        let foreign_guard = foreign
            .write()
            .expect("test should be able to take the foreign cross-process write lock");
        let handle = std::thread::spawn(worker);
        wait_until_scope_mutex_held();
        assert_data_file_absent_within();
        drop(foreign_guard);
        handle
            .join()
            .expect("worker should finish once the foreign lock is released")
    }

    /// #515 cross-process contention on the READ path: while a peer holds the
    /// OS lock, a load must degrade promptly to the unlocked, never-persisting
    /// view (bounded — hot readers never couple to a peer's critical section)
    /// instead of blocking or writing unsynchronized. A later uncontended read
    /// converges the on-disk format.
    #[test]
    fn cross_process_lock_contention_degrades_read_without_persist() {
        with_temp_home("pinvou3-scope-cross-process", || {
            let legacy = r#"["weather"]"#;
            let conn = paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(&conn, legacy).unwrap();

            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(disabled_bundles_lock_path())
                .expect("test should be able to open the lock file");
            let mut foreign = fd_lock::RwLock::new(file);
            let foreign_guard = foreign
                .write()
                .expect("test should be able to take the foreign cross-process write lock");

            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                let got = load_disabled_bundles_for_plain_for_lock_test();
                done_tx.send(got).expect("reader should send its result");
            });
            // A contended read must return promptly. recv_timeout doubles as
            // the regression assertion: a blocking read hangs here and fails
            // the test with a bounded, diagnosable timeout.
            let got = done_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("contended read must degrade instead of blocking on the peer's lock");
            assert_eq!(got, vec!["weather".to_string()]);
            assert!(
                !disabled_bundles_path().exists(),
                "a contended read must not persist the migration"
            );

            drop(foreign_guard);
            reader.join().expect("reader thread should finish");

            // A later uncontended read runs fully locked and converges the
            // file to the new format.
            let got = load_disabled_bundles_for_plain_for_lock_test();
            assert_eq!(got, vec!["weather".to_string()]);
            let content = std::fs::read_to_string(disabled_bundles_path()).unwrap();
            assert!(
                content.contains("\"scopes\""),
                "migration should land on the next uncontended read: {content}"
            );
        });
    }

    /// #515 symmetric case for the WRITE path: while a foreign fd holds the
    /// lock, the save must not land first; after the release the write lands
    /// with its full content.
    #[test]
    fn cross_process_lock_blocks_save_write_until_release() {
        with_temp_home("pinvou3-scope-cross-process-save", || {
            assert_blocked_until_foreign_lock_release(|| {
                save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()])
                    .expect("write should succeed once the foreign lock is released");
            });
            let content = std::fs::read_to_string(disabled_bundles_path()).unwrap();
            assert!(
                content.contains("\"weather\""),
                "write should land after the lock is released: {content}"
            );
        });
    }

    /// The bounded-degrade invariant must hold against the in-process
    /// dimension too: a local write parked inside its critical section holds
    /// `DISABLED_BUNDLES_FILE_LOCK` (worst case: parked on a frozen peer's OS
    /// lock, which blocks without timeout), and a hot reader that waited on
    /// that mutex would hang the engine's per-turn reads. The reader must
    /// degrade promptly to the unlocked snapshot instead — pinned with the
    /// same bounded recv_timeout idiom as the peer-contention regression.
    #[test]
    fn hot_read_degrades_while_local_write_parks_in_critical_section() {
        with_temp_home("pinvou3-scope-hot-read", || {
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()])
                .expect("seeding the stored state should succeed");
            let (park_tx, park_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let writer = std::thread::spawn(move || {
                with_scope_file_lock(|| {
                    park_tx
                        .send(())
                        .expect("writer should signal it reached the critical section");
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(10))
                        .expect("test should release the parked writer");
                })
                .expect("parked writer should complete normally");
            });
            park_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("writer should reach the critical section");
            // The OS lock is free here, so the ONLY thing that could block the
            // read is the in-process mutex the parked writer holds.
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                let got = load_disabled_bundles_for_plain_for_lock_test();
                done_tx.send(got).expect("reader should send its result");
            });
            let got = done_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("hot read must degrade instead of blocking on the parked local writer");
            assert_eq!(got, vec!["weather".to_string()]);
            reader.join().expect("reader thread should finish");

            release_tx.send(()).expect("test should release the writer");
            writer.join().expect("writer thread should finish");
        });
    }

    /// #515 hard-fail: when the lock file cannot be opened (a directory at the
    /// lock path), every write entry point is refused with `Err` — never run
    /// unsynchronized — and the data file stays untouched. The refusal must
    /// name the lock failure so a spurious unrelated error cannot pass the
    /// assertion (the false-pass half of the #528 pattern).
    #[test]
    fn writes_refused_when_lock_file_unavailable() {
        with_temp_home("pinvou3-scope-write-refused", || {
            std::fs::create_dir_all(disabled_bundles_lock_path()).unwrap();
            let error = save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()])
                .unwrap_err();
            assert!(
                error.contains("disabled_bundles.lock"),
                "refusal must name the lock failure: {error}"
            );
            assert!(save_hidden_bundles_for(ConnectorScope::Plain, &["w".to_string()]).is_err());
            assert!(set_project_skills_enabled(true).is_err());
            assert!(
                sync_deny_all_scopes_after_install("weather").is_err(),
                "the DenyAll consent-gate sync must refuse too"
            );
            assert!(
                remove_bundle_from_disabled_scopes("weather").is_err(),
                "the uninstall/restore cleanup must refuse too"
            );
            assert!(!disabled_bundles_path().exists());
        });
    }

    /// The consent gate must distinguish fresh installs from reinstalls: an
    /// unknown id registers deny-first (default-off in every initialized
    /// DenyAll scope), while an already-installed bundle is a no-op — every
    /// scope's current entry is the user's recorded consent, and re-denying
    /// it would let any post-gate install failure silently disable a
    /// previously working installation with no recovery path.
    #[test]
    fn consent_gate_skips_known_bundles_registers_fresh_ones() {
        with_temp_home(|| {
            crate::features::marketplace::store::BundleStore::new()
                .upsert(
                    crate::features::marketplace::store::BundleRecord::installed_now(
                        "weather",
                        crate::features::marketplace::store::BundleSource::Preset,
                    ),
                )
                .unwrap();
            save_disabled_bundles_for(ConnectorScope::Code, &["seed-bundle".to_string()]).unwrap();

            // Known bundle: Ok without adding it to any deny list.
            sync_deny_all_scopes_after_install("weather")
                .expect("a known bundle must skip the consent-gate write");
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["seed-bundle".to_string()],
                "a reinstall must not re-deny the installed bundle"
            );

            // Fresh bundle: registers deny-first.
            sync_deny_all_scopes_after_install("fresh-gate-tool").unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["seed-bundle".to_string(), "fresh-gate-tool".to_string()]
            );
        });
    }

    /// A corrupt data file must stop every locked RMW instead of feeding it a
    /// fabricated empty state: an `Ok` here would persist the fabrication and
    /// silently destroy the user's recorded denies (fail-open on the DenyAll
    /// gate). The locked path quarantines the corrupt bytes, and the next
    /// write starts from the migration default (DenyAll scopes re-derive
    /// fail-closed). During the corrupt window, policy reads of an
    /// uninitialized DenyAll scope still compute the full-deny default.
    #[test]
    fn corrupt_file_refuses_write_then_quarantine_recovers() {
        with_temp_home("pinvou3-scope-corrupt-quarantine", || {
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            let path = disabled_bundles_path();
            let corrupt = "{not json";
            std::fs::write(&path, corrupt).unwrap();

            assert!(
                save_disabled_bundles_for(ConnectorScope::Plain, &["pptx".to_string()]).is_err(),
                "a corrupt file must refuse the read-modify-write"
            );
            let home = paths::pinvou3_home();
            let quarantined: Vec<std::path::PathBuf> = std::fs::read_dir(&home)
                .unwrap()
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .map(|name| {
                            name.to_string_lossy()
                                .starts_with("disabled_bundles.json.corrupt.")
                        })
                        .unwrap_or(false)
                })
                .collect();
            assert_eq!(
                quarantined.len(),
                1,
                "the corrupt file must be quarantined exactly once: {quarantined:?}"
            );
            assert_eq!(
                std::fs::read_to_string(&quarantined[0]).unwrap(),
                corrupt,
                "the quarantine must preserve the evidence"
            );
            assert!(
                !path.exists(),
                "the corrupt file must be moved aside, not left to trip every later write"
            );

            // The next locked write starts from the migration default and
            // lands; the DenyAll code scope fails closed in between.
            assert!(!load_disabled_bundles_for(ConnectorScope::Code).is_empty());
            save_disabled_bundles_for(ConnectorScope::Plain, &["pptx".to_string()]).unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["pptx".to_string()]
            );
        });
    }

    /// The degraded read path never writes — not even the quarantine: a
    /// corrupt file read without the lock is reported loudly and left exactly
    /// as found, and the policy surface falls back to its defaults
    /// (project skills off, uninitialized DenyAll scopes fully denied).
    #[test]
    fn corrupt_file_degrades_read_without_persist_or_quarantine() {
        with_temp_home("pinvou3-scope-corrupt-degrade", || {
            let corrupt = "{not json";
            std::fs::write(disabled_bundles_path(), corrupt).unwrap();
            std::fs::create_dir_all(disabled_bundles_lock_path()).unwrap();

            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Code).is_empty(),
                "an uninitialized DenyAll scope must still default to full deny"
            );
            assert!(!project_skills_enabled());

            let content = std::fs::read_to_string(disabled_bundles_path()).unwrap();
            assert_eq!(
                content, corrupt,
                "the degraded read must not rewrite the file"
            );
            let quarantined = std::fs::read_dir(paths::pinvou3_home())
                .unwrap()
                .filter_map(|entry| entry.ok())
                .any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("disabled_bundles.json.corrupt.")
                });
            assert!(!quarantined, "the degraded read must not quarantine either");
        });
    }

    /// Any failure inside the locked critical section must surface as `Err` —
    /// an `Ok` that silently dropped the caller's change would re-open the
    /// fail-open hole on the DenyAll gate. Injected here via a directory at
    /// the data path (the load inside the RMW fails), which pins the load leg
    /// for every write entry point. The pure atomic-write failure leg has no
    /// inline pin — see the note on
    /// `read_degrades_without_persist_when_lock_unavailable` below.
    /// Unlike before the load-refusal gate, even the conditional DenyAll sync
    /// now refuses instead of legitimately no-oping on a fabricated state.
    #[test]
    fn read_failure_refuses_all_write_entry_points() {
        with_temp_home("pinvou3-scope-read-refused", || {
            std::fs::create_dir_all(disabled_bundles_path()).unwrap();
            assert!(
                save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).is_err()
            );
            assert!(save_hidden_bundles_for(ConnectorScope::Plain, &["w".to_string()]).is_err());
            assert!(
                set_project_skills_enabled(true).is_err(),
                "the project-skills toggle must fail loudly when its RMW fails"
            );
            assert!(
                sync_deny_all_scopes_after_install("weather").is_err(),
                "the DenyAll consent-gate sync must refuse, never no-op on a fabricated state"
            );
            assert!(
                remove_bundle_from_disabled_scopes("weather").is_err(),
                "the uninstall/restore cleanup must refuse too"
            );
        });
    }

    /// The pure atomic-write failure (unix-only read-only-home injection) has
    /// no inline pin: the platform selector would violate the architecture
    /// guard's adapter-layer confinement, so it died with the #540 cleanup of
    /// the never-CI-run integration suite (a follow-up may add an injectable
    /// write seam).
    ///
    /// Reads degrade to an unlocked, never-persisting read when the lock is
    /// unavailable: existing data still loads, and the migration path computes
    /// in memory without writing the data file.
    #[test]
    fn read_degrades_without_persist_when_lock_unavailable() {
        with_temp_home("pinvou3-scope-read-degraded", || {
            std::fs::create_dir_all(disabled_bundles_lock_path()).unwrap();
            // Seed a data file directly (the save path is refused without the lock).
            let mut file = DisabledBundlesFile::default();
            file.scopes
                .insert("plain".to_string(), vec!["weather".to_string()]);
            std::fs::write(
                disabled_bundles_path(),
                serde_json::to_string(&file).unwrap(),
            )
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );

            // Migration path (no data file, legacy connectors file present):
            // loads the migrated view without persisting it.
            std::fs::remove_file(disabled_bundles_path()).unwrap();
            let conn = paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(&conn, r#"["weather"]"#).unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            assert!(
                !disabled_bundles_path().exists(),
                "degraded read must not persist"
            );
        });
    }
}
