//! 包 id × SessionMode 的单一禁用集（`~/.pinvou3/disabled_bundles.json`）。
//!
//! 这是「工具市场统一治理」scope 收敛的单一真相源：取代原先
//! `disabled_connectors.json`（连接器 id）与 `disabled_skills.json`（技能 id）两份
//! 文件。开关粒度收敛为**包 id**（= `bundle.rs` 里 `BundleInfo.id`，即 MCP 工具 id /
//! 技能 id / CLI 连接器 id），一个包 = 一个开关，包内技能（companion skills）可见性
//! 唯一跟随所属包（§5.2 不变量）。
//!
//! On-disk format, isomorphic to the #287-generalized legacy pair: `{scopes, hidden_scopes,
//! default_off_scopes: {"<mode>": [...]}, initialized: ["<mode>"],
//! project_skills_enabled, plain_defaults_migrated}`. Scope keys use `SessionMode`
//! kebab-case names. `plain_defaults_migrated`: marks files written while plain
//! was still AllowAll; migrate-on-read initializes plain to the persisted list
//! and lands the marker on disk. First read migrates the two legacy files
//! into this file (migrate-on-read):
//! Legacy connector ids pass through as pack ids (connector id = pack id); legacy
//! skill ids map to their owner via `to_package_id` (the gating mapping `skill_gating_owner` since R17-MAJOR1);
//! `skill:` prefix residue is stripped. Migration is idempotent; failure falls back to defaults (safe).
//!
// architecture-guard: allow-target-cfg -- the unix regression tests in this file (the round-13 B3 install-sync persist failure and the round-14 #2 freeze memo) need unreadable (0o555 directory / 0o000 file) fixtures; test-only inline cfg(unix)+PermissionsExt (same exemption precedent as mod.rs / package_export.rs, review #455); a real open() probe guards against running as root, Windows is covered by link checks.
//!
//! 依赖方向：本模块与 `bundle` / `skill_marketplace` 同属 marketplace 领域，只依赖
//! `platform::paths` 与 marketplace 内既有类型，不反向依赖 assistant 运行时。

use std::path::PathBuf;
use std::sync::Mutex;

use crate::core::session_mode::{PackDefaultPolicy, SessionMode};
use crate::features::marketplace::bundle::{builtin_cli_bundle_ids, skill_owner_package_with};
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
    /// Round-31 BLOCKER (review #455): ledger of `"<scope>:<pack>"` pairs whose
    /// install-default row the install/connect/startup sync has successfully
    /// persisted — or that a sync observed while the scope was still
    /// uninitialized (round-32 MAJOR 1, review #455). The startup
    /// connector-gate refresh pushes rows only for pairs **absent** here:
    /// "user enabled" and "never synced" are observably identical on current
    /// state alone (both = skills materialized + row absent), so a plain
    /// membership push re-added the row at every boot and silently reverted
    /// explicit enables. A later user enable removes the stored row while this
    /// entry survives — its presence is exactly the "sync already ran once"
    /// fact. Recording in the uninitialized arm closes the second cohort: a
    /// connect on a fresh home (plain uninitialized by design) used to record
    /// nothing, so the user's first enable was backfilled over at the next
    /// boot. Teardown ([`remove_bundle_from_disabled_scopes_exact`]) clears
    /// the pack's entries so a fresh install / reconnect re-syncs default-off.
    #[serde(default)]
    pub install_default_synced: Vec<String>,
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
/// after a successful save the file is the truth and both memos are cleared
/// (scope.rs persist path). On a persist failure the memo carries the verdict
/// for the rest of the process — every later read reuses it instead of
/// re-evaluating (round-28 nit: the round-26 wording claimed the memo is
/// "not cleared on later successful saves", which the code disproves).
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
/// Round-29 m1 (review #455) widens the arming to the corrupt-store read
/// whose **quarantine write failed**: there too the bytes exist with no
/// preserved copy, and the next persist must rename them aside before
/// overwriting. `try_save` first renames the original aside (a rename needs
/// only directory write permission, so it always succeeds) and clears this
/// memo. Without it, "unreadable original → any write" permanently destroys
/// the user's explicit opt-outs with no quarantine copy — the R6-B1
/// violation the read path already refuses for itself.
static UNREADABLE_ORIGINAL: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Clears the verdict memos. Test-only hygiene: the statics are process-global
/// and keyed by home path, so a memo left behind by an aborted earlier case
/// (or by a harness that recreates the same path) must not bleed into the
/// next one; production never clears (the file is the truth; after a write
/// failure a memo carries state for the rest of the process lifetime, until a
/// later writer's save succeeds).
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
            let (mut file, legacy_unconsumable) = migrate_from_legacy_files();
            if legacy_unconsumable {
                // A legacy scope file EXISTS but cannot be consumed (read
                // failure, corrupt JSON, unrecognized/malformed shape).
                // Migrating it leniently would bake "the user disabled
                // nothing" into the frozen verdict while the wide signal
                // above still judges the install upgraded and initializes
                // plain EMPTY — every lost opt-out back ON, permanently
                // (the exact flip the corrupt-sidecar arm above exists to
                // prevent). Same doctrine: recover fail-closed — set only
                // the migration marker, initialize no scope (the DenyAll
                // fallback keeps every previously opted-out pack off),
                // freeze. The legacy file is read-only history and is left
                // untouched for manual recovery; main's strict legacy
                // parser (and its pins, removed earlier in this PR) is
                // restored here over the new fail-closed recovery shape.
                eprintln!(
                    "[marketplace] a legacy scope file exists but cannot be consumed; recovering fail-closed (all scopes fall back to DenyAll), freeze persisted"
                );
                let recovered = DisabledBundlesFile {
                    plain_defaults_migrated: true,
                    ..DisabledBundlesFile::default()
                };
                if let Err(freeze_error) = try_save_disabled_bundles_file(&recovered) {
                    eprintln!(
                        "[scope] CRITICAL: failed to persist the unconsumable-legacy recovery verdict: {freeze_error}; holding the in-process verdict until restart"
                    );
                    *UNPERSISTED_VERDICT
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some((home, recovered.clone()));
                }
                return recovered;
            }
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
            // fallback) **in this process only** — across a restart the
            // first-boot-trace re-evaluation hazard returns (the registered
            // crash-during-freeze family, round-28 nit).
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
        // Round-30 m2 (review #455): the freeze branches' R7-M2 contract —
        // first-read verdicts surface persist failures through the
        // UNPERSISTED_VERDICT memo — applies to the legacy migration too:
        // it persists the same kind of frozen verdict (marker set, plain
        // initialized empty). The log-and-drop wrapper made an unwritable
        // home silently re-run the migration + catalog walk every boot; the
        // memo keeps the in-process verdict and hands it to a later
        // NotFound read (the re-run itself stays idempotent — the verdict
        // derives from the file's own contents).
        if let Err(error) = try_save_disabled_bundles_file(&file) {
            eprintln!(
                "[scope] CRITICAL: failed to persist the legacy migration verdict: {error}; holding the in-process verdict until a save succeeds"
            );
            *UNPERSISTED_VERDICT
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some((paths::pinvou3_home(), file.clone()));
        }
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
        // Round-29 m1 (review #455): quarantine failed, so NO preserved copy
        // of the corrupt bytes exists and the original is still in place —
        // but only this read skips the overwrite. Arm the unreadable-original
        // marker so a LATER load-then-save writer's persist renames those
        // bytes aside before overwriting (differential transient failure:
        // the quarantine write fails now, the main write succeeds later).
        // Cleared by try_save's successful-persist tail.
        *UNREADABLE_ORIGINAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(paths::pinvou3_home());
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
    // Round-27 m9 (review #455): the walk is skipped entirely when every
    // stored list is empty — the all-empty state is the common shape for
    // fresh/minimal installs, and a full manifest walk per load there was a
    // pure regression vs base (it multiplies across every consumer).
    if file.scopes.values().all(|v| v.is_empty())
        && file.hidden_scopes.values().all(|v| v.is_empty())
        && file.default_off_scopes.values().all(|v| v.is_empty())
    {
        return false;
    }
    // Round-24 MAJOR 3 (related cheaper hoist): one manifest walk serves every
    // non-empty list on the file — the per-list normalize re-parsed all
    // manifests once per list on every load.
    let tools = MarketplaceManager::new().available_tools();
    let mut changed = false;
    for ids in file
        .scopes
        .values_mut()
        .chain(file.hidden_scopes.values_mut())
        .chain(file.default_off_scopes.values_mut())
    {
        let normalized = normalize_stored_pkg_ids_with(&tools, ids);
        if normalized.len() != ids.len() || normalized.iter().zip(ids.iter()).any(|(a, b)| a != b) {
            *ids = normalized;
            changed = true;
        }
    }
    changed
}

