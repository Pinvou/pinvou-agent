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
use std::sync::atomic::{AtomicU8, Ordering};

use crate::core::session_mode::{PackDefaultPolicy, SessionMode};
use crate::features::marketplace::bundle::{builtin_cli_bundle_ids, skill_owner_package};
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

/// In-process serialization for `disabled_bundles.json` read-modify-write.
static DISABLED_BUNDLES_FILE_LOCK: Mutex<()> = Mutex::new(());

/// Runs the critical section under both locks and never runs `f()` when the
/// cross-process lock cannot be established. The in-process mutex serializes
/// threads; the exclusive flock on `disabled_bundles.lock` serializes
/// **processes** (the desktop app and the headless CLI each hold their own
/// in-process mutex, and both write the whole file — without the cross-process
/// lock a GUI toggle is overwritten by a CLI write and vice versa, the same
/// shape as the #287 two-legacy-files race). The flock uses the same primitive
/// and the same fd-lock crate as remote_control's process lock, and fails
/// closed like it: an unavailable lock returns `Err` instead of running the
/// write unserialized, because silently proceeding would reintroduce exactly
/// the lost-update this lock exists to prevent. The critical section only
/// reads/writes a file-sized payload (sub-millisecond), so blocking is
/// preferable to retry loops.
fn with_disabled_bundles_lock<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    attempt_cross_process_lock(f).map_err(|(error, _unrun)| error)
}

/// Writer variant of [`with_disabled_bundles_lock`] for closures that can
/// themselves fail (corrupt consent-file refusal, persistence errors): both
/// failure kinds flatten into the entry point's `Err`, so an `Ok` from a
/// write entry point means the change landed on disk — lock refusal alone is
/// not the only way a write can be lost.
fn with_disabled_bundles_writer<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    with_disabled_bundles_lock(f).and_then(std::convert::identity)
}

/// One-shot flags for the degraded read warnings (bit 0: cross-process lock
/// unavailable; bit 1: corrupt data file defaulted to empty). Gating reads
/// run on every prompt and tool listing, so an unbounded per-read `eprintln!`
/// would spam stderr and stall the calling thread on exactly the degraded
/// machines these warnings describe.
static DEGRADED_READ_WARNED: AtomicU8 = AtomicU8::new(0);

fn warn_degraded_read_once(bit: u8, message: &str) {
    if DEGRADED_READ_WARNED.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        eprintln!("{message}");
    }
}

/// Read variant of [`with_disabled_bundles_lock`]: same two locks, but an
/// unavailable cross-process lock degrades to in-process-only serialization
/// instead of failing the read. A read cannot corrupt the file, and gating
/// reads run on every prompt/tool listing — refusing them would break the
/// GUI on exactly the degraded machines the lock failure describes. Writers
/// must not use this wrapper: they refuse (see `with_disabled_bundles_lock`).
/// The closure handed to this wrapper must be persistence-free: on the
/// degraded path it runs without the cross-process lock, so any save it
/// performed could clobber a concurrent lock-holding writer's consent state
/// (exactly the lost update the flock exists to prevent). The read entry
/// point therefore pairs with [`load_disabled_bundles_file_readonly_locked`].
fn with_disabled_bundles_lock_read<T>(f: impl FnOnce() -> T) -> T {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match attempt_cross_process_lock(f) {
        Ok(value) => value,
        Err((error, f)) => {
            warn_degraded_read_once(
                1,
                &format!(
                    "[marketplace] {error}; proceeding with in-process locking only \
                 (read-only path)"
                ),
            );
            // The attempt hands the closure back unrun, so the degraded
            // fallback executes it exactly once.
            f()
        }
    }
}

