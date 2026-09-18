//! 包 id × SessionMode 的单一禁用集（`~/.pinvou3/disabled_bundles.json`）。
//!
//! 这是「工具市场统一治理」scope 收敛（todo A 节）的单一真相源：取代原先
//! `disabled_connectors.json`（连接器 id）与 `disabled_skills.json`（技能 id）两份
//! 文件。开关粒度收敛为**包 id**（= `bundle.rs` 里 `BundleInfo.id`，即 MCP 工具 id /
//! 技能 id / CLI 连接器 id），一个包 = 一个开关，包内技能（companion skills）可见性
//! 唯一跟随所属包（§5.2 不变量）。
//!
//! 落盘格式与 #287 泛化后的两份旧文件同构：`{scopes: {"<mode>": [...]},
//! "initialized": ["<mode>"], project_skills_enabled, plain_defaults_migrated}`，
//! Scope keys are the kebab-case names of `SessionMode`. `plain_defaults_migrated`
//! is the plain scope default-policy migration marker: an old file (false) was
//! written while plain was still AllowAll; read-time migration initializes plain
//! to the persisted list and then sets the marker on disk. The first version
//! migrates the two legacy files into this file on read (migrate-on-read):
//! 旧连接器 id 原样进包 id（连接器 id 即包 id）；旧技能 id 经 `bundle::skill_owner_package`
//! 映射到所属包（companion → MCP/CLI 包，独立技能 → 自身）；`skill:` 前缀跨文件借道
//! 残留统一剥除并清出连接器文件。迁移幂等，失败回退默认值（安全兜底）。
//!
//! 依赖方向：本模块与 `bundle` / `skill_marketplace` 同属 marketplace 领域，只依赖
//! `platform::paths` 与 marketplace 内既有类型，不反向依赖 assistant 运行时。

use std::path::PathBuf;
use std::sync::Mutex;

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
    /// scope → the disabled entries of that scope that the **install default**
    /// wrote (round-11 B2), as opposed to an explicit user switch-off. `scopes`
    /// stays the single gating input; this table only answers "who wrote this
    /// off": a batch enable is refused wholesale only for `scopes` ids that are
    /// **absent** here (explicit user opt-outs), while install-default offs may
    /// be lifted by a user action (welcome card / scene opt-in). Install sync
    /// writes stored+this table; user disable writes stored only; a composer
    /// whole-list write keeps the markers it can still attribute and drops the
    /// rest; migration-seeded lists never appear here (pre-upgrade state =
    /// user-explicit).
    #[serde(default)]
    pub default_off_scopes: std::collections::BTreeMap<String, Vec<String>>,
    /// 已被用户显式初始化（改过开关）的 scope 集合。
    #[serde(default)]
    pub initialized: std::collections::BTreeSet<String>,
    /// Whether project-level skills are enabled (default off; effective only
    /// when the session is bound to a project/work directory — not code-only;
    /// see skill_materialization.rs). Moved here with the skills side.
    #[serde(default)]
    pub project_skills_enabled: bool,
    /// plain scope default-policy migration marker: false (field absent in old
    /// files) = the file was written while plain was still AllowAll; read-time
    /// migration initializes plain to the persisted list (locking in the actual
    /// on/off state at that time) and then sets true — existing users keep their
    /// switch state after upgrading. A fresh install sets true on first read
    /// without initializing plain, and **likewise persists the frozen verdict
    /// to disk** (first-boot self-written settings.json/sessions pollute the
    /// upgrade signal — see the read-path comments); uninitialized plain falls
    /// back to DenyAll (default fully off).
    #[serde(default)]
    pub plain_defaults_migrated: bool,
    /// 未知键原样保留（前向兼容）。
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

fn disabled_bundles_path() -> PathBuf {
    paths::pinvou3_home().join("disabled_bundles.json")
}

/// `disabled_bundles.json` 读-改-写的进程内串行化。
///
/// Lock order (round-11 M1, shared convention with
/// `MARKETPLACE_TRANSACTION_LOCK`): only TRANSACTION → FILE nesting is allowed
/// (e.g. uninstall holds the transaction lock for switch cleanup); this lock's
/// holder **must not** acquire the transaction lock — corrupt `installed.json`
/// recovery on the DenyAll resolution / switch-write paths rebuilds in memory
/// only, taking no lock and persisting nothing (see the read-only recovery
/// branch of `try_installed_ids`); the next writer holding the transaction lock
/// persists it.
static DISABLED_BUNDLES_FILE_LOCK: Mutex<()> = Mutex::new(());

/// In-process verdict memo for freeze persist failures (review #455 R7-M2):
/// when the "fresh vs upgraded" verdict could not be persisted, later reads in
/// the same process **must not** re-evaluate using first-boot self-written
/// traces — a fresh install would be misjudged as an upgrade and plain would
/// flip back to fully on (fail-open, exactly what the freeze prevents). Keyed
/// by home-directory path so tests switching PINVOU3_HOME do not cross-talk;
/// after a successful save the file is the truth, and this memo only briefly
/// carries the verdict on a write failure.
static UNPERSISTED_VERDICT: Mutex<Option<(PathBuf, DisabledBundlesFile)>> = Mutex::new(None);

/// In-process memo for corrupt-recovery "quarantine kept, overwrite save
/// failed" (review #455 R9-M1): when the recovery save fails the corrupt
/// original is still on disk; without remembering this, the next read would
/// re-quarantine (fresh nanosecond timestamp) → `.corrupt.*` copies accumulate
/// unboundedly — the very behavior this PR flags as a blocker elsewhere. Reads
/// hitting the memo reuse the in-memory fail-closed state directly; any
/// successful save (the file becomes valid JSON again) clears it, and the
/// process self-heals.
static PENDING_CORRUPT_RECOVERY: Mutex<Option<(PathBuf, DisabledBundlesFile)>> = Mutex::new(None);