/// Raw entry → pack id. Connector/CLI ids pass through unchanged
/// (`skill_gating_owner` is the identity for them); a `skill:` prefix is
/// stripped and the skill name maps to its owner pack. The gating side uses
/// the physical-aware `skill_gating_owner` (R17-MAJOR1): stored entries may
/// carry combination-pack inner skill names (written by the restore gate's
/// bin-side skill-dir scan), and an undeclared nested skill must resolve to
/// its physical owner pack — otherwise it enters every scope with zero
/// consent and no composer row to turn it off.
/// Round-33 MAJOR 1 (review #455): caller-visible normalization — the install
/// path compares a companion's normalized id against the tool's own to tell
/// "the tool-level sync provably covered this companion" from "this sync is
/// a real write for a different pack" (the known-pack-shield edge).
/// Round-33 MAJOR 2 (review #455): the ONE consent-sync failure marker the
/// frontend keys its actionable-guidance template on (ToolStoreView's
/// consentFailure matcher; pinned per emitter by
/// `consent_failure_message_keeps_the_frontend_marker` in ima.rs and
/// `skill_gate_consent_failure_message_keeps_the_frontend_marker` in
/// skill_gate.rs). Every connector shares the exact string so a backend
/// rewording cannot silently degrade the localized guidance to generic copy.
pub const CONSENT_SYNC_FAILURE_MARKER: &str = "persisting their default-off consent state failed";

pub(crate) fn to_package_id(raw: &str) -> String {
    to_package_id_with(&MarketplaceManager::new().available_tools(), raw)
}

/// `to_package_id` over a pre-walked tool snapshot (round-23 MINOR 3
/// hoist): one `available_tools()` walk serves the whole id list instead of
/// one per entry. `pub(crate)` since round-37 P3 (review #455): the builtin
/// writer guard hoists the same snapshot for its normalization loop.
/// `pub` since round-38: the CLI's `plugins enable/disable` normalize two id
/// lists per scope and hoist the same snapshot instead of re-walking per
/// entry (the anti-pattern this hoist exists for). The wrapper above is
/// spelled out rather than intra-doc-linked because it is `pub(crate)` — a
/// public doc linking a private item fails the `-D warnings` rustdoc gate
/// (round-40 review).
pub fn to_package_id_with(tools: &[super::ToolManifest], raw: &str) -> String {
    let stripped = raw.strip_prefix("skill:").unwrap_or(raw);
    // Known-pack shield (review #455 round-23 MINOR 1): a stored entry that
    // names a physically present pack dir IS that pack and must not be
    // re-routed through the gating owner's physical fallback — with a skill
    // dir nested under another pack sharing the name (`bundles/<a>/skills/pptx`
    // vs a real `pptx` pack, sorted-first), the fallback hijacked the stored
    // opt-out onto `<a>` and silently persisted the remap (P re-enabled, Q
    // disabled). The fallback stays available for skill-gated inputs below —
    // its purpose is mapping undeclared nested skill names, never renaming
    // stored pack rows.
    let pack_row = paths::bundles_root().join(stripped).is_dir()
        // Round-24 MAJOR 1: a pack staged in the recycle bin is equally a
        // pack row — while it awaits restore, its consent row (written
        // verbatim by the gate) must not be re-owned on read through a
        // foreign claim or nesting.
        //
        // Round-28 MINOR 6 (review #455) — disclosed neither-leg window:
        // during `import_plugin_package`'s rename `bundles/p` -> `bundles/p.old`
        // a concurrent load sees p neither on-disk nor in the bin and can
        // remap a stored "p" row onto a foreign claimant (persisted). The
        // window is the rename instant only and atomic on one filesystem, but
        // if the re-import then fails and p is restored, p's row is gone —
        // registered alongside #515 rather than answered with a lock here.
        || super::recycle_bin::RecycleBin::held_dir(stripped).is_dir();
    if pack_row {
        return stripped.to_string();
    }
    crate::features::marketplace::bundle::skill_gating_owner_with(tools, stripped)
}

/// Resolve a stored id to its pack owner **while the package state is still
/// intact** — the pre-teardown snapshot for the exact-cleanup shape (round-26
/// MAJOR 1, review #455). A writer that deletes a skill/package directory and
/// then cleans its consent rows must capture the owner **before** the
/// deletion: once the dir is gone the gating fallback can be hijacked by a
/// foreign pack's `companion_skills` claim or physical nesting, and the
/// cleanup would erase the foreign pack's rows. Pass the snapshot to
/// [`remove_bundle_from_disabled_scopes_exact`].
pub fn resolve_pack_owner_id(raw_id: &str) -> String {
    to_package_id(raw_id)
}

/// Legacy-migration filter: builtin plugins can never be disabled
/// (docs/builtin-toolset-contract.md §3.3), so entries normalizing to a
/// builtin package are skipped instead of written — once persisted, such an
/// id could never be removed through the guarded write paths. Migration stays
/// best-effort (skip rather than reject): one poisoned legacy entry must not
/// abort migrating the rest of the file.
fn migration_keeps_id(id: &str) -> bool {
    !crate::features::marketplace::builtin::is_builtin_tool(id)
}

// Round-40 review: the CLI-facing duplicate of `resolve_pack_owner_id` is
// gone — the CLI now imports that name, so one operation has one public
// symbol and the two cannot drift.

/// 读时归一：存储条目按**当前**认领状态重映射为包 id 并去重（保序）。
/// 认领（`skill_owner_package`）随安装态时变：条目可能在 companion MCP 未装时
/// 按独立技能 id 落库，MCP 后装则认领翻转到包 id——只在写时归一会让用户的
/// 「关/隐藏」在认领翻转后静默失效（F4）；读时归一让门控跟随技能本体。
///
/// Self-heal (docs/builtin-toolset-contract.md §3.1): builtin plugins can
/// never be disabled/hidden and every writer rejects builtin ids, so a stored
/// entry normalizing to a builtin package is always poisoned state (a legacy
/// bug window, a hand-edited file). Drop it at read time — keeping it would
/// feed the id back to the frontend's full-list writes, which
/// `reject_builtin_ids` then fails wholesale, bricking every toggle of the
/// scope with no UI recovery.
fn normalize_stored_pkg_ids(ids: &[String]) -> Vec<String> {
    // Round-31 perf (review #455 ledger, "the cheapest win"): the empty input
    // is the common shape for fresh/minimal stores — the full manifest walk
    // (available_tools) ran even for it on every resolve call; the empty
    // result is the answer without consulting any manifest.
    if ids.is_empty() {
        return Vec::new();
    }
    let tools = MarketplaceManager::new().available_tools();
    normalize_stored_pkg_ids_with(&tools, ids)
}

/// [`normalize_stored_pkg_ids`] over a pre-walked tool snapshot (round-24
/// MAJOR 3: callers hoisting across several lists share one walk).
fn normalize_stored_pkg_ids_with(tools: &[super::ToolManifest], ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(ids.len());
    for id in ids {
        let pkg = to_package_id_with(tools, id);
        if crate::features::marketplace::builtin::is_builtin_tool(&pkg) {
            continue;
        }
        if !out.iter().any(|x| x == &pkg) {
            out.push(pkg);
        }
    }
    out
}

/// First-boot migration: reads the legacy `disabled_connectors.json` and
/// `disabled_skills.json` (each accepting the three documented shapes, parsed
/// STRICTLY — see [`merge_legacy_scope_file_into`]), maps the entries to
/// bundle ids, and merges them into the new file — connector scopes overwrite
/// while skill scopes union, with `project_skills_enabled` taken from the
/// skill file. The legacy files are not deleted (kept as read-only history
/// for this release cycle; retired alongside the legacy layout in a later
/// cycle).
///
/// The bool is "a legacy file exists but cannot be consumed": the caller must
/// not migrate such a state as "the user disabled nothing" — it recovers
/// fail-closed instead (review #455: the wide upgrade signal would otherwise
/// initialize plain EMPTY over the lost opt-outs and freeze them ON).
fn migrate_from_legacy_files() -> (DisabledBundlesFile, bool) {
    let mut file = DisabledBundlesFile::default();
    let mut unconsumable = false;
    unconsumable |= merge_connector_scopes_into(&mut file);
    unconsumable |= merge_skill_scopes_into(&mut file);
    (file, unconsumable)
}

