//! 包 id × SessionMode 的单一禁用集（`~/.pinvou3/disabled_bundles.json`）。
//!
//! 这是「工具市场统一治理」scope 收敛的单一真相源：取代原先
//! `disabled_connectors.json`（连接器 id）与 `disabled_skills.json`（技能 id）两份
//! 文件。开关粒度收敛为**包 id**（= `bundle.rs` 里 `BundleInfo.id`，即 MCP 工具 id /
//! 技能 id / CLI 连接器 id），一个包 = 一个开关，包内技能（companion skills）可见性
//! 唯一跟随所属包（§5.2 不变量）。
//!
//! 落盘格式与 #287 泛化后的两份旧文件同构：`{scopes, hidden_scopes,
//! default_off_scopes: {"<mode>": [...]}, initialized: ["<mode>"],
//! project_skills_enabled, plain_defaults_migrated}`。Scope keys 用 `SessionMode`
//! 的 kebab-case 名。`plain_defaults_migrated`：plain 仍为 AllowAll 时写下的旧
//! 文件标记；读时迁移把 plain 初始化为已持久化列表并落盘标记。首次读入即把两
//! 份旧文件迁移进本文件
//! migrates the two legacy files into this file on read (migrate-on-read):
//! 旧连接器 id 原样进包 id（连接器 id 即包 id）；旧技能 id 经 `to_package_id`
//! （R17-MAJOR1 起为门控口径 `skill_gating_owner`，含物理布局兜底）映射到所属包；
//! `skill:` 前缀残留统一剥除。迁移幂等，失败回退默认值（安全兜底）。
//!
// architecture-guard: allow-target-cfg -- the unix regression tests in this file (the round-13 B3 install-sync persist failure and the round-14 #2 freeze memo) need unreadable (0o555 directory / 0o000 file) fixtures; test-only inline cfg(unix)+PermissionsExt (same exemption precedent as mod.rs / package_export.rs, review #455); a real open() probe guards against running as root, Windows is covered by link checks.
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
/// only, taking no transaction lock and persisting no registry state (the only
/// side effect is the quarantine sidecar copy; see the read-only recovery
/// branch of `try_installed_ids`); the next writer holding the transaction lock
/// persists the registry.
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
/// original is still on disk and every subsequent read re-enters the corrupt
/// branch. The no-sibling quarantine rule already caps `.corrupt.*`
/// accumulation, so the memo's residual value is suppressing the pointless
/// repeated failed-write attempts (quarantine skip + save attempt) on every
/// read while the environment is broken — and it must NOT self-heal by
/// rewriting the file behind a later reader's back once the environment
/// recovers (the next WRITER owns that transition; pinned by
/// recycle_bin's `corrupt_recovery_pins_no_sibling_rule_and_memo`). Reads
/// hitting the memo reuse the in-memory fail-closed state directly; any
/// successful save (the file becomes valid JSON again) clears it, and the
/// process self-heals.
static PENDING_CORRUPT_RECOVERY: Mutex<Option<(PathBuf, DisabledBundlesFile)>> = Mutex::new(None);