/// Clears the verdict memos. Test-only: with_temp_home reuses a pid-keyed temp
/// directory, so the previous case's memo would be matched by path and bleed
/// into the next one; production paths never need to clear (the file is the
/// truth; the memo only briefly carries the verdict after a write failure).
#[cfg(test)]
pub(crate) fn clear_unpersisted_verdict_for_test() {
    *UNPERSISTED_VERDICT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *PENDING_CORRUPT_RECOVERY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// 读完整文件（取文件锁）。可能触发「读到即迁移」的读路径必须走本入口与持锁写方
/// 串行（与旧两份文件的 #287 竞态范式一致）。
pub(crate) fn load_disabled_bundles_file() -> DisabledBundlesFile {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    load_disabled_bundles_file_locked()
}

/// 已持锁读实现。首个版本：文件不存在时从两份旧文件迁移（幂等）；文件存在时按新
/// 格式解析，防御性剥除 `skill:` 前缀残留（新写路径不会再产生）。
///
/// plain default-policy migration (tool switches converge fully to DenyAll):
/// an old file (no `plain_defaults_migrated` field) or the legacy two-file
/// era (legacy files present) = upgraded install, initialize plain to the
/// persisted list — its effective state is the real switch state under the old
/// AllowAll semantics (default empty = fully on), so users notice nothing after
/// upgrading; a fresh install only sets the marker without initializing, and
/// uninitialized plain falls back to DenyAll (default fully off).
///
/// The "fresh install" verdict cannot look only at this file and the two
/// legacy files: the unified file has existed since v0.8.6 and is only
/// persisted when there is content to write — an old install whose user never
/// touched a switch may have none of the three. The upgrade signal is
/// therefore widened to two concrete paths: marketplace/installed.json or a
/// non-empty sessions/ directory; either present marks an upgraded install
/// that keeps the old AllowAll semantics (review #445 P1-2; R8-3 narrowing:
/// settings.json removed from the signal — preset/cross-machine-copied
/// settings.json would misjudge fail-open, and a real old install normally
/// leaves a non-empty sessions/). The narrowing is NOT miss-free (R11-M4): an
/// upgraded install whose sessions/ was wiped by tooling, with no
/// installed.json and no legacy files, is misjudged fresh — the fail-closed
/// direction (default fully off), and the two populations are
/// indistinguishable without a persisted version marker (registered
/// follow-up). Note this is a
/// whitelist-style signal, not "any home-directory trace" — other files (logs,
/// caches, etc.) do not count as upgrade evidence; the install is fresh only
/// when both are absent; do not widen beyond the criteria in this comment
/// (review #455 R5-m1).
///
/// This wide upgrade signal is polluted by the app's own first-boot behavior
/// (bridge boot's ensure_dirs self-writes sessions/default/artifacts/ and
/// back-fills the default settings.json), so the first read is hoisted to the
/// top of the Tauri setup hook (the lib.rs `disabled_bundles_migration`
/// marker, ahead of every first-boot self-written trace), and the
/// "upgraded vs fresh" verdict is **unconditionally, at the first read,
/// persisted to disk** (setting the `plain_defaults_migrated` marker) as a
/// freeze — otherwise a fresh install is misjudged as an upgrade by first-boot
/// traces and flips back to fully on (review #455 blocker).
fn load_disabled_bundles_file_locked() -> DisabledBundlesFile {
    let path = disabled_bundles_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            {
                // Verdict not yet persisted: this process keeps the first
                // verdict, denying first-boot traces a chance to re-evaluate.
                let memo = UNPERSISTED_VERDICT
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some((home, file)) = memo.as_ref() {
                    if *home == paths::pinvou3_home() {
                        return file.clone();
                    }
                }
            }
            let home = paths::pinvou3_home();
            let legacy_existed = home.join("disabled_connectors.json").exists()
                || home.join("disabled_skills.json").exists();
            // Wide upgrade signal (review #455 R8-3 narrowing): none of the
            // three switch-related files exist, but an install record or a
            // non-empty sessions directory is present ⇒ old install, plain
            // keeps the old AllowAll semantics. settings.json is **not**
            // upgrade evidence — a preset template or cross-machine-copied
            // settings.json would misjudge a fresh install as upgraded (plain
            // fully on, fail-open), while a real old install normally leaves
            // a non-empty sessions/ (first-boot ensure_dirs self-writes
            // sessions/default/artifacts). The narrowing is not miss-free
            // (R11-M4): an old install whose sessions/ was wiped by tooling
            // and that has no installed.json / legacy files is misjudged
            // fresh — fail-closed direction; indistinguishable without a
            // persisted version marker (registered follow-up).
            // Other home-directory state such as logs/ is likewise no evidence.
            let upgraded_install = legacy_existed
                || home.join("marketplace").join("installed.json").is_file()
                || paths::sessions_root()
                    .read_dir()
                    .map(|mut entries| entries.next().is_some())
                    .unwrap_or(false);
            let mut file = migrate_from_legacy_files();
            if upgraded_install {
                // Upgraded: initialize plain (scopes default empty = fully on
                // under the old semantics), locking in the pre-upgrade state.
                file.initialized
                    .insert(SessionMode::Plain.as_str().to_string());
            }
            file.plain_defaults_migrated = true;
            // Persist the frozen verdict unconditionally: if a fresh install
            // does not persist the marker, first-boot self-written
            // settings.json/sessions/default pollute the wide upgrade signal
            // and the next read is misjudged as an upgraded install that
            // flips back to fully on (review #455 blocker). On a persist
            // failure, record this verdict in the in-process memo (R7-M2) —
            // later reads reuse it; the fail-closed direction is guaranteed
            // by the verdict itself (fresh = plain uninitialized = DenyAll
            // fallback).
            if let Err(freeze_error) = try_save_disabled_bundles_file(&file) {
                eprintln!(
                    "[scope] CRITICAL: failed to persist the plain-defaults migration verdict: {freeze_error}; holding the in-process verdict (plain initialized = {}) until restart - first-boot traces will not re-open the fresh/upgraded evaluation",
                    file.initialized.contains(SessionMode::Plain.as_str())
                );
                *UNPERSISTED_VERDICT
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((home, file.clone()));
            }
            return file;
        }
        Err(error) => {
            // File exists but is unreadable (permissions/lock in use, etc.):
            // fail-closed, same treatment as corruption; must not fold into
            // the migration branch above — on an upgraded install that would
            // initialize plain as empty (old AllowAll fully on) and overwrite
            // the original without quarantine, silently destroying the user's
            // explicit opt-outs (review #455 R4-B1). Branch on the salvage
            // read: bytes readable (non-UTF-8 goes lossy) → the quarantine
            // copy genuinely preserves the original bytes, and the degraded
            // overwrite completes recovery in one shot; quarantine failed or
            // bytes themselves unreadable → leave the original untouched — a
            // placeholder "quarantine" preserves no bytes, and overwriting
            // then would turn "unreadable but recoverable" into "permanently
            // lost" (review #455 R6-B1). The in-memory fail-closed state is
            // already correct; the next read retries.
            let recovered = DisabledBundlesFile {
                plain_defaults_migrated: true,
                ..DisabledBundlesFile::default()
            };
            match std::fs::read(&path) {
                Ok(bytes) => {
                    // Raw bytes quarantine (R7-M1: a lossy copy is mojibake) with
                    // the shared memo/try-save recovery core (round-10 m5).
                    return quarantine_and_recover_disabled_bundles(&bytes, &error.to_string());
                }
                Err(salvage_error) => {
                    eprintln!(
                        "[marketplace] disabled_bundles.json exists but is unreadable ({error}; salvage read failed: {salvage_error}); skipping quarantine and overwrite this read, fail-closed applies in memory"
                    );
                    return recovered;
                }
            }
        }
    };
    let mut file: DisabledBundlesFile = match serde_json::from_str(&content) {
        Ok(file) => file,
        Err(error) => {
            // Never silently overwrite a corrupt file: first keep a
            // .corrupt.<ts> quarantine copy (same as installed.json), then
            // **overwrite** the degraded state to disk — recovery must
            // complete in one shot, otherwise the corrupt file stays on disk,
            // every read re-quarantines, and copies accumulate unboundedly
            // (review #455 blocker). Recovery must be fail-closed: degrading
            // to an empty state would restore a new-format file carrying the
            // migration marker (where the user may have explicitly disabled
            // packs) to fully on; a security-convergence feature had better
            // recover fully off — set only the migration marker (frozen as
            // the fresh-install verdict) and initialize no scope;
            // uninitialized scopes fall back to DenyAll (review #455).
            quarantine_and_recover_disabled_bundles(content.as_bytes(), &error.to_string())
        }
    };
    if !file.plain_defaults_migrated {
        file.initialized
            .insert(SessionMode::Plain.as_str().to_string());
        file.plain_defaults_migrated = true;
        save_disabled_bundles_file(&file);
    }
    if normalize_stored_lists(&mut file) {
        save_disabled_bundles_file(&file);
    }
    file
}