/// 把旧 `disabled_connectors.json` 的各 scope 条目映射为包 id 并并进 `file`
/// （scope 条目按旧文件**覆盖写**）。返回「文件存在但不可消费」。
fn merge_connector_scopes_into(file: &mut DisabledBundlesFile) -> bool {
    merge_legacy_scope_file_into(
        file,
        &paths::pinvou3_home().join("disabled_connectors.json"),
        |file, key, ids| {
            file.scopes.insert(key.to_string(), ids);
        },
        false,
    )
}

/// 把旧 `disabled_skills.json` 的各 scope 条目映射为包 id 并并进 `file`（取并集），
/// 并继承 `project_skills_enabled`。返回「文件存在但不可消费」。
fn merge_skill_scopes_into(file: &mut DisabledBundlesFile) -> bool {
    merge_legacy_scope_file_into(
        file,
        &paths::pinvou3_home().join("disabled_skills.json"),
        |file, key, ids| merge_ids_into_scope(file, key, ids),
        true,
    )
}

/// 旧 scope 文件（`disabled_connectors.json` / `disabled_skills.json`）的共用解析
/// 骨架：裸数组 → plain scope、新版 `{scopes, initialized}` 对象、旧双 scope 对象
/// `{plain, code, code_initialized}` 三种形态，条目经 `to_package_id` 归一为包 id，
/// 并迁移 `initialized` / `code_initialized`。
///
/// `merge_ids` 决定 scope 条目的落库语义（连接器文件 = 覆盖写，技能文件 = 并集
/// 合并）；`inherit_project_flag` 为真时继承 `project_skills_enabled`（仅技能文件）。
///
/// 返回「文件**存在但不可消费**」：读失败、非 JSON、非对象、无任何可识别的
/// scope 形态、或已识别形态中的字段/条目类型错误（主分支 #455 评审恢复的严格
/// 解析语义：合法 JSON 但只匹配到一半的文件是损坏而非「什么都没关」——消费能
/// 解析的那一半会静默读作「从未禁用过」）。文件不存在是迁移常态，返回 false；
/// 两者必须区分，否则一次读失败会被当成「用户从未禁用过任何东西」。
fn merge_legacy_scope_file_into(
    file: &mut DisabledBundlesFile,
    path: &std::path::Path,
    merge_ids: impl Fn(&mut DisabledBundlesFile, &str, Vec<String>),
    inherit_project_flag: bool,
) -> bool {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(error) => {
            eprintln!("[scope] read {} failed: {error}", path.display());
            return true;
        }
    };
    // 裸数组 → plain scope
    if let Ok(list) = serde_json::from_str::<Vec<String>>(&content) {
        let ids: Vec<String> = list
            .iter()
            .map(|id| to_package_id(id))
            .filter(|id| migration_keeps_id(id))
            .collect();
        if !ids.is_empty() {
            merge_ids(file, SessionMode::Plain.as_str(), ids);
        }
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        eprintln!("[scope] parse {} failed", path.display());
        return true;
    };
    let Some(obj) = value.as_object() else {
        eprintln!("[scope] {} is not a JSON object", path.display());
        return true;
    };
    // A file that is valid JSON but only partially matches a documented shape is
    // corruption, not an empty state: consuming the half that happens to parse
    // would silently read as "nothing was disabled", so any wrong-typed field or
    // unrecognized shape fails closed exactly like an unreadable file.
    // Round-33 merge note: the builtin poison filter lives INSIDE the closure so
    // every shape (bare array, object scopes) filters — builtin ids can never
    // legally sit in a disabled list (skip the entry, don't fail the file).
    let scope_ids = |value: &serde_json::Value| -> Option<Vec<String>> {
        let arr = value.as_array()?;
        let mut ids = Vec::with_capacity(arr.len());
        for entry in arr {
            let id = to_package_id(entry.as_str()?);
            if migration_keeps_id(&id) {
                ids.push(id);
            }
        }
        Some(ids)
    };
    match obj.get("scopes") {
        Some(serde_json::Value::Object(scopes)) => {
            for (key, arr) in scopes {
                let Some(ids) = scope_ids(arr) else {
                    eprintln!(
                        "[scope] {} has a malformed scope entry {key}",
                        path.display()
                    );
                    return true;
                };
                if !ids.is_empty() {
                    merge_ids(file, key, ids);
                }
            }
            match obj.get("initialized") {
                None => {}
                Some(serde_json::Value::Array(keys)) => {
                    for key in keys.iter().filter_map(|v| v.as_str()) {
                        file.initialized.insert(key.to_string());
                    }
                }
                Some(_) => {
                    eprintln!("[scope] {} has a non-array \"initialized\"", path.display());
                    return true;
                }
            }
        }
        Some(_) => {
            eprintln!("[scope] {} has a non-object \"scopes\"", path.display());
            return true;
        }
        None if obj.contains_key("plain")
            || obj.contains_key("code")
            || obj.contains_key("code_initialized") =>
        {
            // 旧双 scope 对象 {plain, code, code_initialized}
            for key in ["plain", "code"] {
                let Some(value) = obj.get(key) else { continue };
                let Some(ids) = scope_ids(value) else {
                    eprintln!(
                        "[scope] {} has a malformed scope entry {key}",
                        path.display()
                    );
                    return true;
                };
                if !ids.is_empty() {
                    merge_ids(file, key, ids);
                }
            }
            match obj.get("code_initialized") {
                None => {}
                Some(serde_json::Value::Bool(true)) => {
                    file.initialized
                        .insert(SessionMode::Code.as_str().to_string());
                }
                Some(serde_json::Value::Bool(false)) => {}
                Some(_) => {
                    eprintln!(
                        "[scope] {} has a non-bool \"code_initialized\"",
                        path.display()
                    );
                    return true;
                }
            }
        }
        None => {
            eprintln!("[scope] {} has no recognizable scope shape", path.display());
            return true;
        }
    }
    if inherit_project_flag {
        match obj.get("project_skills_enabled") {
            None => {}
            Some(serde_json::Value::Bool(enabled)) => file.project_skills_enabled = *enabled,
            Some(_) => {
                eprintln!(
                    "[scope] {} has a non-bool \"project_skills_enabled\"",
                    path.display()
                );
                return true;
            }
        }
    }
    false
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
///
/// Best-effort entry point: failures are logged, not propagated. Governance
/// callers (toggles, visibility, install sync, freeze) use
/// [`try_save_disabled_bundles_file`] instead — a silently lost write would
/// let them continue on a half-applied state while the frontend reports
/// success (#571).
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
            // Round-21 minor 1: rename-aside copies use their OWN `.unreadable.`
            // namespace, deliberately distinct from quarantine's `.corrupt.` —
            // a stale preservation copy must not satisfy the no-sibling rule
            // and rob a later genuine corruption of its preserved copy. Both
            // kinds count as store-existed evidence for the NotFound read.
            let sidecar = path.with_file_name(format!("disabled_bundles.json.unreadable.{stamp}"));
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
        crate::platform::filesystem::atomic_write_private(&path, json.as_bytes())
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