/// Lock-acquisition half shared by both wrappers; the caller must already
/// hold [`DISABLED_BUNDLES_FILE_LOCK`]. `f()` runs exactly once on `Ok` and
/// never on `Err` — on refusal the closure is handed back unrun so the
/// degraded read path can still execute it.
fn attempt_cross_process_lock<T, F: FnOnce() -> T>(f: F) -> Result<T, (String, F)> {
    let lock_path = paths::pinvou3_home().join("disabled_bundles.lock");
    // The first write into a fresh PINVOU3_HOME happens before any other
    // writer has created the directory: create the parent first, otherwise
    // opening the lock would deterministically fail (same acquire-time
    // create_dir_all as remote_control's process lock). The lock file holds
    // nothing sensitive, so private-permission hardening is not pursued.
    if let Some(parent) = lock_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return Err((
                format!(
                    "[marketplace] create {}: {error}; cross-process lock unavailable",
                    parent.display()
                ),
                f,
            ));
        }
    }
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(error) => {
            return Err((
                format!(
                    "[marketplace] open cross-process lock {}: {error}; cross-process \
                     lock unavailable",
                    lock_path.display()
                ),
                f,
            ));
        }
    };
    let mut rw = fd_lock::RwLock::new(file);
    // fd-lock 4's write() returns a borrowing guard (no closure form); the
    // named binding keeps the guard alive until `f()` has returned, and
    // dropping it releases the flock.
    let _flock_guard = loop {
        match rw.write() {
            Ok(guard) => break guard,
            Err(error) => {
                // A caught signal delivered while blocked in flock aborts the
                // wait with EINTR; retrying is the standard convention and
                // keeps a stray signal from failing the write.
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err((
                    format!(
                        "[marketplace] acquire cross-process lock {}: {error}; \
                         cross-process lock unavailable",
                        lock_path.display()
                    ),
                    f,
                ));
            }
        }
    };
    Ok(f())
}

/// Reads the whole file under both locks. The read path that can trigger
/// "load-then-migrate" must serialize with lock-holding writers through this
/// entry point (same race shape as the old two-file #287 bug), and on the
/// degraded path it must not persist anything — see
/// [`load_disabled_bundles_file_readonly_locked`].
pub(crate) fn load_disabled_bundles_file() -> DisabledBundlesFile {
    with_disabled_bundles_lock_read(load_disabled_bundles_file_readonly_locked)
}

/// Locked read implementation used by the degraded path: memory-only. A
/// missing file merges the two legacy files in memory without saving the
/// migration (a concurrent lock-holding writer owns the canonical file); a
/// present file is parsed and `skill:` prefix residuals are stripped in
/// memory only (fresh writers never produce them anymore). Writers must use
/// [`load_disabled_bundles_file_locked`], which persists migration and
/// normalization while holding the cross-process lock.
fn load_disabled_bundles_file_readonly_locked() -> DisabledBundlesFile {
    let path = disabled_bundles_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return migrate_from_legacy_files(),
    };
    let mut file: DisabledBundlesFile = match serde_json::from_str(&content) {
        Ok(file) => file,
        Err(error) => {
            // Reads must never write (the degraded path would clobber a
            // lock-holding writer), so the corrupt file cannot be quarantined
            // here — degrade to the default loudly, once per process.
            warn_degraded_read_once(
                2,
                &format!(
                    "[marketplace] {} is corrupt ({error}); proceeding with the default \
                     consent state until a writer quarantines it",
                    path.display()
                ),
            );
            Default::default()
        }
    };
    strip_skill_prefixes(&mut file);
    file
}