/// Round-19 MAJOR 4: set when a read finds the on-disk `disabled_bundles.json`
/// UNREADABLE (chmod-000 file, AV lock — bytes exist but cannot be salvaged).
/// The next persist must not blind-rename over those unrecoverable bytes:
/// `try_save` first renames the original aside (a rename needs only directory
/// write permission, so it always succeeds) and clears this memo. Without it,
/// "unreadable original → any write" permanently destroys the user's explicit
/// opt-outs with no quarantine copy — the R6-B1 violation the read path
/// already refuses for itself.
static UNREADABLE_ORIGINAL: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Clears the verdict memos. Test-only hygiene: the statics are process-global
/// and keyed by home path, so a memo left behind by an aborted earlier case
/// (or by a harness that recreates the same path) must not bleed into the
/// next one; production never clears (the file is the truth; a memo only
/// briefly carries state after a write failure).
#[cfg(test)]
pub(crate) fn clear_unpersisted_verdict_for_test() {
    *UNPERSISTED_VERDICT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *PENDING_CORRUPT_RECOVERY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *UNREADABLE_ORIGINAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Test-only write-failure injection for `try_save_disabled_bundles_file`
/// (round-20 MAJOR B): fires after the rename-aside decision point, so the
/// restore-on-write-failure path is drivable on an otherwise writable home.
/// Same drop-guard convention as the mod.rs/recycle_bin failpoints: an
/// unconsumed injection auto-clears instead of leaking into unrelated tests.
#[cfg(test)]
static FAIL_NEXT_DISABLED_BUNDLES_WRITE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
pub(crate) fn fail_next_disabled_bundles_write_for_test() -> super::FailpointResetGuard {
    super::arm_failpoint(&FAIL_NEXT_DISABLED_BUNDLES_WRITE)
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
            // Round-20 MAJOR B, crash-window evidence: a `.corrupt.*` sibling
            // proves a converged-era store EXISTED and was lost without a
            // successful overwrite (the rename-aside crash window in
            // `try_save_disabled_bundles_file`, or a wiped live file with a
            // surviving quarantine copy). Its stranded verdict is unknowable
            // (the bytes were unreadable even before the rename), so recover
            // fail-closed — set only the migration marker, initialize no
            // scope, freeze; the DenyAll fallback keeps every previously
            // opted-out pack off. Without this arm the wide signal below
            // judges "upgraded" and initializes plain EMPTY: every opt-out
            // back ON. This is deliberately narrower than the registered
            // R10-m3 deletion case (live file deleted, NO sibling): the
            // sibling distinguishes "store existed and was lost mid-recovery"
            // from the never-had-a-store 0.8.6–0.9.2 cohort that the
            // registered case must keep protecting, so the legacy migration
            // is skipped here entirely — legacy files are pre-convergence
            // remnants when the unified store demonstrably existed.
            if corrupt_sidecar_evidence_exists(&home) {
                eprintln!(
                    "[marketplace] disabled_bundles.json is gone but a .corrupt.* copy proves a lost store; recovering fail-closed (all scopes fall back to DenyAll), freeze persisted"
                );
                let recovered = DisabledBundlesFile {
                    plain_defaults_migrated: true,
                    ..DisabledBundlesFile::default()
                };
                if let Err(freeze_error) = try_save_disabled_bundles_file(&recovered) {
                    eprintln!(
                        "[scope] CRITICAL: failed to persist the lost-store recovery verdict: {freeze_error}; holding the in-process verdict until restart"
                    );
                    *UNPERSISTED_VERDICT
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some((home, recovered.clone()));
                }
                return recovered;
            }
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
                    // Round-19 MAJOR 4: mark the home so the next persist
                    // preserves the unreadable bytes (rename-aside) instead of
                    // destroying them.
                    *UNREADABLE_ORIGINAL
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(paths::pinvou3_home());
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

/// 原始条目 → 包 id。连接器/CLI id 原样保留（`skill_gating_owner` 对它们恒等）；
/// `skill:` 前缀剥除后按技能名映射到所属包。门控侧用物理感知的
/// `skill_gating_owner`（R17-MAJOR1）：存储条目可能携带组合包的圈内技能名
/// （restore 门按 bin 侧技能目录扫描写入），未声明的嵌套技能必须归到物理
/// 所属包，否则以零同意进入所有 scope 且无行可关。
fn to_package_id(raw: &str) -> String {
    let stripped = raw.strip_prefix("skill:").unwrap_or(raw);
    crate::features::marketplace::bundle::skill_gating_owner(stripped)
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
    let home = paths::pinvou3_home();
    // Round-19 MAJOR 4: when a prior read found the on-disk file UNREADABLE,
    // this persist must not blind-rename over bytes nobody could read. Rename
    // the original aside first (needs only directory write permission, so it
    // works even where the file is unreadable) — the bytes survive either way;
    // a failed rename refuses the write instead of destroying them.
    //
    // Round-20 MAJOR B: the marker stays armed until the write itself
    // succeeds (it is cleared by the successful-persist tail below). The
    // previous form cleared it right after the rename, so a failed write (or
    // a crash between the two adjacent ops) left NO live file, NO marker, and
    // the bytes only in the sidecar — the next read took the NotFound branch,
    // the wide signal judged "upgraded", and plain initialized empty: every
    // opted-out pack back ON (fail-open) inside the very machinery that was
    // built to prevent the loss. On a write failure the sidecar is renamed
    // back, restoring "unreadable original in place, marker armed" exactly.
    let mut renamed_aside: Option<PathBuf> = None;
    {
        let degraded = UNREADABLE_ORIGINAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|p| p == &home)
            .unwrap_or(false);
        if degraded && path.exists() {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let sidecar = path.with_file_name(format!("disabled_bundles.json.corrupt.{stamp}"));
            std::fs::rename(&path, &sidecar).map_err(|error| {
                format!(
                    "refusing to overwrite unreadable disabled_bundles.json: rename-aside to {} failed: {error}",
                    sidecar.display()
                )
            })?;
            renamed_aside = Some(sidecar.clone());
            eprintln!(
                "[marketplace] unreadable disabled_bundles.json preserved as {} before overwrite",
                sidecar.display()
            );
        }
    }
    let write_result = (|| {
        #[cfg(test)]
        if FAIL_NEXT_DISABLED_BUNDLES_WRITE.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(
                "injected disabled_bundles.json write failure (test failpoint)".to_string(),
            );
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!("create parent dir for disabled_bundles.json failed: {error}")
            })?;
        }
        let json = serde_json::to_string(file)
            .map_err(|error| format!("serialize disabled_bundles.json failed: {error}"))?;
        deepseek_tui::utils::write_atomic(&path, json.as_bytes())
            .map_err(|error| format!("write disabled_bundles.json failed: {error}"))
    })();
    if let Err(error) = write_result {
        if let Some(sidecar) = renamed_aside.as_ref() {
            // Restore the unreadable original: the store must not sit absent
            // with only a sidecar while the marker claims otherwise. If even
            // the rename-back fails, UNREADABLE_ORIGINAL stays armed (never
            // cleared mid-flight) and the NotFound read's `.corrupt.*`
            // sibling evidence keeps the verdict fail-closed (round-20
            // MAJOR B).
            match std::fs::rename(sidecar, &path) {
                Ok(()) => eprintln!(
                    "[marketplace] disabled_bundles.json write failed ({error}); the unreadable original was restored from {}",
                    sidecar.display()
                ),
                Err(restore_error) => eprintln!(
                    "[marketplace] disabled_bundles.json write failed ({error}) and restoring the unreadable original from {} failed too: {restore_error}; the corrupt-copy evidence keeps the next read fail-closed",
                    sidecar.display()
                ),
            }
        }
        return Err(error);
    }
    // Successful persist = the file is valid JSON again: both "write-failed"
    // memos are now stale (R9-M1).
    for memo in [&UNPERSISTED_VERDICT, &PENDING_CORRUPT_RECOVERY] {
        let mut slot = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((memo_home, _)) = slot.as_ref() {
            if *memo_home == home {
                *slot = None;
            }
        }
    }
    // Round-19 MAJOR 4: the unreadable-original marker has no payload tuple —
    // a successful persist means the (renamed-aside) file is the truth again.
    *UNREADABLE_ORIGINAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    Ok(())
}

/// Whether any `disabled_bundles.json.corrupt.*` sibling exists in the home:
/// both quarantine copies (`quarantine_corrupt_state_file`) and the
/// rename-aside preservation (`try_save_disabled_bundles_file`) share the
/// naming pattern, and either proves the converged-era store existed
/// (round-20 MAJOR B).
fn corrupt_sidecar_evidence_exists(home: &std::path::Path) -> bool {
    const EVIDENCE_PREFIX: &str = "disabled_bundles.json.corrupt.";
    std::fs::read_dir(home)
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(EVIDENCE_PREFIX))
            })
        })
        .unwrap_or(false)
}