/// Corrupt-file recovery core shared by the parse-error and unreadable-salvage
/// branches (round-10 m5): consult the PENDING_CORRUPT_RECOVERY memo (a prior
/// quarantine succeeded but the overwrite save failed — reuse instead of
/// re-quarantining), quarantine the raw bytes, then try the one-shot fail-
/// closed overwrite; on save failure record the memo and leave the original
/// in place. The recovered state never initializes any scope (DenyAll
/// fallback) — see the branch comments for the consent rationale.
fn quarantine_and_recover_disabled_bundles(raw: &[u8], error: &str) -> DisabledBundlesFile {
    let recovered = DisabledBundlesFile {
        plain_defaults_migrated: true,
        ..DisabledBundlesFile::default()
    };
    {
        let memo = PENDING_CORRUPT_RECOVERY
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((home, file)) = memo.as_ref() {
            if *home == paths::pinvou3_home() {
                return file.clone();
            }
        }
    }
    if let Err(quarantine_err) = quarantine_corrupt_disabled_bundles(raw, error) {
        eprintln!(
            "[marketplace] {quarantine_err}; skipping disabled_bundles.json overwrite this read"
        );
        return recovered;
    }
    if let Err(save_error) = try_save_disabled_bundles_file(&recovered) {
        eprintln!(
            "[marketplace] {save_error}; corrupt recovery overwrite failed - holding the in-memory fail-closed state, re-quarantine suppressed until a save succeeds"
        );
        *PENDING_CORRUPT_RECOVERY
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some((paths::pinvou3_home(), recovered.clone()));
        return recovered;
    }
    recovered
}