/// Locked read-and-heal implementation for writers: a missing file migrates
/// the two legacy files (idempotent) and persists the result; a present file
/// is parsed and defensive `skill:` prefix stripping is saved back. A present
/// but CORRUPT file refuses with `Err` after quarantining the bytes aside
/// (timestamped, so a second corruption never destroys the first evidence):
/// rebuilding from the default and saving would wipe every other scope's
/// recorded denies on this write. The next write after a quarantine rebuilds
/// from the migration default with the evidence preserved. The in-loader
/// heal saves are best-effort — the caller's final save is the one that must
/// succeed, and its failure propagates. Must run under
/// [`with_disabled_bundles_lock`].
fn load_disabled_bundles_file_locked() -> Result<DisabledBundlesFile, String> {
    let path = disabled_bundles_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            let file = migrate_from_legacy_files();
            if !file.scopes.is_empty() || file.initialized.iter().any(|k| !k.is_empty()) {
                let _ = save_disabled_bundles_file(&file);
            }
            return Ok(file);
        }
    };
    let file: DisabledBundlesFile = match serde_json::from_str(&content) {
        Ok(file) => file,
        Err(error) => {
            let quarantine = path.with_extension(format!(
                "json.corrupt-{}",
                chrono::Utc::now().format("%Y%m%d%H%M%S")
            ));
            let quarantined = match std::fs::rename(&path, &quarantine) {
                Ok(()) => format!(
                    "; the corrupt bytes are quarantined at {} and the next write rebuilds \
                     from defaults",
                    quarantine.display()
                ),
                Err(quarantine_error) => format!(
                    "; quarantining failed ({quarantine_error}) — repair or remove the file \
                     before writing"
                ),
            };
            return Err(format!(
                "[scope] {} is corrupt ({error}); refusing to overwrite the consent state \
                 from the default{quarantined}",
                path.display()
            ));
        }
    };
    let mut file = file;
    if strip_skill_prefixes(&mut file) {
        let _ = save_disabled_bundles_file(&file);
    }
    Ok(file)
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