/// Whether any preserved-copy sibling of `disabled_bundles.json` exists in
/// the home: quarantine copies (`…corrupt.<ts>`, shared with installed.json's
/// no-sibling rule) and the rename-aside preservation (`…unreadable.<ts>`,
/// round-21 minor 1 keeps the two kinds in separate namespaces) both prove
/// the converged-era store existed (round-20 MAJOR B).
fn corrupt_sidecar_evidence_exists(home: &std::path::Path) -> bool {
    const EVIDENCE_PREFIXES: [&str; 2] = [
        "disabled_bundles.json.corrupt.",
        "disabled_bundles.json.unreadable.",
    ];
    std::fs::read_dir(home)
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| EVIDENCE_PREFIXES.iter().any(|p| name.starts_with(p)))
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
///
/// The computed DenyAll default drops ids that normalize to a builtin
/// catalog entry (round-6 B1): builtins are feature-switch-governed, never
/// package-governed, and a default containing the boot-seeded builtin would
/// fail `reject_builtin_ids` on the very write that initializes the scope —
/// bricking every toggle on an uninitialized DenyAll profile.
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
            // Round-23 MINOR 3 hoist: one manifest walk per resolution pass —
            // `skill_owner_package`/`skill_gating_owner` each parse every
            // manifest under `bundles_root`, so the skill arms below were
            // O(skills × packs) manifest reads per expansion before this.
            let tools = manager.available_tools();
            let mut ids: Vec<String> = match manager.try_installed_ids() {
                Ok(ids) => ids,
                Err(error) => {
                    eprintln!(
                        "[scope] {error}; DenyAll expansion falls back to the full available catalog (fail-closed)"
                    );
                    let mut catalog: Vec<String> =
                        tools.iter().map(|manifest| manifest.id.clone()).collect();
                    // The two record arms deliberately use the claim mapping
                    // (`skill_owner_package`): their inputs are ids the registry itself claims —
                    // identical to the gating mapping whenever a claim exists, and standalone
                    // registered skills have no physical nested layout to fall back to. Only the
                    // disk leg below walks `skill_gating_owner`: it enumerates exactly the
                    // **unclaimed** physical directories and must stay on the same lens as
                    // materialization's directory scan (round-20 minor 3: the asymmetry vs the
                    // record arms is annotated here deliberately, not an oversight).
                    for info in SkillMarketplaceManager::new().list_skills() {
                        let pkg = skill_owner_package_with(&tools, &info.id);
                        if !catalog.iter().any(|id| id == &pkg) {
                            catalog.push(pkg);
                        }
                    }
                    catalog
                }
            };
            ids.extend(builtin_cli_bundle_ids().map(str::to_string));
            // Same as above (round-20 minor 3): the record arm uses the claim mapping, the disk leg the gating mapping.
            // #584 composition: the record arm probes STRICTLY — when skill
            // enumeration degrades, the blanket union below biases the freshly
            // computed default toward over-denial instead of letting a failed
            // probe silently shrink the consent set.
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
                    let pkg = skill_owner_package_with(&tools, &skill_id);
                    if !ids.iter().any(|id| id == &pkg) {
                        ids.push(pkg);
                    }
                }
            }
            for skill_id in skill_ids {
                let pkg = skill_owner_package_with(&tools, &skill_id);
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
                    // Round-26 minor 8 (review #455): skip import staging
                    // (`<id>.tmp`) and landing backup (`<id>.old`) dirs — the
                    // same exclusion materialization's scan applies
                    // (skill_materialization::skill_source_dirs). A staging
                    // dir resolving through the physical fallback to a
                    // suffix-owner id would be persisted by the composer's
                    // first-write seeding as a stored row + install-default
                    // marker (inert junk after the window; over-deny).
                    let pack_name = entry.file_name().to_string_lossy().into_owned();
                    if pack_name.ends_with(".tmp") || pack_name.ends_with(".old") {
                        continue;
                    }
                    let Ok(skills_dir) = std::fs::read_dir(entry.path().join("skills")) else {
                        continue;
                    };
                    for skill in skills_dir.flatten() {
                        // Same walk, one truth (round-23 MINOR 2): skip stray
                        // non-directory entries exactly like materialization's
                        // scan — a `skills/README.md` file would otherwise
                        // become a junk owner id that the seeding below then
                        // persists with an install-default marker (over-deny).
                        if !skill.path().is_dir() {
                            continue;
                        }
                        let name = skill.file_name().to_string_lossy().into_owned();
                        if name.is_empty() {
                            continue;
                        }
                        let pkg = crate::features::marketplace::bundle::skill_gating_owner_with(
                            &tools, &name,
                        );
                        if !ids.iter().any(|id| id == &pkg) {
                            ids.push(pkg);
                        }
                    }
                }
            }
            // Builtins are feature-switch-governed, never package-governed
            // (§3.1, same consent semantics as the DenyAll install-sync
            // exemption): the boot seed installs session-reader into
            // installed.json, and a computed default containing it would make
            // the echoed full-set write that initializes the scope fail
            // `reject_builtin_ids` — every composer toggle on an uninitialized
            // DenyAll profile then errors forever with no UI recovery. Drop
            // ids that normalize to a builtin, the same predicate
            // `normalize_stored_pkg_ids` and the guard itself apply.
            // Merge note: the retain runs AFTER the disk-derived arm so the
            // filter covers disk-derived ids too.
            ids.retain(|id| {
                !crate::features::marketplace::builtin::is_builtin_tool(&to_package_id_with(
                    &tools, id,
                ))
            });
            ids
        }
    }
}

/// 写某 scope 被禁用的包 id 列表（写入即标记该 scope 已初始化）。入参统一归一为包
/// id（剥 `skill:` 前缀 + companion 映射），防御历史版本误写入的带前缀条目。
/// Write failures propagate as-is (round-19 MAJOR 1: main #563's fail-loud
/// contract is shipped user-visible semantics and this PR does not downgrade
/// it — the composer caller's retry/rollback UX is #515 rework's to deliver).
pub fn save_disabled_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    // Builtin plugins cannot be disabled (§3.1): the guard lives on the
    // persistence function itself so every caller inherits it — not only
    // `apply_disabled_connectors_for` — and a future direct writer cannot
    // silently seed a builtin into an initialized scope (the read-time
    // self-heal would drop it again, but the write should refuse upfront).
    // The DenyAll computed default and the install-sync exemption already
    // exclude builtin ids, so legitimate internal callers are unaffected.
    crate::features::marketplace::builtin::reject_builtin_ids(ids)?;
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Round-23 MINOR 3 hoist: one manifest walk for the whole list.
    let tools = MarketplaceManager::new().available_tools();
    let normalized: Vec<String> = ids
        .iter()
        .map(|id| to_package_id_with(&tools, id))
        .collect();
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
    //
    // #521 评审接受的窗口（在此记录口径而非加锁）：expansion 经 bundles.json 的
    // 有界 try 读（records()）取得，若此刻一个安装的写锁正持 bundles 锁，这里会
    // 退化到安装前快照，在装包可能缺席 seeded_defaults（被物化成无 marker 的
    // 「用户自关」或缺席禁用集，直到该包的下次显式开关）。毫秒级窗口且仅进程内
    // （跨进程暴露与 main 相同——main 本就无跨进程排除）；闭合它需要把持锁的
    // store 快照贯穿 resolve_scope_disabled_ids 的整条调用链，对该窗口不成比例。
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
    // Round-23 MINOR 4 honesty note (direction corrected in round-24 minor
    // 9): when this write lands over an unreadable/recovered store (the
    // `UNREADABLE_ORIGINAL` / `PENDING_CORRUPT_RECOVERY` paths), the
    // in-memory file's scopes are uninitialized — the next composer save
    // takes the seeding arm above, which ADDS install-default markers to the
    // re-seeded entries. The original bytes are preserved rename-aside, but
    // attribution is lost in the opposite direction the round-23 wording
    // claimed: trapped user opt-outs are re-persisted as liftable
    // install-defaults, not as explicit verdicts — persisting entries
    // without markers would have made them MORE explicit, not less.
    // Attribution is lost, not
    // the verdicts; recovering the original's markers is the corrupt-recovery
    // rebuild's job, not this writer's.
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
/// Write failures propagate as-is (round-19 MAJOR 1, same as save_disabled_bundles_for).
pub fn save_hidden_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    // Builtin plugins cannot be hidden (§3.1): the guard lives on the manager
    // function (not just the command layer) so every caller inherits it —
    // same layering as the disable path (review round-5 minor 3).
    crate::features::marketplace::builtin::reject_builtin_ids(ids)?;
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Round-23 MINOR 3 hoist: one manifest walk for the whole list.
    let tools = MarketplaceManager::new().available_tools();
    let normalized: Vec<String> = ids
        .iter()
        .map(|id| to_package_id_with(&tools, id))
        .collect();
    let mut file = load_disabled_bundles_file_locked();
    file.hidden_scopes
        .insert(scope.as_str().to_string(), normalized);
    try_save_disabled_bundles_file(&file)
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
    sync_deny_all_scopes_inner(raw_id, false)
}