/// 读某 scope 被禁用的**包 id** 列表（读不到/空 → 空）。
///
/// Initialized scopes follow the persisted list; uninitialized scopes fall
/// back to DenyAll (all installed package ids ∪ all built-in CLI package ids
/// ∪ the owner packages of installed standalone skills)
/// — "default fully off, external capabilities enabled explicitly". All modes
/// are DenyAll; existing plain installs are initialized by the read-time
/// migration in `load_disabled_bundles_file_locked` (locking in the
/// pre-upgrade switch state) and, on a clean read path, never take this
/// fallback (the disclosed R11-M4 exception: an upgraded install whose freeze
/// persist failed re-enters it after restart). Including
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
                    // 两条记录臂刻意用认领口径（`skill_owner_package`）：它们的
                    // 输入是登记表自己认领的技能 id——有认领时与门控口径一致，
                    // 独立登记的技能没有可兜底的物理嵌套布局。只有下面的 disk
                    // leg 走 `skill_gating_owner`：它枚举的正是**无认领**的物理
                    // 目录，必须与物化层的目录扫描同一口径（round-20 minor 3：
                    // 与记录臂的口径差异在此显式标注，非遗漏）。
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
            // 同上（round-20 minor 3）：登记表臂用认领口径，disk leg 用门控口径。
            for skill_id in SkillMarketplaceManager::new().installed_skill_ids() {
                let pkg = skill_owner_package(&skill_id);
                if !ids.iter().any(|id| id == &pkg) {
                    ids.push(pkg);
                }
            }
            // Disk-derived skill arm (round-19 MAJOR 2): the record-driven
            // enumeration above misses packs whose skills live on disk but
            // whose ids never enter the records — MCP-free plugin.json packs
            // (import skips install_upload), under-declared combinations,
            // half-registered states. Materialization is directory-scan
            // based, so derive the same owners from the same walk: every
            // nested skill dir claims its physical owner pack
            // (`skill_gating_owner`), keeping the expansion and
            // materialization on one truth.
            if let Ok(rd) = std::fs::read_dir(paths::bundles_root()) {
                for entry in rd.flatten() {
                    let Ok(skills_dir) = std::fs::read_dir(entry.path().join("skills")) else {
                        continue;
                    };
                    for skill in skills_dir.flatten() {
                        let name = skill.file_name().to_string_lossy().into_owned();
                        if name.is_empty() {
                            continue;
                        }
                        let pkg = crate::features::marketplace::bundle::skill_gating_owner(&name);
                        if !ids.iter().any(|id| id == &pkg) {
                            ids.push(pkg);
                        }
                    }
                }
            }
            ids
        }
    }
}