/// Defensive: normalize the entries of every scope's disabled and invisible
/// sets to package ids (strip the `skill:` prefix + map companions by the
/// current claim), deduplicating while preserving order, and return whether
/// anything changed. **Persist** the normalization (review #455 R5-m6): the
/// write paths' removal/enable match by normalized id, so pre-claim-flip raw
/// entries match nothing, survive an uninstall (which likewise cleans by
/// normalized matching), and revive the user's removed disabled/hidden state
/// once a same-named pack is reinstalled — read-time normalization (F4) only
/// fixes the gating criteria, not the storage itself; every writer goes
/// through `load_disabled_bundles_file_locked` → save, so persisting here
/// converges the whole file.
fn normalize_stored_lists(file: &mut DisabledBundlesFile) -> bool {
    let mut changed = false;
    for ids in file
        .scopes
        .values_mut()
        .chain(file.hidden_scopes.values_mut())
        .chain(file.default_off_scopes.values_mut())
    {
        let normalized = normalize_stored_pkg_ids(ids);
        if normalized.len() != ids.len() || normalized.iter().zip(ids.iter()).any(|(a, b)| a != b) {
            *ids = normalized;
            changed = true;
        }
    }
    changed
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

/// 首启迁移：读旧 `disabled_connectors.json` + `disabled_skills.json`（各兼容三种
/// 旧形态），把条目映射为包 id 后按 scope 取并集，`project_skills_enabled` 取自技能
/// 文件。迁移不删旧文件（本版本内保留为惰性历史，只读新文件；下个版本周期随
/// 旧布局退役一并清理，见 todo C 节）。
fn migrate_from_legacy_files() -> DisabledBundlesFile {
    let mut file = DisabledBundlesFile::default();
    merge_connector_scopes_into(&mut file);
    merge_skill_scopes_into(&mut file);
    file
}

/// 把旧 `disabled_connectors.json` 的各 scope 条目映射为包 id 并并进 `file`。
fn merge_connector_scopes_into(file: &mut DisabledBundlesFile) {
    let path = paths::pinvou3_home().join("disabled_connectors.json");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    // 裸数组 → plain scope
    if let Ok(list) = serde_json::from_str::<Vec<String>>(&content) {
        let ids: Vec<String> = list.iter().map(|id| to_package_id(id)).collect();
        if !ids.is_empty() {
            file.scopes
                .insert(SessionMode::Plain.as_str().to_string(), ids);
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
                    file.scopes.insert(key.clone(), ids);
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
                    file.scopes.insert(key.to_string(), ids);
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
}

/// 把旧 `disabled_skills.json` 的各 scope 条目映射为包 id 并并进 `file`（取并集），
/// 并继承 `project_skills_enabled`。
fn merge_skill_scopes_into(file: &mut DisabledBundlesFile) {
    let path = paths::pinvou3_home().join("disabled_skills.json");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    // 裸数组 → plain scope
    if let Ok(list) = serde_json::from_str::<Vec<String>>(&content) {
        let ids: Vec<String> = list.iter().map(|id| to_package_id(id)).collect();
        if !ids.is_empty() {
            merge_ids_into_scope(file, SessionMode::Plain.as_str(), ids);
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
                merge_ids_into_scope(file, key, ids);
            }
        }
        if let Some(initialized) = obj.get("initialized").and_then(|v| v.as_array()) {
            for key in initialized.iter().filter_map(|v| v.as_str()) {
                file.initialized.insert(key.to_string());
            }
        }
    } else {
        for key in ["plain", "code"] {
            if let Some(arr) = obj.get(key).and_then(|v| v.as_array()) {
                let ids: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(to_package_id))
                    .collect();
                merge_ids_into_scope(file, key, ids);
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
    if let Some(enabled) = obj.get("project_skills_enabled").and_then(|v| v.as_bool()) {
        file.project_skills_enabled = enabled;
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

/// Write the full file (atomic replace, same as the legacy files). Create the
/// parent directory first: the first persist of a migration freeze or corrupt
/// recovery may precede any directory-creating startup step, `write_atomic`
/// does not create parent directories, and a failed write would silently
/// unfreeze the "fresh vs upgraded" verdict and fail open on the next read
/// (review #455).
fn save_disabled_bundles_file(file: &DisabledBundlesFile) {
    if let Err(error) = try_save_disabled_bundles_file(file) {
        eprintln!("[scope] {error}");
    }
}

/// Reports persist failures as Err (review #455 R7-M2): semantically
/// sensitive callers such as freeze need to distinguish "written" from
/// "silently lost".
fn try_save_disabled_bundles_file(file: &DisabledBundlesFile) -> Result<(), String> {
    let path = disabled_bundles_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!("create parent dir for disabled_bundles.json failed: {error}")
        })?;
    }
    let json = serde_json::to_string(file)
        .map_err(|error| format!("serialize disabled_bundles.json failed: {error}"))?;
    deepseek_tui::utils::write_atomic(&path, json.as_bytes())
        .map_err(|error| format!("write disabled_bundles.json failed: {error}"))?;
    // Successful persist = the file is valid JSON again: both "write-failed"
    // memos are now stale (R9-M1).
    let home = paths::pinvou3_home();
    for memo in [&UNPERSISTED_VERDICT, &PENDING_CORRUPT_RECOVERY] {
        let mut slot = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((memo_home, _)) = slot.as_ref() {
            if *memo_home == home {
                *slot = None;
            }
        }
    }
    Ok(())
}

/// 读某 scope 被禁用的**包 id** 列表（读不到/空 → 空）。
///
/// Initialized scopes follow the persisted list; uninitialized scopes fall
/// back to DenyAll (all installed package ids ∪ all built-in CLI package ids)
/// — "default fully off, external capabilities enabled explicitly". All modes
/// are DenyAll; existing plain installs are initialized by the read-time
/// migration in `load_disabled_bundles_file_locked` (locking in the
/// pre-upgrade switch state) and never take this fallback. Including
/// unconnected CLI packs is harmless (companion skills are not on disk, so
/// excluding them is a no-op), and "connected later" is also off by default.
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
            // installed.json exists but cannot be read (permissions/lock in
            // use): the "installed set" is unknown — must fail closed by
            // disabling all installable packs; treating it as an empty set
            // would strip every installed pack from an uninitialized scope's
            // effective disabled set (plain sessions pass with zero consent,
            // a fail-open flip), and an enable at that moment would
            // materialize the shrunken expansion into a permanent opt-in
            // (review #455 R5-B2). Over-disabling uninstalled packs is
            // harmless: packs installed later are still off by default,
            // consistent with this branch's semantics.
            let manager = MarketplaceManager::new();
            let mut ids: Vec<String> = match manager.try_installed_ids() {
                Ok(ids) => ids,
                Err(error) => {
                    eprintln!(
                        "[scope] {error}; DenyAll expansion falls back to the full available catalog (fail-closed)"
                    );
                    let mut catalog: Vec<String> = manager
                        .available_tools()
                        .into_iter()
                        .map(|manifest| manifest.id)
                        .collect();
                    for info in SkillMarketplaceManager::new().list_skills() {
                        let pkg = skill_owner_package(&info.id);
                        if !catalog.iter().any(|id| id == &pkg) {
                            catalog.push(pkg);
                        }
                    }
                    catalog
                }
            };
            ids.extend(builtin_cli_bundle_ids().map(str::to_string));
            for skill_id in SkillMarketplaceManager::new().installed_skill_ids() {
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
pub fn save_disabled_bundles_for(scope: ConnectorScope, ids: &[String]) {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
    let mut file = load_disabled_bundles_file_locked();
    let key = scope.as_str().to_string();
    let previous = file.scopes.get(&key).cloned().unwrap_or_default();
    file.scopes.insert(key.clone(), normalized.clone());
    file.initialized.insert(key.clone());
    // The composer sends the whole list, not a per-id gesture, so this write
    // says nothing about who switched a given entry off. Keep the
    // install-default marker for entries that were already persisted as off
    // and stay off (`previous ∩ new`): the user did not transition them in
    // this write, and dropping the marker would turn a pack they never touched
    // into an explicit opt-out that the welcome/scene opt-in has to refuse —
    // the round-11 B2 contradiction, re-opened by any unrelated composer
    // toggle (round-12 self-review). Entries entering the list here are the
    // user's own verdict and carry no marker; entries leaving it are on again,
    // so their marker goes with them.
    let retained: Vec<String> = file
        .default_off_scopes
        .get(&key)
        .map(|markers| {
            markers
                .iter()
                .filter(|id| previous.contains(id) && normalized.contains(id))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if retained.is_empty() {
        file.default_off_scopes.remove(&key);
    } else {
        file.default_off_scopes.insert(key.clone(), retained);
    }
    save_disabled_bundles_file(&file);
}

/// 读某 scope 被「不可见」（可见性过滤）的包 id 列表。缺省空 = 全可见。
/// 与 `load_disabled_bundles_for`（开关）正交：可见性只决定是否出现在 composer 列表，
/// 不决定 on/off。
pub fn load_hidden_bundles_for(scope: ConnectorScope) -> Vec<String> {
    let file = load_disabled_bundles_file();
    normalize_stored_pkg_ids(
        &file
            .hidden_scopes
            .get(scope.as_str())
            .cloned()
            .unwrap_or_default(),
    )
}

/// 写某 scope 被「不可见」的包 id 列表（不参与 DenyAll 默认，显式写入才隐藏）。
pub fn save_hidden_bundles_for(scope: ConnectorScope, ids: &[String]) {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
    let mut file = load_disabled_bundles_file_locked();
    file.hidden_scopes
        .insert(scope.as_str().to_string(), normalized);
    save_disabled_bundles_file(&file);
}

/// 该 scope 对底座「不可用」的包 id 并集 = 开关关（disabled）+ 不可见（hidden）。
/// 物化/工具白名单按此并集排除，两套门控对模型都是「调不到」。
pub fn unavailable_bundles_for(scope: ConnectorScope) -> Vec<String> {
    let mut ids = load_disabled_bundles_for(scope);
    for id in load_hidden_bundles_for(scope) {
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
pub fn save_disabled_bundles(ids: &[String]) {
    save_disabled_bundles_for(ConnectorScope::Plain, ids);
}

/// After a pack is installed/connected, sync every initialized scope: when
/// the user has touched the switches, newly installed packs stay off by
/// default (added to that scope's disabled set); uninitialized scopes need
/// nothing (load falls back to "all installed packs disabled by default").
/// All modes are DenyAll (plain joins the sync once initialized by the
/// read-time migration). Connector and skill installs share this entry: the
/// input may be a connector id / skill id / package id, uniformly normalized
/// to a package id.
pub fn sync_deny_all_scopes_after_install(raw_id: &str) {
    let package_id = to_package_id(raw_id);
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_bundles_file_locked();
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
            // Mark the entry as install-written (round-11 B2): the off is a
            // default, not a user verdict — later user-initiated enables may
            // remove it without tripping the explicit-opt-out refusal.
            let defaults = file.default_off_scopes.entry(key.to_string()).or_default();
            if !defaults.iter().any(|id| id == &package_id) {
                defaults.push(package_id.clone());
            }
            changed = true;
        }
    }
    if changed {
        save_disabled_bundles_file(&file);
    }
}

/// 包卸载/断开后同步所有 scope：从各 scope 禁用集与可见性集移除该包 id，避免残留
/// 指向不存在的包。连接器与技能卸载共用本入口：入参可为连接器 id / 技能 id / 包 id，
/// 统一归一为包 id。
pub fn remove_bundle_from_disabled_scopes(raw_id: &str) {
    let package_id = to_package_id(raw_id);
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_bundles_file_locked();
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
    // The marker must go with the entry (round-12 self-review): a stale
    // install-default marker would later let a welcome/scene opt-in lift a
    // *user* off that re-added the same id (uninstall or logout clears the
    // stored entry, then the user switches the connector off again).
    for defaults in file.default_off_scopes.values_mut() {
        let before = defaults.len();
        defaults.retain(|id| id != &package_id);
        changed |= defaults.len() != before;
    }
    if changed {
        save_disabled_bundles_file(&file);
    }
}

/// Batch-enable entry for user actions such as scenario opt-ins (review #455
/// R7-M3): **single-critical-section** RMW — read the current effective
/// disabled set → batch-remove ids → one persist. The frontend's whole-table
/// read-modify-write across IPC is not covered by this lock, and a concurrent
/// composer toggle's write would be overwritten by the stale snapshot (packs
/// the user explicitly disabled get revived, fail-open). When the scope is
/// uninitialized, materialize the opt-in as (on-the-fly expansion − ids);
/// when initialized, remove from the persisted list; the hidden set is
/// cleaned in sync (a hidden pack sees no tools even with the switch on).
pub fn enable_packages_in_scope(scope: ConnectorScope, raw_ids: &[String]) -> Vec<String> {
    let ids: Vec<String> = raw_ids.iter().map(|id| to_package_id(id)).collect();
    if ids.is_empty() {
        return Vec::new();
    }
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_bundles_file_locked();
    let key = scope.as_str();
    if file.initialized.contains(key) {
        // Initialized scope: refuse only ids the **user** explicitly turned
        // off — stored entries not attributable to the install default
        // (round-11 B2 fixes the round-10 Major 2 contradiction: install-sync
        // writes stored+default_off, so a just-installed pack stays enableable
        // and the welcome/scene opt-in works for the upgraded cohort).
        let stored = file.scopes.get(key).cloned().unwrap_or_default();
        let defaults = file
            .default_off_scopes
            .get(key)
            .cloned()
            .unwrap_or_default();
        let blocked: Vec<String> = ids
            .iter()
            .filter(|id| stored.contains(id) && !defaults.contains(id))
            .cloned()
            .collect();
        if !blocked.is_empty() {
            return blocked;
        }
    }
    let mut changed = false;
    if file.initialized.contains(key) {
        if let Some(list) = file.scopes.get_mut(key) {
            let before = list.len();
            list.retain(|id| !ids.contains(id));
            changed |= list.len() != before;
        }
        // An enable clears the install-default marker too: the pack is now on
        // by the user's own gesture; a later disable is that user's verdict.
        if let Some(defaults) = file.default_off_scopes.get_mut(key) {
            let before = defaults.len();
            defaults.retain(|id| !ids.contains(id));
            changed |= defaults.len() != before;
        }
    } else if scope.pack_default_policy() == PackDefaultPolicy::DenyAll {
        let mut effective = resolve_scope_disabled_ids(&file, scope);
        let before = effective.len();
        effective.retain(|id| !ids.contains(id));
        if effective.len() != before {
            // Materialized snapshot (expansion − enabled ids): every entry is
            // off-by-default, not user-verdict — later enables of other packs
            // from the snapshot must not trip the explicit refusal (B2).
            file.default_off_scopes
                .insert(key.to_string(), effective.clone());
            file.scopes.insert(key.to_string(), effective);
            file.initialized.insert(key.to_string());
            changed = true;
        } else {
            // No requested id sits in the expansion: an explicit user action
            // would be silently voided — log it (round-11 m5; the id is
            // likely not installed/known yet, so there is nothing to persist).
            eprintln!(
                "[scope] enable_packages_in_scope({key}): none of {ids:?} matched the DenyAll expansion; no opt-in materialized"
            );
        }
    }
    if let Some(hidden) = file.hidden_scopes.get_mut(key) {
        let before = hidden.len();
        hidden.retain(|id| !ids.contains(id));
        changed |= hidden.len() != before;
    }
    if changed {
        save_disabled_bundles_file(&file);
    }
    Vec::new()
}

/// Consent gate for trash restores (review #455 R5-m5 / R9-M2): a **single
/// critical section** performs "clear hidden leftovers + add the package id
/// and its in-pack skills back into initialized DenyAll scopes' disabled
/// sets". A version spanning N independent lock acquisitions can be
/// interleaved by a concurrent enable between locks (lost-update); same
/// lock-holding pattern as enable_packages_in_scope / the disable arm.
/// Semantics unchanged: initialized scopes restore as disabled
/// (conservative convergence), uninitialized scopes are not written (the
/// DenyAll on-the-fly expansion already covers them), and the hidden set is
/// only cleared, never written (restored packs must stay visible to the user).
pub fn apply_restore_consent_gate(raw_ids: &[String]) -> Result<(), String> {
    let ids: Vec<String> = raw_ids.iter().map(|id| to_package_id(id)).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_bundles_file_locked();
    let mut changed = false;
    for id in &ids {
        // Hidden leftover cleanup: hidden entries left after an uninstall
        // would wrongly hide restored packs.
        for hidden in file.hidden_scopes.values_mut() {
            let before = hidden.len();
            hidden.retain(|x| x != id);
            changed |= hidden.len() != before;
        }
        // Initialized DenyAll scopes: add the id back into the disabled set
        // (consent gate) and mark it install-default (round-11 B2): the
        // restore click is not a verdict against future opt-ins — the
        // welcome/scene enable may still lift it, same as a fresh install's
        // default-off.
        for mode in SessionMode::ALL {
            if mode.pack_default_policy() != PackDefaultPolicy::DenyAll {
                continue;
            }
            let key = mode.as_str();
            if !file.initialized.contains(key) {
                continue;
            }
            let list = file.scopes.entry(key.to_string()).or_default();
            if !list.iter().any(|x| x == id) {
                list.push(id.clone());
                changed = true;
            }
            let defaults = file.default_off_scopes.entry(key.to_string()).or_default();
            if !defaults.iter().any(|x| x == id) {
                defaults.push(id.clone());
                changed = true;
            }
        }
    }
    if changed {
        // Fire-and-forget here would leave a restored pack live with zero
        // consent after a failed save (round-10 m4): propagate so the caller
        // can fail the restore — restore is idempotent, the user retries.
        try_save_disabled_bundles_file(&file)?;
    }
    Ok(())
}

/// 项目级 skills 开关（默认关）。
pub fn project_skills_enabled() -> bool {
    load_disabled_bundles_file().project_skills_enabled
}

/// 写项目级 skills 开关。落盘后由调用方重写在线会话组合目录。
pub fn set_project_skills_enabled(enabled: bool) {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_bundles_file_locked();
    if file.project_skills_enabled == enabled {
        return;
    }
    file.project_skills_enabled = enabled;
    save_disabled_bundles_file(&file);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把 PINVOU3_HOME 指到干净临时目录跑闭包，借 ENV_LOCK 与其它 mutate 测试串行。
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("pinvou3-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        f();
        match prev {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundles_roundtrip_per_scope() {
        with_temp_home(|| {
            // After all modes went DenyAll, an uninitialized scope on a fresh
            // home defaults fully off (including built-in CLI packs); this
            // test focuses on the per-scope read/write roundtrip, so
            // explicitly initialize plain as an empty set first.
            save_disabled_bundles_for(ConnectorScope::Plain, &[]);
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]);
            save_disabled_bundles_for(ConnectorScope::Code, &["feishu".to_string()]);
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

    /// 开关（disabled）与可见性（hidden）两套集合正交，互不污染。
    #[test]
    fn hidden_bundles_are_orthogonal_to_disabled() {
        with_temp_home(|| {
            assert!(load_hidden_bundles_for(ConnectorScope::Plain).is_empty());
            // Explicitly initialize plain as an empty set (after the DenyAll
            // convergence, a fresh home's uninitialized default is fully off;
            // the hidden-orthogonality assertions need an empty disabled
            // baseline).
            save_disabled_bundles_for(ConnectorScope::Plain, &[]);
            save_hidden_bundles_for(ConnectorScope::Plain, &["combo-demo".to_string()]);
            // hidden 不影响 disabled
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            assert_eq!(
                load_hidden_bundles_for(ConnectorScope::Plain),
                vec!["combo-demo".to_string()]
            );
            // 并集：不可用集包含 hidden
            assert!(
                unavailable_bundles_for(ConnectorScope::Plain).contains(&"combo-demo".to_string())
            );
        });
    }

    /// Unavailable = disabled + hidden, deduped; visibility writes must not
    /// pollute the disabled set.
    #[test]
    fn unavailable_is_union_deduped() {
        with_temp_home(|| {
            // Hidden starts empty; disabled does not — after the DenyAll
            // convergence an uninitialized plain scope falls back to the
            // on-the-fly expansion, so pin an explicitly initialized empty
            // baseline first (same shape as
            // `hidden_bundles_are_orthogonal_to_disabled`).
            save_disabled_bundles_for(ConnectorScope::Plain, &[]);
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            assert!(load_hidden_bundles_for(ConnectorScope::Plain).is_empty());

            // Disable weather, hide weather + pptx (weather appears in both sets).
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]);
            save_hidden_bundles_for(
                ConnectorScope::Plain,
                &["weather".to_string(), "pptx".to_string()],
            );

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

    /// Round-11 B2 schema: `default_off_scopes` is backward compatible (an old
    /// file without the field parses with empty defaults, and legacy stored
    /// entries count as user-explicit → refused by the batch enable), the
    /// install-sync marker roundtrips on disk, and a composer whole-list write
    /// only re-attributes the entries it actually transitioned (round-12
    /// self-review: clearing the whole scope made any unrelated toggle turn an
    /// untouched install-default pack into an explicit opt-out).
    #[test]
    fn default_off_scopes_schema_backward_compat_and_roundtrip() {
        with_temp_home(|| {
            // Old-format file: no default_off_scopes key at all.
            let path = disabled_bundles_path();
            std::fs::write(
                &path,
                r#"{"scopes":{"plain":["weather"]},"initialized":["plain"],"plain_defaults_migrated":true}"#,
            )
            .unwrap();
            let file = load_disabled_bundles_file();
            assert!(
                file.default_off_scopes.is_empty(),
                "missing field must default to empty: {file:?}"
            );
            // Legacy stored entries are pre-upgrade switch state = explicit:
            // the batch enable refuses them (round-10 Major 2 preserved).
            let blocked = enable_packages_in_scope(ConnectorScope::Plain, &["weather".to_string()]);
            assert_eq!(blocked, vec!["weather".to_string()]);

            // Install-sync writes stored + default marker; the field
            // roundtrips through the on-disk file.
            sync_deny_all_scopes_after_install("pptx");
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(
                content.contains("default_off_scopes"),
                "the marker set must be persisted: {content}"
            );
            let file = load_disabled_bundles_file();
            assert!(
                file.default_off_scopes
                    .get("plain")
                    .map(|d| d.iter().any(|id| id == "pptx"))
                    .unwrap_or(false),
                "install-default marker survives a reload: {file:?}"
            );
            // An install-default off lifts freely.
            let blocked = enable_packages_in_scope(ConnectorScope::Plain, &["pptx".to_string()]);
            assert!(blocked.is_empty(), "default-off lifts freely: {blocked:?}");

            // A composer whole-list write does not re-attribute the entries it
            // never transitioned: re-arm the install default for `pptx` (the
            // enable above cleared it), then write a list that keeps `pptx` and
            // adds `weather`, which the user turns off in that very write.
            sync_deny_all_scopes_after_install("pptx");
            save_disabled_bundles_for(
                ConnectorScope::Plain,
                &["pptx".to_string(), "weather".to_string()],
            );
            let file = load_disabled_bundles_file();
            assert!(
                file.default_off_scopes
                    .get("plain")
                    .map(|d| d.iter().any(|id| id == "pptx"))
                    .unwrap_or(false),
                "an untouched install-default entry keeps its marker: {file:?}"
            );
            let blocked = enable_packages_in_scope(ConnectorScope::Plain, &["pptx".to_string()]);
            assert!(
                blocked.is_empty(),
                "an untouched default-off still lifts after a composer write: {blocked:?}"
            );

            // The entry this write itself switched off (`weather` enters the
            // persisted list here) is the user's own verdict: no marker, and the
            // batch enable refuses it.
            assert!(
                file.default_off_scopes
                    .get("plain")
                    .map(|d| !d.iter().any(|id| id == "weather"))
                    .unwrap_or(true),
                "an entry the user switched off carries no marker: {file:?}"
            );
            let blocked = enable_packages_in_scope(ConnectorScope::Plain, &["weather".to_string()]);
            assert_eq!(blocked, vec!["weather".to_string()]);
        });
    }

    /// A stale install-default marker must not survive the removal of its
    /// stored entry (round-12 self-review): uninstall/logout clears the stored
    /// entry, the user switches the connector off again, and a marker left
    /// behind would let the next welcome/scene opt-in lift that user verdict.
    #[test]
    fn remove_bundle_clears_the_install_default_marker() {
        with_temp_home(|| {
            let path = disabled_bundles_path();
            std::fs::write(
                &path,
                r#"{"scopes":{"plain":["pptx"]},"default_off_scopes":{"plain":["pptx"]},"initialized":["plain"],"plain_defaults_migrated":true}"#,
            )
            .unwrap();
            remove_bundle_from_disabled_scopes("pptx");
            let file = load_disabled_bundles_file();
            assert!(
                file.default_off_scopes
                    .get("plain")
                    .map(|d| d.is_empty())
                    .unwrap_or(true),
                "the marker goes with the stored entry: {file:?}"
            );
        });
    }

    /// The user's own switch-off is explicit, so it must drop an
    /// install-default marker left by an earlier install (round-12
    /// self-review); the marker is only ever written by install-sync, the
    /// welcome/scene opt-in, and the restore gate. Driven through the live
    /// connector switch (the composer's whole-list write,
    /// `save_disabled_bundles_for`).
    #[test]
    fn connector_switch_off_clears_the_install_default_marker() {
        with_temp_home(|| {
            let path = disabled_bundles_path();
            std::fs::write(
                &path,
                r#"{"scopes":{"plain":["pptx"]},"default_off_scopes":{"plain":["pptx"]},"initialized":["plain"],"plain_defaults_migrated":true}"#,
            )
            .unwrap();
            // Taking pptx back on removes the stored entry, and the
            // install-default marker goes with it.
            save_disabled_bundles_for(ConnectorScope::Plain, &[]);
            let file = load_disabled_bundles_file();
            assert!(
                file.default_off_scopes
                    .get("plain")
                    .map(|d| d.is_empty())
                    .unwrap_or(true),
                "enabling clears the marker: {file:?}"
            );
            // Switching it off again enters it as the user's own verdict; no
            // marker may be re-armed for an entry this write transitioned.
            save_disabled_bundles_for(ConnectorScope::Plain, &["pptx".to_string()]);
            let file = load_disabled_bundles_file();
            assert!(
                file.default_off_scopes
                    .get("plain")
                    .map(|d| d.is_empty())
                    .unwrap_or(true),
                "the user's own switch-off must not re-arm a marker: {file:?}"
            );
            assert_eq!(
                enable_packages_in_scope(ConnectorScope::Plain, &["pptx".to_string()]),
                vec!["pptx".to_string()],
                "the user's explicit off is refused by the batch enable"
            );
        });
    }

    /// 卸载/断开后清理残留：同时清 disabled 与 hidden 两套集合。
    #[test]
    fn remove_bundle_clears_both_sets() {
        with_temp_home(|| {
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]);
            save_hidden_bundles_for(ConnectorScope::Plain, &["weather".to_string()]);
            remove_bundle_from_disabled_scopes("weather");
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            assert!(load_hidden_bundles_for(ConnectorScope::Plain).is_empty());
        });
    }

    /// 保存路径统一归一为包 id：剥 `skill:` 前缀 + companion 映射到所属包。
    #[test]
    fn save_normalizes_to_package_id() {
        with_temp_home(|| {
            // gongwen 未装：companion 技能保留独立纯技能包形态 → 归一为自身 id
            // （与 list_bundles 的 V5 认领展示一致，开关不回弹）。
            save_disabled_bundles_for(
                ConnectorScope::Plain,
                &["skill:government-writing".to_string()],
            );
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
            );
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
        with_temp_home(|| {
            save_disabled_bundles_for(ConnectorScope::Plain, &["government-writing".to_string()]);
            save_hidden_bundles_for(ConnectorScope::Plain, &["government-writing".to_string()]);
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
            // Byte-level on-disk assertion (review #455 R6-m2): normalization
            // must not only fix the gating criteria but also persist —
            // otherwise raw entries survive uninstalling the claim owner and
            // revive on reinstall.
            let persisted = std::fs::read_to_string(disabled_bundles_path()).unwrap();
            assert!(
                persisted.contains("\"gongwen\"") && !persisted.contains("government-writing"),
                "归一化结果应落盘替换原始条目: {persisted}"
            );
        });
    }

    /// 项目级 skills 开关往返。
    #[test]
    fn project_skills_roundtrip() {
        with_temp_home(|| {
            assert!(!project_skills_enabled(), "项目技能默认关");
            set_project_skills_enabled(true);
            assert!(project_skills_enabled());
            set_project_skills_enabled(false);
            assert!(!project_skills_enabled());
        });
    }

    /// 读路径的「读到即迁移落盘」必须取 `DISABLED_BUNDLES_FILE_LOCK` 与持锁写方
    /// 串行：持锁期间并发 load（磁盘为旧连接器文件、必然触发迁移落盘）不得先行落盘。
    #[test]
    fn read_path_migration_serializes_with_file_lock() {
        with_temp_home(|| {
            let legacy = r#"["weather"]"#;
            let conn = paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(&conn, legacy).unwrap();
            let guard = DISABLED_BUNDLES_FILE_LOCK
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let reader = std::thread::spawn(load_disabled_bundles_for_plain_for_lock_test);
            std::thread::sleep(std::time::Duration::from_millis(200));
            // 持锁期间 bundles 文件尚未写入（迁移被串行化）。
            assert!(
                !disabled_bundles_path().exists(),
                "持锁期间读路径不得先行迁移落盘"
            );
            drop(guard);
            assert_eq!(reader.join().unwrap(), vec!["weather".to_string()]);
            let content = std::fs::read_to_string(disabled_bundles_path()).unwrap();
            assert!(
                content.contains("\"scopes\""),
                "释放锁后迁移完成: {content}"
            );
        });
    }

    fn load_disabled_bundles_for_plain_for_lock_test() -> Vec<String> {
        load_disabled_bundles_for(ConnectorScope::Plain)
    }
}

/// Quarantines the raw bytes of a corrupt disabled_bundles.json into a
/// `.corrupt.<ts>` copy (sharing `quarantine_corrupt_state_file` with
/// installed.json); the read path then self-heals via fail-closed degrade.
/// Quarantine failure propagates as Err — the caller uses that to give up
/// overwriting the original, avoiding wiping recoverable raw bytes when the
/// quarantine copy never landed (review #455 R5-m4).
fn quarantine_corrupt_disabled_bundles(content: &[u8], error: &str) -> Result<(), String> {
    let path = disabled_bundles_path();
    super::quarantine_corrupt_state_file(&path, content)?;
    eprintln!(
        "[marketplace] disabled_bundles.json was corrupt ({error}); quarantined, attempting the fail-closed reset"
    );
    Ok(())
}