/// The STARTUP connector-gate-refresh variant (round-31 BLOCKER, review
/// #455): LEDGER-GATED — pushes rows only for `(scope, pack)` pairs that no
/// sync ever recorded. For a connected connector the refresh's `show` is
/// always true (the legacy disable flags are read-only), so a plain
/// membership push here re-added the row a user enable had removed at every
/// boot, silently reverting explicit enables. The ledger entry survives the
/// user's later enable (which removes the stored row, not this fact);
/// ledger absence is exactly the never-synced cohort the backfill targets.
/// The install/connect variant stays deliberately un-gated: connecting is a
/// user action with fresh-install semantics ("等同新装") and re-arms the
/// default-off — both variants record the ledger pair **even when the scope is
/// still uninitialized** (round-32 MAJOR 1, review #455), so this refresh
/// never undoes what either of them wrote, nor an enable made between a
/// pre-initialization sync and the scope's materialization.
pub fn sync_deny_all_scopes_refresh(raw_id: &str) -> Result<(), String> {
    sync_deny_all_scopes_inner(raw_id, true)
}

fn sync_deny_all_scopes_inner(raw_id: &str, ledger_gated: bool) -> Result<(), String> {
    let package_id = to_package_id(raw_id);
    // Builtin plugins can never be disabled (docs/builtin-toolset-contract.md
    // §3.3): exempt them here, or a direct-IPC reinstall of an already
    // installed builtin would seed it into the disabled set of every
    // initialized DenyAll scope with no UI path to remove it.
    if crate::features::marketplace::builtin::is_builtin_tool(&package_id) {
        return Ok(());
    }
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
            // Round-32 MAJOR 1 (review #455): the uninitialized arm must still
            // record the pair. A connect/install sync on a fresh home runs
            // exactly here — plain is uninitialized by design until the
            // user's first composer/welcome/scene write materializes it — so
            // skipping the ledger made the user's FIRST enable observably
            // identical to "never synced": the next boot's refresh backfilled
            // the default-off row over it. Recording (without pushing any
            // row — the uninitialized scope is covered by the on-the-fly
            // expansion) marks the pair as seen, so the post-materialization
            // refresh leaves the enable in place.
            let ledger_key = format!("{key}:{package_id}");
            if !file.install_default_synced.contains(&ledger_key) {
                file.install_default_synced.push(ledger_key);
                changed = true;
            }
            continue;
        }
        let ledger_key = format!("{key}:{package_id}");
        if ledger_gated && file.install_default_synced.contains(&ledger_key) {
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
        // Recorded even when the row already existed — the ledger answers
        // "did a sync ever run for this pair", which is what keeps the
        // STARTUP refresh from re-adding a lifted row later.
        if !file.install_default_synced.contains(&ledger_key) {
            file.install_default_synced.push(ledger_key);
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
///
/// Round-26 MAJOR 1 (review #455): writers that run **after** the package
/// directory is gone (uninstall/rollback/companion teardown) must NOT use
/// this normalized form — with the dir absent the fallback can be hijacked
/// by a foreign pack's `companion_skills` claim or physical nesting, and the
/// removal erases the foreign pack's consent rows. Those writers snapshot
/// the owner with [`resolve_pack_owner_id`] before the deletion (or pass the
/// pack id they already hold) and call
/// [`remove_bundle_from_disabled_scopes_exact`].
pub fn remove_bundle_from_disabled_scopes(raw_id: &str) -> Result<(), String> {
    let package_id = to_package_id(raw_id);
    remove_bundle_from_disabled_scopes_exact(&package_id)
}

/// [`remove_bundle_from_disabled_scopes`] for a caller that already holds the
/// **package id** — no re-normalization, on **either** side (round-26 MAJOR 1,
/// review #455). Post-teardown cleanup writers use this: the normalized
/// load would re-own a dir-absent id onto a foreign pack's claim/nesting
/// *and persist the remap before the removal even runs*, so the removal
/// would erase the foreign pack's rows while the stale rows survive as the
/// foreign owner. This variant reads the raw file (no migration, no
/// normalization, no freeze — a missing file is nothing to clean and a
/// corrupt file is left for the regular read path's fail-closed recovery)
/// and removes exactly the caller-resolved owner's rows from the three sets.
pub fn remove_bundle_from_disabled_scopes_exact(package_id: &str) -> Result<(), String> {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = disabled_bundles_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        // Nothing was ever stored for cleanup.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "reading disabled_bundles.json for the exact cleanup: {error}"
            ));
        }
    };
    let mut file: DisabledBundlesFile = match serde_json::from_str(&content) {
        Ok(file) => file,
        // Skip the cleanup on a corrupt store: the regular read path owns the
        // fail-closed recovery (quarantine + DenyAll fallback), and blindly
        // overwriting from here could race or bypass the quarantine. The
        // leftover rows are the stale-deny direction (fail-safe).
        Err(error) => {
            return Err(format!(
                "disabled_bundles.json unparseable — skipping the exact cleanup (recovery is owned by the regular read path): {error}"
            ));
        }
    };
    let mut changed = false;
    for ids in file.scopes.values_mut() {
        let before = ids.len();
        ids.retain(|id| id != package_id);
        changed |= ids.len() != before;
    }
    // 可见性集同样清理：卸载后残留 hidden 会误隐藏未来同名重装。
    for ids in file.hidden_scopes.values_mut() {
        let before = ids.len();
        ids.retain(|id| id != package_id);
        changed |= ids.len() != before;
    }
    // The marker must go with the entry (round-12 self-review): a stale
    // install-default marker would later let a welcome/scene opt-in lift a
    // *user* off that re-added the same id (uninstall or logout clears the
    // stored entry, then the user switches the connector off again).
    for defaults in file.default_off_scopes.values_mut() {
        let before = defaults.len();
        defaults.retain(|id| id != package_id);
        changed |= defaults.len() != before;
    }
    // Round-31 BLOCKER (review #455): teardown also clears the pack's
    // install-default sync ledger entries, so a fresh install / reconnect
    // re-syncs default-off. This is the TEARDOWN-only hook: the composer
    // enable path rewrites the scope lists wholesale and deliberately does
    // NOT come through here — a user enable must keep the ledger entry
    // (that is what makes the enable sticky against the startup refresh).
    // Round-32 minor 1 (review #455): the entries are `"<scope>:<pack>"`, so
    // the clear matches them EXACTLY per scope key — a suffix match would let
    // a pack id whose tail equals another pack id (`a:b` vs `b`) lose its
    // ledger entry on the other pack's teardown (spurious default-off re-push
    // at the next refresh).
    let mut ledger_keys: Vec<String> = Vec::new();
    for mode in SessionMode::ALL {
        ledger_keys.push(format!("{}:{package_id}", mode.as_str()));
    }
    let before_ledger = file.install_default_synced.len();
    file.install_default_synced
        .retain(|entry| !ledger_keys.contains(entry));
    changed |= file.install_default_synced.len() != before_ledger;
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
    /// Round-26 minor 1 (review #455): whether this call actually mutated the
    /// persisted file — the disabled/default-off lists, the materialized
    /// snapshot, or the hidden set. A mixed batch (some ids applied, some
    /// `not_applied`) and a hidden-only un-hide both set this true, so the
    /// caller's hot-refresh gate cannot skip a refresh that live sessions
    /// need. Independent of `blocked`/`not_applied`, which describe the
    /// per-id outcome, not the persisted-state delta.
    pub state_changed: bool,
}