/// 写某 scope 被禁用的包 id 列表（写入即标记该 scope 已初始化）。入参统一归一为包
/// id（剥 `skill:` 前缀 + companion 映射），防御历史版本误写入的带前缀条目。
/// 写失败原样上抛（round-19 MAJOR 1：main #563 的 fail-loud 契约是既成用户
/// 可见语义，本 PR 不降级——composer 调用方的重试/回滚 UX 由 #515 重work 落实）。
pub fn save_disabled_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
    let mut file = load_disabled_bundles_file_locked();
    let key = scope.as_str().to_string();
    let was_uninitialized = !file.initialized.contains(&key);
    let previous = file.scopes.get(&key).cloned().unwrap_or_default();
    // First composer write on an uninitialized DenyAll scope (round-13 B1):
    // the composer holds the *effective* set — the full on-the-fly DenyAll
    // expansion on a fresh install, every pack rendered off — and sends the
    // whole list. Without seeding, that write would materialize the expansion
    // as stored entries with no `default_off_scopes` markers: every untouched
    // pack would become an "explicit user opt-out" the user never made, and
    // the welcome/scene opt-in would refuse it forever. Seed the markers from
    // the pre-write effective expansion ∩ the new list, the same attribution
    // the enable path's materialization arm uses: entries off-by-default that
    // stay off are defaults (liftable), entries newly written off here are
    // the user's own verdict (no marker). Computed before the mutations below
    // — the expansion depends on the still-uninitialized state. Under an
    // AllowAll policy there is no expansion and nothing to seed.
    let seeded_defaults: Vec<String> =
        if was_uninitialized && scope.pack_default_policy() == PackDefaultPolicy::DenyAll {
            resolve_scope_disabled_ids(&file, scope)
                .into_iter()
                .filter(|id| normalized.contains(id))
                .collect()
        } else {
            Vec::new()
        };
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
    let retained: Vec<String> = if was_uninitialized {
        // Round-13 B1: markers come from the pre-write expansion (above), not
        // from `previous ∩ new` — on a fresh install `previous` is empty, so
        // the persisted-list filter would retain nothing and strand every
        // default as an unattributed opt-out. Over-attribution is safe: a
        // marker only makes a later enable *easier*.
        seeded_defaults
    } else {
        file.default_off_scopes
            .get(&key)
            .map(|markers| {
                markers
                    .iter()
                    .filter(|id| previous.contains(id) && normalized.contains(id))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    };
    if retained.is_empty() {
        file.default_off_scopes.remove(&key);
    } else {
        file.default_off_scopes.insert(key.clone(), retained);
    }
    // The persist is fail-loud (round-19 MAJOR 1): main's #563 made this
    // writer's failure a user-visible command error (the frontend rolls the
    // toggle back and alerts), and with #563 now in this PR's merge base,
    // keeping the old fire-and-forget tail would silently downgrade a shipped
    // contract. The #515 rework still owns the deeper composer concerns — the
    // whole-list replace's cross-process RMW and the stale-snapshot
    // resurrection (round-14 M1) — but a lost write now surfaces instead of
    // rendering success over unpersisted state.
    try_save_disabled_bundles_file(&file)
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
/// 写失败原样上抛（round-19 MAJOR 1，同 save_disabled_bundles_for）。
pub fn save_hidden_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
    let mut file = load_disabled_bundles_file_locked();
    file.hidden_scopes
        .insert(scope.as_str().to_string(), normalized);
    try_save_disabled_bundles_file(&file)
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

/// 写全局（plain）被禁用的包 id 列表。测试专用（生产写一律走
/// [`save_disabled_bundles_for`] 显式给 scope）。启动期 best-effort，
/// 写失败降级为日志（调用方无法处理治理写失败）。
#[cfg(test)]
pub fn save_disabled_bundles(ids: &[String]) {
    // The documented best-effort contract (round-20 minor 5: the Result must
    // be consumed explicitly now that the writer is fail-loud).
    let _ = save_disabled_bundles_for(ConnectorScope::Plain, ids);
}

/// After a pack is installed/connected, sync every initialized scope: when
/// the user has touched the switches, newly installed packs stay off by
/// default (added to that scope's disabled set); uninitialized scopes need
/// nothing (load falls back to "all installed packs disabled by default").
/// All modes are DenyAll (plain joins the sync once initialized by the
/// read-time migration). Connector and skill installs share this entry: the
/// input may be a connector id / skill id / package id, uniformly normalized
/// to a package id.
///
/// The persist is **fail-visible** (round-13 B3): `installed.json` has
/// already committed the pack when this runs, and for the migrated cohort
/// every scope here is initialized — the stored list is the authoritative
/// consent store, so a swallowed save would leave the pack ON in every new
/// session with zero consent while the install reported success (fail-open,
/// the worse direction of the enable path's round-12 invariant "applied and
/// persisted"). Callers surface the error after their own success commit; a
/// retry of the sync is safe (idempotent membership push).
pub fn sync_deny_all_scopes_after_install(raw_id: &str) -> Result<(), String> {
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
        try_save_disabled_bundles_file(&file)?;
    }
    Ok(())
}

/// Sync every scope after a bundle uninstall/disconnect: drop the id from each
/// scope's disabled and visibility sets so no stale entry keeps pointing at a
/// missing package. Shared entry point for connector, skill, and package
/// teardown: the argument may be a connector id / skill id / package id and is
/// normalized to the package id.
///
/// Fail-visible (round-17 minor 1): a lost removal save leaves the stale
/// stored entry + install-default marker behind, and a same-id reinstall
/// inherits them through the install sync's initialized-arm no-op — the
/// mirror image of the install-sync fail-visible direction (round-13 B3), so
/// the persist propagates instead of degrading to a log line.
pub fn remove_bundle_from_disabled_scopes(raw_id: &str) -> Result<(), String> {
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
        try_save_disabled_bundles_file(&file)?;
    }
    Ok(())
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
/// The persist is **fail-visible** (round-12 review): the caller's contract is
/// "applied and persisted", and the hot refresh re-reads the file from disk,
/// so a swallowed save would report success while the model never sees the
/// tool — not even in the current session. Same invariant as
/// `apply_restore_consent_gate`; on `Err` nothing was applied.
///
/// Round-13 m3: ids absent from the DenyAll expansion (an install committing
/// right after the snapshot, or an unknown id) get nothing applied — they are
/// reported in `not_applied` instead of only logged, so the caller does not
/// present `enabled: true` while the requested opt-in never materialized.
/// Fail-closed direction: the pack stays off.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EnablePackagesOutcome {
    /// Ids refused as the user's explicit opt-out (initialized scope, stored
    /// entry without an install-default marker). Nothing was enabled for the
    /// whole batch when this is non-empty.
    pub blocked: Vec<String>,
    /// Requested ids that matched no entry (absent from the DenyAll
    /// expansion): nothing was applied for them — likely a concurrent install
    /// that had not committed yet, or an unknown id.
    ///
    /// Report-honesty caveat (round-16 m2, restated round-20 minor 2): only
    /// the uninitialized (expansion) arm can detect these — an **initialized**
    /// scope has no equivalent signal, so an unknown id there is treated as
    /// already-on and stays unreported (`not_applied` empty, `blocked`
    /// empty): fail-closed in effect, but callers must not read an empty
    /// `not_applied` as "coverage proven" in that state.
    pub not_applied: Vec<String>,
}

pub fn enable_packages_in_scope(
    scope: ConnectorScope,
    raw_ids: &[String],
) -> Result<EnablePackagesOutcome, String> {
    let mut ids: Vec<String> = raw_ids.iter().map(|id| to_package_id(id)).collect();
    // Sort-then-dedup (round-20 minor 1): bare `dedup` removes consecutive
    // duplicates only, so e.g. ["a","b","a"] leaked a duplicate into the
    // blocked/not_applied reporting; sorting also makes the reported order
    // deterministic.
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Ok(EnablePackagesOutcome::default());
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
        // Round-16 minor 2 caveat: ids absent from the stored list are treated
        // as already-on (`not_applied` stays empty here) — the
        // install-commits-after-snapshot race that m3 reports for the
        // expansion arm has no equivalent signal in this arm.
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
            return Ok(EnablePackagesOutcome {
                blocked,
                not_applied: Vec::new(),
            });
        }
    }
    let mut not_applied: Vec<String> = Vec::new();
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
        let not_applied_ids: Vec<String> = ids
            .iter()
            .filter(|id| !effective.contains(id))
            .cloned()
            .collect();
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
        not_applied = not_applied_ids;
    }
    if let Some(hidden) = file.hidden_scopes.get_mut(key) {
        let before = hidden.len();
        hidden.retain(|id| !ids.contains(id));
        changed |= hidden.len() != before;
    }
    if changed {
        // Fail-visible (round-12 review): the caller must not report
        // "enabled" when the state did not reach disk — the hot refresh reads
        // the file back, so a swallowed failure leaves the tool invisible to
        // the model while the UI claims the opt-in happened.
        try_save_disabled_bundles_file(&file)?;
    }
    Ok(EnablePackagesOutcome {
        blocked: Vec::new(),
        not_applied,
    })
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
    apply_restore_consent_gate_impl(raw_ids, false)
}