/// Maps a user-supplied raw id to the package id the persisted list stores,
/// for headless callers (the CLI's toggle read-back verification). A raw
/// skill id is conditionally re-claimed to its owner package, so verifying
/// against the raw id yields false positives.
pub fn package_id_for(raw: &str) -> String {
    to_package_id(raw)
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

/// Writes the whole file (atomic replace, same pattern as the legacy file).
/// Failures must propagate: a write entry point returning `Ok` means the
/// change landed on disk — swallowing a disk failure would let the GUI treat
/// `Ok` as success and hot-refresh from stale state (fail-open), while this
/// file's write semantics are fail-closed.
fn save_disabled_bundles_file(file: &DisabledBundlesFile) -> Result<(), String> {
    let json = serde_json::to_string(file)
        .map_err(|error| format!("[scope] serialize disabled_bundles.json failed: {error}"))?;
    deepseek_tui::utils::write_atomic(&disabled_bundles_path(), json.as_bytes())
        .map_err(|error| format!("[scope] write disabled_bundles.json failed: {error}"))
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
///
/// Fails closed: when the cross-process lock cannot be established the write
/// is refused with `Err` instead of running unserialized (writers from the
/// GUI and the CLI processes would overwrite each other whole-file).
pub fn save_disabled_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    with_disabled_bundles_writer(|| {
        let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
        let mut file = load_disabled_bundles_file_locked()?;
        let key = scope.as_str().to_string();
        file.scopes.insert(key.clone(), normalized);
        file.initialized.insert(key);
        save_disabled_bundles_file(&file)
    })
}

/// Single-critical-section read-modify-write of one scope's disabled package
/// id list, serialized both in-process and across the GUI/CLI processes. A
/// per-scope load→save across two lock acquisitions loses concurrent writes
/// in the inter-lock window (same shape as M-6b: while the GUI toggles
/// exclusively, a whole CLI disable can be dropped); the CLI's enable/disable
/// and the lock-holding writers share this entry point. The closure receives
/// the effective list including the DenyAll fallback, matching
/// `load_disabled_bundles_for`. The closure runs under both locks and must
/// not re-enter this module's load/save helpers (the in-process mutex is not
/// reentrant — it would self-deadlock). Fails closed: when the cross-process
/// lock cannot be established the write is refused with `Err` instead of
/// running unserialized, a corrupt consent file is refused too (its bytes are
/// quarantined first), and a failed disk write propagates — an `Ok` means the
/// change landed.
pub fn update_disabled_bundles_for(
    scope: ConnectorScope,
    update: impl FnOnce(&mut Vec<String>),
) -> Result<(), String> {
    with_disabled_bundles_writer(|| {
        let file = load_disabled_bundles_file_locked()?;
        let mut ids = resolve_scope_disabled_ids(&file, scope);
        update(&mut ids);
        let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
        let mut file = file;
        let key = scope.as_str().to_string();
        file.scopes.insert(key.clone(), normalized);
        file.initialized.insert(key);
        save_disabled_bundles_file(&file)
    })
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
///
/// Fails closed like the other writers: an unavailable cross-process lock
/// refuses the write with `Err`.
pub fn save_hidden_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    with_disabled_bundles_writer(|| {
        let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
        let mut file = load_disabled_bundles_file_locked()?;
        file.hidden_scopes
            .insert(scope.as_str().to_string(), normalized);
        save_disabled_bundles_file(&file)
    })
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

/// 写全局（plain）被禁用的包 id 列表。兼容既有调用方。
pub fn save_disabled_bundles(ids: &[String]) -> Result<(), String> {
    save_disabled_bundles_for(ConnectorScope::Plain, ids)
}

/// 包安装/连接后同步所有 DenyAll 且已初始化的 scope：用户已改过这类会话开关时，
/// 新装的包默认仍保持关闭（加入该 scope 禁用集）；未初始化时无需处理（load 会按
/// 「默认全禁已装包」兜底）。AllowAll 模式无需同步（默认全开）。连接器与技能安装
/// 共用本入口：入参可为连接器 id / 技能 id / 包 id，统一归一为包 id。
///
/// Fails closed like the other writers: an unavailable cross-process lock
/// refuses the write with `Err`. Propagating matters here — a skipped sync
/// would leave a freshly installed bundle enabled in DenyAll scopes, the
/// opposite of the user's standing default.
pub fn sync_deny_all_scopes_after_install(raw_id: &str) -> Result<(), String> {
    let package_id = to_package_id(raw_id);
    with_disabled_bundles_writer(|| {
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
            save_disabled_bundles_file(&file)?;
        }
        Ok(())
    })
}

/// Removes the package id from every scope's disabled and hidden lists so no
/// stale entry survives an uninstall. Shared entry point for connector, skill,
/// and package teardown: the argument may be a connector id / skill id /
/// package id and is normalized to the package id. Fails closed like the
/// other writers: an unavailable cross-process lock refuses the write with
/// `Err`.
pub fn remove_bundle_from_disabled_scopes(raw_id: &str) -> Result<(), String> {
    let package_id = to_package_id(raw_id);
    with_disabled_bundles_writer(|| {
        let mut file = load_disabled_bundles_file_locked()?;
        let mut changed = false;
        for ids in file.scopes.values_mut() {
            let before = ids.len();
            ids.retain(|id| id != &package_id);
            changed |= ids.len() != before;
        }
        // The hidden sets are cleaned the same way: a stale hidden entry left
        // behind by an uninstall would wrongly hide a future reinstall of the
        // same id.
        for ids in file.hidden_scopes.values_mut() {
            let before = ids.len();
            ids.retain(|id| id != &package_id);
            changed |= ids.len() != before;
        }
        if changed {
            save_disabled_bundles_file(&file)?;
        }
        Ok(())
    })
}

/// 项目级 skills 开关（默认关）。
pub fn project_skills_enabled() -> bool {
    load_disabled_bundles_file().project_skills_enabled
}

/// 写项目级 skills 开关。落盘后由调用方重写在线会话组合目录。
///
/// Fails closed like the other writers: an unavailable cross-process lock
/// refuses the write with `Err`.
pub fn set_project_skills_enabled(enabled: bool) -> Result<(), String> {
    with_disabled_bundles_writer(|| {
        let mut file = load_disabled_bundles_file_locked()?;
        if file.project_skills_enabled == enabled {
            return Ok(());
        }
        file.project_skills_enabled = enabled;
        save_disabled_bundles_file(&file)
    })
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

    /// The lock acquire path must create a missing PINVOU3_HOME before
    /// opening the lock file: the first scope write into a fresh home would
    /// otherwise refuse (fail-closed) on every fresh install. Unlike
    /// `with_temp_home`, the home directory is deliberately NOT pre-created.
    #[test]
    fn writer_creates_a_fresh_home_before_taking_the_lock() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let parent =
            std::env::temp_dir().join(format!("pinvou3-scope-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        let home = parent.join("fresh-home");
        assert!(!home.exists(), "precondition: home must not exist yet");
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };
        let saved = save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]);
        match prev {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&parent);
        saved.expect("first write into a fresh PINVOU3_HOME must create the home and succeed");
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

    /// Writers go through the cross-process lock file: the GUI app and the
    /// headless CLI are two processes whose in-process mutexes cannot see each
    /// other, so the flock on `disabled_bundles.lock` (same primitive as
    /// remote_control's process lock) is the only serialization point they
    /// share. The lock file must be created with the first write.
    #[test]
    fn writers_hold_the_cross_process_lock_file() {
        with_temp_home("pinvou3-scope", || {
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            assert!(
                paths::pinvou3_home().join("disabled_bundles.lock").exists(),
                "cross-process lock file must exist after the first write"
            );
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()],
                "the write must survive the double critical section"
            );
        });
    }

    /// A corrupt consent file must never be overwritten from the default: the
    /// parse failure may hide other scopes' recorded denies, and a
    /// default-based save would wipe them without evidence. The write is
    /// refused, the corrupt bytes are quarantined (renamed aside, preserved),
    /// and the NEXT write rebuilds from the migration default.
    #[test]
    fn corrupt_consent_file_refuses_the_write_and_quarantines() {
        with_temp_home("pinvou3-scope", || {
            let path = disabled_bundles_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let corrupt = r#"{"scopes": {"plain": ["we"#;
            std::fs::write(&path, corrupt).unwrap();

            let error = save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()])
                .unwrap_err();
            assert!(
                error.contains("corrupt") && error.contains("refusing"),
                "the refusal must name the corruption: {error}"
            );
            // The canonical file is gone (renamed aside) and the evidence is
            // preserved verbatim under the timestamped quarantine name.
            assert!(!path.exists(), "the corrupt file must be renamed aside");
            let mut evidence = std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    path.to_str()
                        .is_some_and(|p| p.contains(".json.corrupt-"))
                        .then_some(path)
                })
                .collect::<Vec<_>>();
            assert_eq!(
                evidence.len(),
                1,
                "exactly one quarantine file must exist: {evidence:?}"
            );
            let preserved = std::fs::read_to_string(evidence.pop().unwrap()).unwrap();
            assert_eq!(preserved, corrupt, "the corrupt bytes must survive");

            // The next write rebuilds from the migration default and succeeds.
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()],
                "the store must be writable again after the quarantine"
            );
        });
    }

    /// Fail-closed covers persistence too: a write entry point returning `Ok`
    /// means the change landed on disk. Replacing the data file with a
    /// directory makes the atomic replace fail, so the writer must return
    /// `Err` instead of reporting success from stale state.
    #[test]
    fn write_entry_points_propagate_persistence_failures() {
        with_temp_home("pinvou3-scope", || {
            let path = disabled_bundles_path();
            std::fs::create_dir_all(&path).unwrap();

            let error = save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()])
                .unwrap_err();
            assert!(
                error.contains("write disabled_bundles.json failed"),
                "the error must name the failed write: {error}"
            );
            assert!(
                update_disabled_bundles_for(ConnectorScope::Plain, |_| {}).is_err(),
                "the RMW must propagate the same persistence failure"
            );
        });
    }

    /// Restores the previous `PINVOU3_HOME` value on drop — on normal return
    /// and on panic unwind — so a failing test cannot pollute later tests.
    /// `OsString` preserves non-Unicode values.
    struct HomeGuard(Option<std::ffi::OsString>);
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: constructed while ENV_LOCK is held; env writes are
            // serialized in-process.
            unsafe {
                match self.0.take() {
                    Some(value) => std::env::set_var("PINVOU3_HOME", value),
                    None => std::env::remove_var("PINVOU3_HOME"),
                }
            }
        }
    }

    /// Fail-closed: when the lock file cannot be opened the write is refused
    /// with `Err` and the critical section never runs — running it would
    /// reintroduce exactly the cross-process lost-update the lock exists to
    /// prevent. The read path degrades instead and still reflects the
    /// untouched state.
    #[test]
    fn writer_refuses_when_lock_file_cannot_be_opened() {
        with_temp_home("pinvou3-scope", || {
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            let lock_path = paths::pinvou3_home().join("disabled_bundles.lock");
            std::fs::remove_file(&lock_path).unwrap();
            std::fs::create_dir(&lock_path).unwrap();
            let ran = std::cell::Cell::new(false);
            let result = update_disabled_bundles_for(ConnectorScope::Plain, |ids| {
                ran.set(true);
                ids.push("unserialized".to_string());
            });
            assert!(
                result.is_err(),
                "the write must be refused while the lock file cannot be opened"
            );
            assert!(
                !ran.get(),
                "the critical section must not run without the cross-process lock"
            );
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()],
                "the refused write must leave the persisted state untouched"
            );
        });
    }

    /// Fail-closed at the earliest stage: when the lock file's parent
    /// directory cannot be created (here: the home's parent is a regular
    /// file — the realistic fresh-home failure), the write is refused, the
    /// closure never runs, and no state file is created.
    #[test]
    fn writer_refuses_when_lock_directory_cannot_be_created() {
        let env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let base = std::env::temp_dir().join(format!(
            "pinvou3-scope-lockdir-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_file(&base);
        std::fs::write(&base, b"not a directory").unwrap();
        let prev = std::env::var_os("PINVOU3_HOME");
        // SAFETY: ENV_LOCK held; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", base.join("home")) };
        let _home = HomeGuard(prev);
        let ran = std::cell::Cell::new(false);
        let result = update_disabled_bundles_for(ConnectorScope::Plain, |ids| {
            ran.set(true);
            ids.push("unserialized".to_string());
        });
        assert!(
            result.is_err(),
            "the write must be refused while the lock directory cannot be created"
        );
        assert!(
            !ran.get(),
            "the critical section must not run without the cross-process lock"
        );
        assert!(
            !paths::pinvou3_home().join("disabled_bundles.json").exists(),
            "the refused write must not create the state file"
        );
        drop(_home);
        drop(env_lock);
        let _ = std::fs::remove_file(&base);
    }

    /// The degraded read must be persistence-free: when the cross-process
    /// lock is unavailable (here: the lock path is a directory, so opening
    /// it fails) the read still returns the effective view — legacy entries
    /// merged in memory — but must not save the migration or any
    /// normalization, because it would run without the flock and could
    /// clobber a concurrent lock-holding writer's consent state.
    #[test]
    fn degraded_read_never_persists_the_migration() {
        with_temp_home("pinvou3-scope", || {
            // Seed the legacy two-file layout so an in-memory migration has
            // something to merge (the canonical file stays absent).
            std::fs::write(
                paths::pinvou3_home().join("disabled_connectors.json"),
                serde_json::to_string(&vec!["weather".to_string()]).unwrap(),
            )
            .unwrap();
            // Make the cross-process lock unopenable: a directory where the
            // lock file should be.
            std::fs::create_dir(paths::pinvou3_home().join("disabled_bundles.lock")).unwrap();

            let loaded = load_disabled_bundles_for(ConnectorScope::Plain);
            assert!(
                loaded.contains(&"weather".to_string()),
                "the degraded read must still surface the legacy entries"
            );
            assert!(
                !disabled_bundles_path().exists(),
                "the degraded read must not persist the migrated canonical file"
            );
        });
    }

    /// Single-critical-section RMW semantics: the closure receives the
    /// **effective list** (an uninitialized DenyAll scope expands to the
    /// currently claimed set ∪ built-in CLI packages, the same view
    /// `load_disabled_bundles_for` returns), and writing back marks the scope
    /// initialized — from then on the read path trusts the persisted list and
    /// the fallback no longer expands.
    #[test]
    fn update_disabled_bundles_for_writes_effective_list_and_initializes() {
        with_temp_home("pinvou3-scope", || {
            // Uninitialized Code (DenyAll): the read path and the closure
            // input must be the same effective list.
            let effective = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(
                !effective.is_empty(),
                "the DenyAll fallback must expand to the effective set"
            );
            update_disabled_bundles_for(ConnectorScope::Code, |ids| {
                assert_eq!(
                    *ids, effective,
                    "closure input must match the read path's effective list"
                );
                ids.clear();
                ids.push("kept-pkg".to_string());
            })
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["kept-pkg".to_string()],
                "the write-back must freeze the scope as initialized"
            );
            // RMW after initialization: the closure sees the persisted list,
            // not the fallback expansion (idempotent read-modify-write).
            update_disabled_bundles_for(ConnectorScope::Code, |ids| {
                assert_eq!(ids.as_slice(), ["kept-pkg".to_string()].as_slice());
            })
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["kept-pkg".to_string()]
            );
        });
    }

    /// The flock's actual exclusion: while another fd in this process holds
    /// the lock (flock conflicts per open file description, equivalent to
    /// another process), a writer must block until the lock is released. A
    /// "create the lock file but forget to flock" regression stays green under
    /// an existence assertion and only turns red here.
    #[test]
    fn cross_process_lock_blocks_a_concurrent_writer() {
        with_temp_home("pinvou3-scope", || {
            let lock_path = paths::pinvou3_home().join("disabled_bundles.lock");
            let stand_in = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)
                .unwrap();
            let mut rw = fd_lock::RwLock::new(stand_in);
            let guard = rw.write().expect("hold the stand-in process lock");
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            let writer = std::thread::spawn(move || {
                save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
                tx.send(()).expect("signal writer completion");
            });
            // A non-blocking write over a small file returns in well under a
            // millisecond; still incomplete after 500ms proves it is waiting on
            // the lock.
            assert!(
                rx.recv_timeout(std::time::Duration::from_millis(500))
                    .is_err(),
                "a writer must block while another process holds the flock"
            );
            drop(guard);
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .expect("the writer completes once the flock is released");
            writer.join().unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()],
                "the blocked write must land intact after the lock is released"
            );
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

    /// 读路径的「读到即迁移落盘」必须取 `DISABLED_BUNDLES_FILE_LOCK` 与持锁写方
    /// 串行：持锁期间并发 load（磁盘为旧连接器文件、必然触发迁移落盘）不得先行落盘。
    #[test]
    fn read_path_stays_persistence_free_and_serialized() {
        with_temp_home("pinvou3-scope", || {
            let legacy = r#"["weather"]"#;
            let conn = paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(&conn, legacy).unwrap();
            let guard = DISABLED_BUNDLES_FILE_LOCK
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let reader = std::thread::spawn(load_disabled_bundles_for_plain_for_lock_test);
            std::thread::sleep(std::time::Duration::from_millis(200));
            // While the lock is held the read must not write the migration.
            assert!(
                !disabled_bundles_path().exists(),
                "the read path must not persist the migration while the lock is held"
            );
            drop(guard);
            assert_eq!(reader.join().unwrap(), vec!["weather".to_string()]);
            // The read is persistence-free even after the lock releases: a
            // degraded (lock-less) read runs the same closure, so it must not
            // save anything a lock-holding writer could clobber. Only writers
            // materialize the canonical file.
            assert!(
                !disabled_bundles_path().exists(),
                "the read path must never persist the migrated canonical file"
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
}