pub fn enable_packages_in_scope(
    scope: ConnectorScope,
    raw_ids: &[String],
) -> Result<EnablePackagesOutcome, String> {
    // Round-23 MINOR 3 hoist: one manifest walk for the whole list.
    let tools = MarketplaceManager::new().available_tools();
    let mut ids: Vec<String> = raw_ids
        .iter()
        .map(|id| to_package_id_with(&tools, id))
        .collect();
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
                state_changed: false,
            });
        }
    }
    let mut not_applied: Vec<String> = Vec::new();
    let mut changed = false;
    let mut applied: Vec<String> = Vec::new();
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
        applied = ids.clone();
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
            applied = ids
                .iter()
                .filter(|id| !not_applied_ids.contains(id))
                .cloned()
                .collect();
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
    // Round-37 C1 (review #455): an explicit enable also RECORDS the ledger
    // pair. The connect/install sync normally wrote it first (the
    // uninitialized arm records without pushing a row); recording here closes
    // the surviving window where that sync's persist FAILED between connect
    // and enable — the row is absent and the pair unledgered, so the next
    // boot's refresh classified the enable as never-synced and reverted it
    // (surfaced only as a startup timeline mark). A ledger entry written by
    // the enable is the same fact the refresh gates on — "this pair was
    // settled", whatever wrote it. Only APPLIED ids are recorded: a
    // not_applied id was never enabled, and ledgering it would suppress the
    // legitimate backfill of its default-off row.
    for id in &applied {
        let ledger_key = format!("{key}:{id}");
        if !file.install_default_synced.contains(&ledger_key) {
            file.install_default_synced.push(ledger_key);
            changed = true;
        }
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
        state_changed: changed,
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
pub fn apply_restore_consent_gate(pack_id: &str, skill_ids: &[String]) -> Result<(), String> {
    apply_restore_consent_gate_impl(pack_id, skill_ids, false)
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
pub fn apply_restore_consent_gate_secrets_pack(
    pack_id: &str,
    skill_ids: &[String],
) -> Result<(), String> {
    apply_restore_consent_gate_impl(pack_id, skill_ids, true)
}

fn apply_restore_consent_gate_impl(
    pack_id: &str,
    skill_ids: &[String],
    force_uninitialized: bool,
) -> Result<(), String> {
    // Round-23 MINOR 3 hoist: one manifest walk for the whole list.
    let tools = MarketplaceManager::new().available_tools();
    let mut ids: Vec<String> = skill_ids
        .iter()
        .map(|id| to_package_id_with(&tools, id))
        .collect();
    // Round-24 MAJOR 1: the pack's own row is never re-owned. At gate time the
    // pack dir is still in the recycle bin, so the known-pack shield cannot see
    // it, and a foreign installed pack claiming the id as a companion skill (or
    // physically nesting `skills/<pkg_id>/`) would remap it onto that pack —
    // the deny row lands on the wrong id and the restored pack goes live with
    // zero consent once the restore completes and its own claims resolve. The
    // pack id lands verbatim, like the round-23 MINOR 1 shield guarantees for
    // physically present packs.
    if !ids.iter().any(|id| id == pack_id) {
        ids.push(pack_id.to_string());
    }
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

/// Writes the project-level skills toggle. After persisting, the caller rewrites
/// the online session composed catalogs. Write failures propagate unchanged
/// (user governance state must not be silently lost — same principle as the
/// toggle/visibility writes).
pub fn set_project_skills_enabled(enabled: bool) -> Result<(), String> {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_bundles_file_locked();
    if file.project_skills_enabled == enabled {
        return Ok(());
    }
    file.project_skills_enabled = enabled;
    try_save_disabled_bundles_file(&file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::test_support::{make_dir_unreadable_for_test, with_temp_home};

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
                        .map(|n| n.contains(".unreadable."))
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
                        .map(|n| n.contains(".unreadable."))
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
                        .map(|n| n.contains(".unreadable."))
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

    /// Round-29 m1 (review #455): a corrupt store whose QUARANTINE write
    /// fails leaves no preserved copy, and only the failing read skips the
    /// overwrite — without a marker, a later load-then-save writer
    /// blind-writes over the still-unquarantined bytes (differential
    /// transient failure: the quarantine write fails now, the main write
    /// succeeds later). The failing read must arm the unreadable-original
    /// marker so the next persist renames the corrupt original aside instead
    /// of destroying the only copy.
    #[cfg(unix)]
    #[test]
    fn quarantine_failure_arms_marker_so_later_writer_preserves_corrupt_bytes() {
        use std::os::unix::fs::PermissionsExt;
        with_temp_home("pinvou3-scope", || {
            let home = paths::pinvou3_home();
            let path = disabled_bundles_path();
            let original = b"corrupt-user-optouts{{{".to_vec();
            std::fs::write(&path, &original).unwrap();

            // Read-only DIRECTORY (0o555): the file itself stays readable, so
            // the read reaches the parse-corrupt branch, but the quarantine
            // copy cannot land (it needs directory write permission).
            std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o555)).unwrap();
            let probe = home.join(".quarantine-fail-probe");
            if std::fs::write(&probe, b"probe").is_ok() {
                std::fs::remove_file(&probe).ok();
                std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
                eprintln!(
                    "ROOT-SKIP[quarantine_failure_arms_marker_so_later_writer_preserves_corrupt_bytes]: read-only-dir fixture not effective (root); NOT exercised"
                );
                return;
            }

            let recovered = load_disabled_bundles_file();
            assert!(
                recovered.plain_defaults_migrated,
                "the corrupt read degrades fail-closed: {recovered:?}"
            );
            assert_eq!(
                std::fs::read(&path).unwrap(),
                original,
                "the failing read must not touch the unquarantined original"
            );
            let siblings: Vec<std::path::PathBuf> = std::fs::read_dir(&home)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.contains(".corrupt.") || n.contains(".unreadable."))
                        .unwrap_or(false)
                })
                .collect();
            assert!(
                siblings.is_empty(),
                "the failed quarantine left no preserved copy: {siblings:?}"
            );

            // The differential: the transient failure heals, and a writer
            // holding the in-memory recovered state persists it. The marker
            // armed by the failed quarantine must rename the corrupt original
            // aside instead of blind-writing over the only copy.
            std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
            try_save_disabled_bundles_file(&recovered).unwrap();

            let sidecars: Vec<std::path::PathBuf> = std::fs::read_dir(&home)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.contains(".unreadable."))
                        .unwrap_or(false)
                })
                .collect();
            assert_eq!(
                sidecars.len(),
                1,
                "the writer preserved the unquarantined original: {sidecars:?}"
            );
            assert_eq!(
                std::fs::read(&sidecars[0]).unwrap(),
                original,
                "the preserved copy must carry the original corrupt bytes"
            );
            let on_disk: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&path).expect("the persist must land"),
            )
            .expect("the persisted store must be valid JSON");
            assert_eq!(
                on_disk.get("plain_defaults_migrated"),
                Some(&serde_json::Value::Bool(true)),
                "the writer's recovered state is the store again: {on_disk}"
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

    /// Round-24 minor 1: the NotFound sidecar-evidence rule covers BOTH
    /// namespaces — quarantine's `.corrupt.<ts>` AND the rename-aside
    /// `.unreadable.<ts>` half. This twin of
    /// `not_found_read_with_corrupt_sidecar_fails_closed` seeds only an
    /// `.unreadable.<ts>` sibling: a regression to corrupt-only prefixes
    /// would stay green while reopening the round-20 MAJOR-B crash window
    /// (the wide upgrade signal judging a lost store as fresh).
    #[test]
    fn not_found_read_with_unreadable_sidecar_fails_closed() {
        with_temp_home("pinvou3-scope", || {
            let home = paths::pinvou3_home();
            // Both wide-signal legs present: without the sibling-evidence arm
            // this home judges "upgraded" and initializes plain empty.
            let installed = home.join("marketplace").join("installed.json");
            std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
            std::fs::write(&installed, "[\"weather\"]").unwrap();
            std::fs::create_dir_all(home.join("sessions").join("default")).unwrap();
            std::fs::write(home.join("sessions").join("default").join("t.json"), "{}").unwrap();
            std::fs::write(home.join("disabled_bundles.json.unreadable.456"), b"lost").unwrap();

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
            // baseline first.
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
            // Round-21 minor 2: assert BOTH halves — a mutation that cleared
            // the marker but leaked the stored entry must fail here.
            assert!(
                file.default_off_scopes
                    .get("plain")
                    .map(|d| d.is_empty())
                    .unwrap_or(true),
                "the marker goes with the stored entry: {file:?}"
            );
            assert!(
                !file
                    .scopes
                    .values()
                    .any(|ids| ids.iter().any(|id| id == "pptx")),
                "the stored entry itself must be removed from every scope: {file:?}"
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

    /// Round-28 MINOR 3 (review #455): `state_changed` is the load-bearing
    /// hot-refresh gate in `enable_marketplace_packages` — pin it at the
    /// domain level. A regression flipping the command gate back to the IPC
    /// `enabled` flag (the round-26 minor-1 bug class) or dropping a
    /// `changed |=` leg fails here.
    #[test]
    fn enable_packages_state_changed_tracks_persisted_delta() {
        with_temp_home("pinvou3-scope", || {
            // Initialized scope, hidden-only un-hide: X sits in the hidden
            // set only — every disabled/default list is untouched but the
            // hidden leg persists a visibility change.
            save_hidden_bundles_for(ConnectorScope::Plain, &["hidden-only".to_string()]).unwrap();
            save_disabled_bundles_for(ConnectorScope::Plain, &[]).unwrap();
            let outcome =
                enable_packages_in_scope(ConnectorScope::Plain, &["hidden-only".to_string()])
                    .unwrap();
            assert!(
                outcome.state_changed,
                "hidden-only un-hide persists state: {outcome:?}"
            );

            // Applied batch: "a" carries an install-default marker
            // (enableable); "b" is an explicit opt-out without one. Plant the
            // raw file — the composer write path would seed no markers for
            // entries it transitioned.
            let path = disabled_bundles_path();
            std::fs::write(
                &path,
                r#"{"scopes":{"plain":["a","b"]},"default_off_scopes":{"plain":["a"]},"initialized":["plain"],"plain_defaults_migrated":true}"#,
            )
            .unwrap();
            let outcome =
                enable_packages_in_scope(ConnectorScope::Plain, &["a".to_string()]).unwrap();
            assert!(
                outcome.state_changed && outcome.not_applied.is_empty(),
                "enableable applied batch persists state: {outcome:?}"
            );

            // Refused batch (explicit user opt-out "b"): nothing changes.
            let outcome =
                enable_packages_in_scope(ConnectorScope::Plain, &["b".to_string()]).unwrap();
            assert!(
                !outcome.state_changed && !outcome.blocked.is_empty(),
                "refused batch changes nothing: {outcome:?}"
            );

            // Uninitialized DenyAll scope, no id matches the expansion:
            // not_applied-only, nothing persisted.
            let outcome =
                enable_packages_in_scope(ConnectorScope::Code, &["no-such-pack".to_string()])
                    .unwrap();
            assert!(
                !outcome.state_changed && !outcome.not_applied.is_empty(),
                "not_applied-only batch persists nothing: {outcome:?}"
            );
        });
    }

    /// Round-26 MAJOR 1 (review #455): post-teardown cleanup writers remove
    /// rows by the **pre-teardown owner** (exact form). With the victim
    /// pack's dir deleted and nothing in the bin, the normalized form's
    /// gating fallback re-owns the absent id onto a foreign pack that claims
    /// it (`companion_skills`) or physically nests it — the removal would
    /// then erase the FOREIGN pack's consent rows while the stale victim
    /// rows survive (silent zero-consent re-enable). The exact form targets
    /// only the victim's rows and leaves the foreign pack untouched.
    #[test]
    fn exact_cleanup_never_reowns_absent_dir_id_onto_foreign_claim() {
        with_temp_home("pinvou3-scope", || {
            // Foreign installed pack claiming `victim` as a companion skill
            // AND physically nesting skills/victim/ (both remap legs).
            let shadow = paths::bundles_root().join("shadow-pack");
            std::fs::create_dir_all(shadow.join("mcp")).unwrap();
            std::fs::write(
                shadow.join("mcp").join("manifest.json"),
                r#"{"id":"shadow-pack","name":"shadow","description":"d","version":"1","icon":"x","category":"c","mcp_tools":[],"command":"python","args":["s.py"],"companion_skills":["victim"]}"#,
            )
            .unwrap();
            std::fs::create_dir_all(shadow.join("skills/victim")).unwrap();
            std::fs::create_dir_all(paths::pinvou3_home().join("marketplace")).unwrap();
            std::fs::write(
                paths::pinvou3_home()
                    .join("marketplace")
                    .join("installed.json"),
                serde_json::json!(["shadow-pack"]).to_string(),
            )
            .unwrap();
            // The victim pack is deleted-and-unbinned: no `bundles/victim/`,
            // no bin entry. The fallback does hijack the id in this state —
            // that is exactly why the writers must not re-normalize here.
            assert_eq!(
                resolve_pack_owner_id("victim"),
                "shadow-pack",
                "precondition: the absent-dir id is hijackable by the foreign claim"
            );

            // Verbatim rows as the writers would have inherited them (the
            // write paths normalize, so plant the file directly).
            let path = disabled_bundles_path();
            std::fs::write(
                &path,
                r#"{"scopes":{"plain":["victim","shadow-pack"]},"hidden_scopes":{"plain":["victim","shadow-pack"]},"default_off_scopes":{"plain":["victim"]},"initialized":["plain"],"plain_defaults_migrated":true}"#,
            )
            .unwrap();

            remove_bundle_from_disabled_scopes_exact("victim").unwrap();
            let file = load_disabled_bundles_file();
            assert_eq!(
                file.scopes.get("plain").map(|v| v.as_slice()),
                Some(&["shadow-pack".to_string()][..]),
                "the victim row goes, the foreign pack's row stays: {file:?}"
            );
            assert_eq!(
                file.hidden_scopes.get("plain").map(|v| v.as_slice()),
                Some(&["shadow-pack".to_string()][..]),
                "the hidden leg obeys the same ownership: {file:?}"
            );
            assert!(
                file.default_off_scopes
                    .get("plain")
                    .map(|d| d.is_empty())
                    .unwrap_or(true),
                "the marker goes with the victim entry only: {file:?}"
            );
        });
    }

    /// Round-32 minor 1 (review #455): the teardown ledger-clear matches the
    /// `"<scope>:<pack>"` entries EXACTLY per scope key, not by suffix — a
    /// pack id containing `:` whose tail equals another pack id must keep its
    /// own ledger entry when the other pack is torn down (a suffix match
    /// would clear it and the next refresh would re-push a spurious
    /// default-off row for it).
    #[test]
    fn teardown_ledger_clear_is_exact_not_suffix_based() {
        use crate::platform::test_support::with_temp_home;

        with_temp_home("pinvou3-scope-ledger-exact", || {
            let path = disabled_bundles_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                r#"{"scopes":{},"install_default_synced":["plain:good","plain:evil:good"]}"#,
            )
            .unwrap();

            remove_bundle_from_disabled_scopes_exact("good").unwrap();
            let file = load_disabled_bundles_file();
            assert_eq!(
                file.install_default_synced,
                vec!["plain:evil:good".to_string()],
                "tearing down `good` must not clear `evil:good`'s ledger entry"
            );
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

            remove_bundle_from_disabled_scopes("weather").unwrap();

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
            // Byte-level on-disk assertion (review #455 R6-m2): normalization
            // must not only fix the gating criteria but also persist —
            // otherwise raw entries survive uninstalling the claim owner and
            // revive on reinstall.
            let persisted = std::fs::read_to_string(disabled_bundles_path()).unwrap();
            assert!(
                persisted.contains("\"gongwen\"") && !persisted.contains("government-writing"),
                "the normalized result must be persisted over the raw entries: {persisted}"
            );
        });
    }

    /// Builtin plugins are exempt from the post-install DenyAll sync: a
    /// direct-IPC reinstall of an already installed builtin must not seed it
    /// into the disabled set of initialized scopes (it could never be
    /// removed again through the guarded write paths).
    #[test]
    fn deny_all_sync_skips_builtin_packages() {
        with_temp_home("pinvou3-scope", || {
            save_disabled_bundles_for(ConnectorScope::Code, &["weather".to_string()]).unwrap();
            sync_deny_all_scopes_after_install("session-reader").unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["weather".to_string()],
                "builtin id must not be synced into the initialized DenyAll scope"
            );
            // A normal package is still synced in.
            sync_deny_all_scopes_after_install("pptx").unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["weather".to_string(), "pptx".to_string()]
            );
        });
    }

    /// Legacy migration never persists builtin ids: entries normalizing to a
    /// builtin package (including the `skill:`-prefixed alias) are skipped,
    /// not written.
    #[test]
    fn legacy_migration_skips_builtin_ids() {
        with_temp_home("pinvou3-scope", || {
            let conn = paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(
                &conn,
                r#"["weather", "skill:session-reader", "session-reader"]"#,
            )
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
        });
    }

    /// Same builtin-skip migration for the legacy `{scopes, initialized}`
    /// object shape (the bare-array branch is covered above).
    #[test]
    fn legacy_migration_scopes_object_skips_builtin_ids() {
        with_temp_home("pinvou3-scope", || {
            let conn = paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(
                &conn,
                r#"{"scopes":{"plain":["session-reader","weather"],"code":["skill:session-reader"]},"initialized":["plain","code"]}"#,
            )
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()],
                "the builtin id must be dropped from the migrated plain scope"
            );
            // The code scope contained only the poisoned entry: after the
            // skip nothing is migrated for it, and as an initialized scope it
            // reads back empty (its persisted list wins over the default).
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                Vec::<String>::new(),
                "an initialized scope whose only entry was poisoned migrates to empty"
            );
        });
    }

    /// Same builtin-skip migration for the oldest legacy shape
    /// `{plain, code, code_initialized}`.
    #[test]
    fn legacy_migration_dual_scope_object_skips_builtin_ids() {
        with_temp_home("pinvou3-scope", || {
            let conn = paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(
                &conn,
                r#"{"plain":["session-reader","weather"],"code_initialized":true}"#,
            )
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()],
                "the builtin id must be dropped from the migrated plain scope"
            );
        });
    }

    /// Live read path self-heal: a builtin id that already landed in
    /// `disabled_bundles.json` (legacy bug window / hand edit) is dropped at
    /// read/normalize time, so the frontend's next full-list write no longer
    /// trips `reject_builtin_ids` — the poisoned entry cannot brick every
    /// toggle of the scope.
    #[test]
    fn live_read_path_drops_poisoned_builtin_ids() {
        with_temp_home("pinvou3-scope", || {
            let path = paths::pinvou3_home().join("disabled_bundles.json");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                r#"{"scopes":{"plain":["session-reader","weather"]},"hidden_scopes":{"plain":["session-reader","pptx"]},"initialized":["plain"]}"#,
            )
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()],
                "the poisoned builtin id must be dropped from the disabled set"
            );
            assert_eq!(
                load_hidden_bundles_for(ConnectorScope::Plain),
                vec!["pptx".to_string()],
                "the poisoned builtin id must be dropped from the hidden set"
            );
            // Recovery: the cleaned list the frontend now holds passes the
            // builtin guard on the next full-list write.
            save_disabled_bundles_for(
                ConnectorScope::Plain,
                &["weather".to_string(), "pptx".to_string()],
            )
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string(), "pptx".to_string()]
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

    /// Round-29 m3 (review #455): the expansion disk leg's staging exclusion
    /// is consent-load-bearing — crash-residue `<id>.tmp`/`<id>.old` dirs
    /// nesting skills must not join the DenyAll expansion (a suffix-owner id
    /// would then be persisted by the composer's first-write seeding as a
    /// stored row + install-default marker).
    #[test]
    fn denyall_disk_leg_skips_staging_residue() {
        with_temp_home("pinvou3-scope-denyall", || {
            let bundles = paths::bundles_root();
            for residue in ["stage-pack.tmp", "stage-pack.old"] {
                let dir = bundles.join(residue).join("skills").join("leak");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("SKILL.md"), "# leak").unwrap();
            }
            // Round-33 merge note (#585): builtins are feature-switch-governed —
            // the computed default drops ids that normalize to a builtin, so the
            // pin is twofold: no staging-residue suffix-owner junk joins the
            // expansion, and no builtin id does either.
            let default_ids = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(
                !default_ids
                    .iter()
                    .any(|id| id.starts_with("stage-pack.") || id == "leak"),
                "staging residue must not join the expansion (no suffix-owner junk, no leak owner): {default_ids:?}"
            );
            assert!(
                !default_ids.iter().any(|id| {
                    crate::features::marketplace::builtin::is_builtin_tool(&to_package_id(id))
                }),
                "builtins are feature-switch-governed, never package-governed: {default_ids:?}"
            );
        });
    }

    /// Round-9 review: the disable writer itself must reject builtin ids, not
    /// only the `apply_disabled_connectors_for` layer above it — the function
    /// is pub-re-exported, and a future direct caller would otherwise
    /// silently seed a builtin into an initialized scope. Same layering as
    /// the hide writer (`save_hidden_bundles_for`), which already guards
    /// inside the persistence function.
    #[test]
    fn save_disabled_bundles_for_rejects_builtin_ids_directly() {
        with_temp_home("pinvou3-scope-disable-guard", || {
            crate::platform::paths::ensure_dirs().unwrap();
            for builtin_id in ["session-reader", "Session-Reader"] {
                let error =
                    save_disabled_bundles_for(ConnectorScope::Plain, &[builtin_id.to_string()])
                        .expect_err("a direct disable write must refuse builtin ids");
                assert!(
                    error.contains(builtin_id),
                    "the refusal must name the rejected id: {error}"
                );
            }
            // A clean write still initializes the scope normally.
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
        });
    }

    /// Round-6 B1: the boot seed installs the session-reader builtin into
    /// installed.json before the first scope read. The DenyAll computed
    /// default must drop ids that normalize to builtins — a default
    /// containing the seed would make the echoed full-set write that
    /// initializes the scope fail `reject_builtin_ids`, so every composer
    /// toggle on an uninitialized DenyAll profile errored forever with no
    /// escape (uninstall is guarded, the builtin page is read-only).
    #[test]
    fn denyall_computed_default_excludes_builtins_and_roundtrips_a_toggle() {
        with_temp_home("pinvou3-scope-denyall-builtin", || {
            crate::platform::paths::ensure_dirs().unwrap();
            // The exact startup order that produced the deadlock: seed first,
            // then read an uninitialized scope.
            crate::features::marketplace::MarketplaceManager::new()
                .ensure_default_installed_mcp_tools();
            let default_set = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(
                !default_set.iter().any(|id| {
                    crate::features::marketplace::builtin::is_builtin_tool(&to_package_id(id))
                }),
                "computed default must not contain builtin ids: {default_set:?}"
            );
            assert!(
                !default_set.contains(&"session-reader".to_string()),
                "the boot-seeded builtin must not appear in the default: {default_set:?}"
            );

            // Round-trip the composer shape: the UI reads the full effective
            // set and echoes it back on any toggle — that write passes the
            // builtin guard and initializes the scope (previously it was the
            // very write that was rejected).
            let echoed = load_disabled_bundles_for(ConnectorScope::Code);
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime
                .block_on(crate::features::marketplace::apply_disabled_connectors_for(
                    ConnectorScope::Code,
                    echoed,
                ))
                .expect("echoing the computed default back must pass the builtin guard");
            // Initialized: the persisted list wins from here on, and a later
            // read is stable (idempotent toggle behavior).
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                load_disabled_bundles_for(ConnectorScope::Code),
            );
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
                // Round-26 minor 9 (review #455): an ineffective fixture (root,
                // Windows) previously skipped silently — the pin then passed
                // vacuously. Loud ROOT-SKIP, per the round-11 m12 convention.
                eprintln!(
                    "ROOT-SKIP[denyall_default_degraded_scan_biases_to_overdeny]: unreadable-dir fixture not effective (root or non-unix); NOT exercised"
                );
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
                // Round-26 minor 9 (review #455): an ineffective fixture (root,
                // Windows) previously skipped silently — the pin then passed
                // vacuously. Loud ROOT-SKIP, per the round-11 m12 convention.
                eprintln!(
                    "ROOT-SKIP[denyall_default_degraded_upload_scan_biases_to_overdeny]: unreadable-dir fixture not effective (root or non-unix); NOT exercised"
                );
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
                // Round-26 minor 9 (review #455): an ineffective fixture (root,
                // Windows) previously skipped silently — the pin then passed
                // vacuously. Loud ROOT-SKIP, per the round-11 m12 convention.
                eprintln!(
                    "ROOT-SKIP[denyall_default_packages_root_failure_biases_to_overdeny]: unreadable-dir fixture not effective (root or non-unix); NOT exercised"
                );
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

    /// #531 boundary: an initialized scope sticks to its persisted list; skill
    /// enumeration degradation must not affect it (the over-denial fallback
    /// only applies to the freshly computed default of an uninitialized scope).
    #[test]
    fn initialized_scope_ignores_degraded_skill_scan() {
        with_temp_home("pinvou3-scope-initialized", || {
            save_disabled_bundles_for(ConnectorScope::Code, &["weather".to_string()]).unwrap();
            let skill_dir = install_preset_skill_under_claimed_owner();
            let Some(_unreadable) = make_dir_unreadable_for_test(&skill_dir) else {
                // Round-26 minor 9 (review #455): an ineffective fixture (root,
                // Windows) previously skipped silently — the pin then passed
                // vacuously. Loud ROOT-SKIP, per the round-11 m12 convention.
                eprintln!(
                    "ROOT-SKIP[initialized_scope_ignores_degraded_skill_scan]: unreadable-dir fixture not effective (root or non-unix); NOT exercised"
                );
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