/// Force variant for the supply-skipped restore cohort (review #455 R16-MAJOR1):
/// a secrets-declaring pack skips `install_upload`, so its id never re-enters
/// `installed.json`; a combination pack has no `skills/<pack-id>/` directory, so
/// `find_skill_dir` hides it from `list_skills`. Its id is therefore in NONE of
/// the three DenyAll expansion inputs, while directory-scan session
/// materialization still sees the on-disk skills — for uninitialized scopes the
/// gate's "the expansion covers the pack" premise is false and the pack (and its
/// scripts, via the same disabled set) would go live with zero consent. For
/// those scopes this variant materializes `expansion ∪ ids` as the stored list
/// with install-default markers and initializes the scope: the pack is
/// explicitly off, untouched defaults stay liftable, and the freeze trade-off
/// (later added builtins default on in this scope) is accepted exactly as for
/// the enable path's materialization arm.
pub fn apply_restore_consent_gate_secrets_pack(raw_ids: &[String]) -> Result<(), String> {
    apply_restore_consent_gate_impl(raw_ids, true)
}

fn apply_restore_consent_gate_impl(
    raw_ids: &[String],
    force_uninitialized: bool,
) -> Result<(), String> {
    let ids: Vec<String> = raw_ids.iter().map(|id| to_package_id(id)).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_bundles_file_locked();
    let mut changed = false;
    // Round-16 MAJOR1, force pass: materialize uninitialized DenyAll scopes
    // once, before the per-id loop — `expansion ∪ ids` with install-default
    // markers, then initialize the scope. Per-id re-add below then finds every
    // id already stored and only asserts the markers.
    if force_uninitialized {
        for mode in SessionMode::ALL {
            if mode.pack_default_policy() != PackDefaultPolicy::DenyAll {
                continue;
            }
            let key = mode.as_str();
            if file.initialized.contains(key) {
                continue;
            }
            let mut stored = resolve_scope_disabled_ids(&file, *mode);
            for id in &ids {
                if !stored.iter().any(|x| x == id) {
                    stored.push(id.clone());
                }
            }
            file.scopes.insert(key.to_string(), stored.clone());
            file.default_off_scopes.insert(key.to_string(), stored);
            file.initialized.insert(key.to_string());
            changed = true;
        }
    }
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
        // default-off. Round-16 minor 1: the marker push lives inside the
        // new-entry guard — re-arming a marker on a stored entry without one
        // would re-attribute a surviving user verdict as install-default.
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
                let defaults = file.default_off_scopes.entry(key.to_string()).or_default();
                if !defaults.iter().any(|x| x == id) {
                    defaults.push(id.clone());
                }
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
    use crate::platform::test_support::with_temp_home;

    /// Round-19 MAJOR 2 regression: an MCP-free skills-only plugin pack whose
    /// skill ids differ from the pack id is invisible to the record-driven
    /// enumeration (import skips install_upload; list_skills' Upload leg skips
    /// records whose skills/<pack-id>/ dir is absent) — yet directory-scan
    /// materialization sees its skills. The DenyAll expansion must therefore
    /// derive those owners from the same disk walk, so an uninitialized scope
    /// gates the pack without any opt-in.
    #[test]
    fn mcp_free_plugin_pack_gates_via_disk_derived_expansion() {
        with_temp_home("pinvou3-scope", || {
            let pkg = paths::bundles_root().join("skills-only-pack");
            for name in ["skill-a", "skill-b"] {
                std::fs::create_dir_all(pkg.join("skills").join(name)).unwrap();
                std::fs::write(
                    pkg.join("skills").join(name).join("SKILL.md"),
                    format!("---\nname: {name}\n---\n# {name}\n"),
                )
                .unwrap();
            }
            // No records anywhere: the pack exists only as a directory tree.
            let unavailable = load_disabled_bundles_for(ConnectorScope::Plain);
            assert!(
                unavailable.iter().any(|id| id == "skills-only-pack"),
                "the disk-derived expansion must gate the pack id: {unavailable:?}"
            );
        });
    }

    /// Round-19 MAJOR 4: a persist over an UNREADABLE on-disk store must
    /// preserve the original bytes (rename-aside) instead of blind-renaming
    /// over them — "unreadable but recoverable" must never become
    /// "permanently lost" (R6-B1). The write itself still succeeds on a
    /// writable home, and the store loads cleanly afterwards.
    #[cfg(unix)]
    #[test]
    fn save_over_unreadable_original_preserves_bytes() {
        use std::os::unix::fs::PermissionsExt;
        with_temp_home("pinvou3-scope", || {
            let path = disabled_bundles_path();
            let original = b"unrecoverable-user-optouts{{{".to_vec();
            std::fs::write(&path, &original).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
            // Root probe (mode bits are no-ops for root): if the file is still
            // readable, the unreadable-read branch never runs — skip loudly.
            if std::fs::read(&path).is_ok() {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
                eprintln!(
                    "ROOT-SKIP[save_over_unreadable_original_preserves_bytes]: running as root - the unreadable-file fixture stays readable; NOT exercised"
                );
                return;
            }

            let result = save_disabled_bundles_for(ConnectorScope::Plain, &[]);
            assert!(
                result.is_ok(),
                "a writable home lets the consent save succeed: {result:?}"
            );

            // The unreadable original survived the overwrite, renamed aside.
            let parent = path.parent().unwrap();
            let sidecars: Vec<std::path::PathBuf> = std::fs::read_dir(parent)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.contains(".corrupt."))
                        .unwrap_or(false)
                })
                .collect();
            assert_eq!(
                sidecars.len(),
                1,
                "exactly one preserved copy: {sidecars:?}"
            );
            // The rename preserves the 0o000 mode: grant read access to prove
            // the original bytes survived (only the test can do this — the
            // production path never needs to read them).
            std::fs::set_permissions(&sidecars[0], std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(
                std::fs::read(&sidecars[0]).unwrap(),
                original,
                "the preserved copy must carry the original bytes"
            );

            // The store is the new write's state again and loads cleanly.
            let file = load_disabled_bundles_file();
            assert!(
                file.initialized.contains("plain"),
                "the opt-in write landed: {file:?}"
            );
        });
    }

    /// Round-20 MAJOR B: rename-aside succeeded but the write failed — the
    /// unreadable original must be renamed BACK (store in place, marker still
    /// armed) instead of leaving no live file and no memo. The previous form
    /// cleared the marker right after the rename, so the next read took the
    /// NotFound branch, the wide signal judged "upgraded", and plain
    /// initialized empty: every opted-out pack back ON, inside the very
    /// machinery built to prevent the loss.
    #[cfg(unix)]
    #[test]
    fn failed_write_after_rename_aside_restores_original() {
        use std::os::unix::fs::PermissionsExt;
        with_temp_home("pinvou3-scope", || {
            let path = disabled_bundles_path();
            let original = b"unrecoverable-user-optouts{{{".to_vec();
            std::fs::write(&path, &original).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
            // Root probe (mode bits are no-ops for root): if the file is still
            // readable, the unreadable-read branch never runs — skip loudly.
            if std::fs::read(&path).is_ok() {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
                eprintln!(
                    "ROOT-SKIP[failed_write_after_rename_aside_restores_original]: running as root - the unreadable-file fixture stays readable; NOT exercised"
                );
                return;
            }
            // A read of the unreadable file arms the marker (fail-closed
            // recovery in memory) — the same state a real episode holds when
            // the next persist runs.
            let _ = load_disabled_bundles_file();

            let _fail = fail_next_disabled_bundles_write_for_test();
            let result = save_disabled_bundles_for(ConnectorScope::Plain, &[]);
            assert!(
                result.is_err(),
                "the injected write failure must surface: {result:?}"
            );

            // The unreadable original is back in place, byte-identical, and
            // no sidecar remains.
            assert!(path.exists(), "the store must not sit absent");
            let parent = path.parent().unwrap();
            let sidecars: Vec<std::path::PathBuf> = std::fs::read_dir(parent)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.contains(".corrupt."))
                        .unwrap_or(false)
                })
                .collect();
            assert!(
                sidecars.is_empty(),
                "the sidecar was renamed back: {sidecars:?}"
            );
            // The marker survived the failed write: the next successful save
            // again renames the original aside before overwriting, and the
            // bytes stay recoverable.
            let result = save_disabled_bundles_for(ConnectorScope::Plain, &[]);
            assert!(
                result.is_ok(),
                "a writable home lets the retry succeed: {result:?}"
            );
            let sidecars: Vec<std::path::PathBuf> = std::fs::read_dir(parent)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.contains(".corrupt."))
                        .unwrap_or(false)
                })
                .collect();
            assert_eq!(
                sidecars.len(),
                1,
                "the retry preserved the original: {sidecars:?}"
            );
            std::fs::set_permissions(&sidecars[0], std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(
                std::fs::read(&sidecars[0]).unwrap(),
                original,
                "the preserved copy must carry the original bytes"
            );
        });
    }

    /// Round-20 MAJOR B, crash-window arm: the live store is gone but a
    /// `.corrupt.*` sibling proves it existed — the NotFound read must
    /// recover fail-closed (no scope initialized, freeze persisted) instead
    /// of letting the wide upgrade signal (installed.json ∪ non-empty
    /// sessions/) initialize plain empty: every opt-out back ON.
    #[test]
    fn not_found_read_with_corrupt_sidecar_fails_closed() {
        with_temp_home("pinvou3-scope", || {
            let home = paths::pinvou3_home();
            // Both wide-signal legs present: without the sibling-evidence arm
            // this home judges "upgraded" and initializes plain empty.
            let installed = home.join("marketplace").join("installed.json");
            std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
            std::fs::write(&installed, "[\"weather\"]").unwrap();
            std::fs::create_dir_all(home.join("sessions").join("default")).unwrap();
            std::fs::write(home.join("sessions").join("default").join("t.json"), "{}").unwrap();
            std::fs::write(home.join("disabled_bundles.json.corrupt.123"), b"lost").unwrap();

            let file = load_disabled_bundles_file();
            assert!(
                file.plain_defaults_migrated,
                "the lost-store recovery is frozen: {file:?}"
            );
            assert!(
                !file.initialized.contains("plain"),
                "plain must NOT initialize — DenyAll fallback keeps the stranded opt-outs off: {file:?}"
            );
            // The freeze persisted: a second read is stable (no re-evaluation).
            let again = load_disabled_bundles_file();
            assert_eq!(again.initialized, file.initialized);
            assert!(again.plain_defaults_migrated);
        });
    }

    #[test]
    fn bundles_roundtrip_per_scope() {
        with_temp_home("pinvou3-scope", || {
            // DenyAll: an uninitialized plain scope reads as the on-the-fly
            // expansion, so pin an explicitly initialized empty baseline first.
            save_disabled_bundles_for(ConnectorScope::Plain, &[]).unwrap();
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
            // Hidden starts empty; disabled does not — after the DenyAll
            // convergence an uninitialized plain scope falls back to the
            // on-the-fly expansion, so pin an explicitly initialized empty
            // baseline first (same shape as
            // `hidden_bundles_are_orthogonal_to_disabled`).
            save_disabled_bundles_for(ConnectorScope::Plain, &[]).unwrap();
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

    /// Round-11 B2 schema: `default_off_scopes` is backward compatible (an old
    /// file without the field parses with empty defaults, and legacy stored
    /// entries count as user-explicit → refused by the batch enable), the
    /// install-sync marker roundtrips on disk, and a composer whole-list write
    /// only re-attributes the entries it actually transitioned (round-12
    /// self-review: clearing the whole scope made any unrelated toggle turn an
    /// untouched install-default pack into an explicit opt-out).
    #[test]
    fn default_off_scopes_schema_backward_compat_and_roundtrip() {
        with_temp_home("pinvou3-scope", || {
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
            let blocked = enable_packages_in_scope(ConnectorScope::Plain, &["weather".to_string()])
                .unwrap()
                .blocked;
            assert_eq!(blocked, vec!["weather".to_string()]);

            // Install-sync writes stored + default marker; the field
            // roundtrips through the on-disk file.
            sync_deny_all_scopes_after_install("pptx").unwrap();
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
            let blocked = enable_packages_in_scope(ConnectorScope::Plain, &["pptx".to_string()])
                .unwrap()
                .blocked;
            assert!(blocked.is_empty(), "default-off lifts freely: {blocked:?}");

            // A composer whole-list write does not re-attribute the entries it
            // never transitioned: re-arm the install default for `pptx` (the
            // enable above cleared it), then write a list that keeps `pptx` and
            // adds `weather`, which the user turns off in that very write.
            sync_deny_all_scopes_after_install("pptx").unwrap();
            save_disabled_bundles_for(
                ConnectorScope::Plain,
                &["pptx".to_string(), "weather".to_string()],
            )
            .unwrap();
            let file = load_disabled_bundles_file();
            assert!(
                file.default_off_scopes
                    .get("plain")
                    .map(|d| d.iter().any(|id| id == "pptx"))
                    .unwrap_or(false),
                "an untouched install-default entry keeps its marker: {file:?}"
            );
            let blocked = enable_packages_in_scope(ConnectorScope::Plain, &["pptx".to_string()])
                .unwrap()
                .blocked;
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
            let blocked = enable_packages_in_scope(ConnectorScope::Plain, &["weather".to_string()])
                .unwrap()
                .blocked;
            assert_eq!(blocked, vec!["weather".to_string()]);
        });
    }

    /// A stale install-default marker must not survive the removal of its
    /// stored entry (round-12 self-review): uninstall/logout clears the stored
    /// entry, the user switches the connector off again, and a marker left
    /// behind would let the next welcome/scene opt-in lift that user verdict.
    #[test]
    fn remove_bundle_clears_the_install_default_marker() {
        with_temp_home("pinvou3-scope", || {
            let path = disabled_bundles_path();
            std::fs::write(
                &path,
                r#"{"scopes":{"plain":["pptx"]},"default_off_scopes":{"plain":["pptx"]},"initialized":["plain"],"plain_defaults_migrated":true}"#,
            )
            .unwrap();
            remove_bundle_from_disabled_scopes("pptx").unwrap();
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
        with_temp_home("pinvou3-scope", || {
            let path = disabled_bundles_path();
            std::fs::write(
                &path,
                r#"{"scopes":{"plain":["pptx"]},"default_off_scopes":{"plain":["pptx"]},"initialized":["plain"],"plain_defaults_migrated":true}"#,
            )
            .unwrap();
            // Taking pptx back on removes the stored entry, and the
            // install-default marker goes with it.
            save_disabled_bundles_for(ConnectorScope::Plain, &[]).unwrap();
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
            save_disabled_bundles_for(ConnectorScope::Plain, &["pptx".to_string()]).unwrap();
            let file = load_disabled_bundles_file();
            assert!(
                file.default_off_scopes
                    .get("plain")
                    .map(|d| d.is_empty())
                    .unwrap_or(true),
                "the user's own switch-off must not re-arm a marker: {file:?}"
            );
            assert_eq!(
                enable_packages_in_scope(ConnectorScope::Plain, &["pptx".to_string()])
                    .unwrap()
                    .blocked,
                vec!["pptx".to_string()],
                "the user's explicit off is refused by the batch enable"
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
        with_temp_home("pinvou3-scope", || {
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

    /// Round-13 B1: the first composer whole-list write on a fresh install
    /// (plain uninitialized) materializes the DenyAll expansion minus the
    /// toggled-on pack. Every untouched entry must land as an
    /// install-default (marker seeded from the pre-write effective
    /// expansion) — without seeding, each becomes an unattributed "explicit
    /// opt-out" and the welcome/scene opt-in refuses it forever, a verdict
    /// the user never made.
    #[test]
    fn composer_first_write_on_fresh_install_seeds_default_markers() {
        with_temp_home("pinvou3-scope", || {
            // Fresh install: plain uninitialized, the effective disabled set
            // is the on-the-fly DenyAll expansion (covers the builtin CLI
            // packs).
            let expansion = load_disabled_bundles_for(ConnectorScope::Plain);
            assert!(
                expansion.contains(&"feishu".to_string())
                    && expansion.contains(&"wecom".to_string()),
                "the DenyAll expansion covers the builtin CLI packs: {expansion:?}"
            );

            // The composer holds the effective set; the user toggles feishu
            // on and the whole list is written back (feishu removed).
            let mut composer_list = expansion.clone();
            composer_list.retain(|id| id != "feishu");
            save_disabled_bundles_for(ConnectorScope::Plain, &composer_list).unwrap();

            let file = load_disabled_bundles_file();
            assert!(
                file.initialized.contains("plain"),
                "the first write initializes plain: {file:?}"
            );
            assert_eq!(
                file.scopes.get("plain").cloned().unwrap_or_default(),
                composer_list,
                "the stored list is what the composer sent: {file:?}"
            );
            let markers = file
                .default_off_scopes
                .get("plain")
                .cloned()
                .unwrap_or_default();
            for id in &composer_list {
                assert!(
                    markers.contains(id),
                    "untouched default {id} must keep a liftable marker: {markers:?}"
                );
            }
            assert!(
                !markers.contains(&"feishu".to_string()),
                "the pack this write turned on is the user's verdict, no marker: {markers:?}"
            );

            // The welcome/scene opt-in for another pack lifts freely — no
            // "explicit opt-out" refusal for a verdict the user never made.
            let outcome =
                enable_packages_in_scope(ConnectorScope::Plain, &["wecom".to_string()]).unwrap();
            assert!(
                outcome.blocked.is_empty(),
                "seeded install-default must not trip the explicit refusal: {:?}",
                outcome.blocked
            );
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain).contains(&"wecom".to_string()),
                "the opt-in lifted the seeded default"
            );
        });
    }

    /// Round-13 m3: requested ids absent from the DenyAll expansion get
    /// nothing applied and are reported in `not_applied` instead of the
    /// command reporting a plain success (an install committing right after
    /// the snapshot, or an unknown id).
    #[test]
    fn enable_reports_ids_absent_from_the_expansion() {
        with_temp_home("pinvou3-scope", || {
            // Pure miss: no opt-in materialized, nothing persisted.
            let outcome =
                enable_packages_in_scope(ConnectorScope::Plain, &["not-a-pack".to_string()])
                    .unwrap();
            assert_eq!(outcome.not_applied, vec!["not-a-pack".to_string()]);
            assert!(outcome.blocked.is_empty());
            let file = load_disabled_bundles_file();
            assert!(
                !file.initialized.contains("plain"),
                "a full miss must not materialize the scope: {file:?}"
            );

            // Mixed batch: the matched id materializes, the miss is reported.
            let outcome = enable_packages_in_scope(
                ConnectorScope::Plain,
                &["feishu".to_string(), "not-a-pack".to_string()],
            )
            .unwrap();
            assert_eq!(outcome.not_applied, vec!["not-a-pack".to_string()]);
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain).contains(&"feishu".to_string()),
                "the matched id is enabled"
            );
            let file = load_disabled_bundles_file();
            assert!(
                file.initialized.contains("plain"),
                "the matched part materialized the opt-in: {file:?}"
            );

            // An id that IS in the expansion is applied, not reported.
            let outcome =
                enable_packages_in_scope(ConnectorScope::Plain, &["wecom".to_string()]).unwrap();
            assert!(outcome.not_applied.is_empty(), "{outcome:?}");
            assert!(outcome.blocked.is_empty());
        });
    }

    /// Round-13 B3: the install-sync persist is fail-visible — for the
    /// migrated cohort every scope here is initialized and the stored list is
    /// the authoritative consent store, so a swallowed save would leave the
    /// pack ON in every new session with zero consent while the install
    /// reported success. Fixture: a read-only home forces the write to fail;
    /// the open() probe keeps the root skip loud (round-11 m12 pattern).
    #[cfg(unix)]
    #[test]
    fn install_sync_persist_failure_is_reported_and_retryable() {
        use std::os::unix::fs::PermissionsExt;

        with_temp_home("pinvou3-scope", || {
            // Upgraded cohort: plain initialized with an empty stored list.
            let path = disabled_bundles_path();
            std::fs::write(
                &path,
                r#"{"scopes":{"plain":[]},"initialized":["plain"],"plain_defaults_migrated":true}"#,
            )
            .unwrap();

            let home = crate::platform::paths::pinvou3_home();
            std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o555)).unwrap();
            let probe = home.join(".root-probe");
            if std::fs::write(&probe, b"").is_ok() {
                let _ = std::fs::remove_file(&probe);
                std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
                eprintln!(
                    "ROOT-SKIP[install_sync_persist_failure_is_reported_and_retryable]: running as root - read-only home fixture stays writable; NOT exercised"
                );
                return;
            }

            let error = sync_deny_all_scopes_after_install("pptx")
                .expect_err("a failed persist must surface as Err, not as a silent success");
            assert!(
                error.contains("disabled_bundles.json"),
                "the failure must name the file it could not write: {error}"
            );

            // Nothing was half-applied; the same gesture succeeds once the
            // environment can persist again, with the install-default marker.
            std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
            sync_deny_all_scopes_after_install("pptx").unwrap();
            let file = load_disabled_bundles_file();
            assert!(
                file.scopes
                    .get("plain")
                    .map(|ids| ids.iter().any(|id| id == "pptx"))
                    .unwrap_or(false),
                "the retried sync persisted the pack default-off: {file:?}"
            );
            assert!(
                file.default_off_scopes
                    .get("plain")
                    .map(|ids| ids.iter().any(|id| id == "pptx"))
                    .unwrap_or(false),
                "the retried sync marked the entry install-default: {file:?}"
            );
        });
    }

    /// Round-14 minor #2: the freeze persist-failure memo (`UNPERSISTED_VERDICT`)
    /// is load-bearing but was completely unpinned — deleting it kept every test
    /// green. Fixture: fresh home, first read under a read-only home (freeze
    /// persist fails, memo carries the verdict), then a first-boot trace
    /// (`sessions/default`) appears and the file is re-read. With the memo the
    /// fresh-install verdict survives (plain stays DenyAll); without it the
    /// wide signal judges the install upgraded and flips plain fully on
    /// (fail-open).
    #[cfg(unix)]
    #[test]
    fn freeze_persist_failure_survives_first_boot_trace_within_process() {
        use std::os::unix::fs::PermissionsExt;

        with_temp_home("pinvou3-scope", || {
            let home = crate::platform::paths::pinvou3_home();
            std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o555)).unwrap();
            let probe = home.join(".root-probe");
            if std::fs::write(&probe, b"").is_ok() {
                let _ = std::fs::remove_file(&probe);
                std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
                eprintln!(
                    "ROOT-SKIP[freeze_persist_failure_survives_first_boot_trace_within_process]: running as root - read-only home fixture stays writable; NOT exercised"
                );
                return;
            }

            // First read on a fresh home: fresh-install verdict, persist fails,
            // the in-process memo carries the verdict.
            let file = load_disabled_bundles_file();
            assert!(
                file.plain_defaults_migrated && !file.initialized.contains("plain"),
                "fresh-install verdict held in memory: {file:?}"
            );
            assert!(
                !disabled_bundles_path().exists(),
                "the freeze persist must have failed under the read-only home"
            );

            // First-boot trace appears, then a later read in the same process.
            std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::create_dir_all(paths::sessions_root().join("default")).unwrap();

            let file = load_disabled_bundles_file();
            assert!(
                !file.initialized.contains("plain"),
                "the memo must deny the first-boot trace a re-evaluation (fail-open flip otherwise): {file:?}"
            );
            // The verdict now persists: a further save path lands the frozen
            // file. The composer write always transitions (uninitialized →
            // initialized), unlike the install-sync — a no-op for
            // uninitialized scopes, so it would never persist here.
            save_disabled_bundles_for(ConnectorScope::Plain, &[]).unwrap();
            assert!(
                disabled_bundles_path().exists(),
                "the memo-carried verdict must reach disk on the next save: {:?}",
                load_disabled_bundles_file()
            );
        });
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
