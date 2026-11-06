//! Per-session runtime mode state machine + 类型定义。
//!
//! `SessionModeState` 等 session 域聚合类型（persona/knowledge/multi-agent 等
//! 特性字段）原先在 `core/mode_state.rs`，违反 `core/README.md` 的"feature 内部
//! 类型不入 core"准则，Wave 3 将其定义迁回此文件（行为 impl 一直在此），并由
//! `features::sessions` 正式 re-export，消费方一律从
//! `crate::features::sessions::{...}` 导入。跨层协议类型仍定义在
//! `core/mode_state.rs`：其中 `SerializableMode` 经 `features::sessions`
//! re-export（`mod.rs` 的 `pub use`），`ModeLane`/`ModeDefaultsView` 由消费方
//! 直接从 `crate::core::mode_state` 导入，不经 features 转出。
//!
//! These methods drive the `mode_states` map (mode, pending Plan ticket
//! + claim-in-flight, persona, mounted collection). The durable subset:
//! the per-session MODE is persisted to the `_session_mode_states.json`
//! sidecar (with the legacy `_code_mode_states.json` migrated on load)
//! and `_multi_agent.json` carries the multi-agent flags — the mode and
//! both multi-agent flags survive a restart, and reopening a session
//! restores its own last mode. What stays in memory is interaction state
//! that must reset on a bounce: the pending Plan ticket and its
//! claim-in-flight, persona, and the mounted knowledge collection
//! (model selections are persisted in their own sidecars — see
//! [`super::sidecars`], which lists all five durable sidecar files).

use serde::{Deserialize, Serialize};

use anyhow::{Context, Result, bail};

use crate::core::mode_state::{ModeDefaultsView, ModeLane, SerializableMode};

use super::SessionStore;
use super::injections::{PendingPlanClaim, PendingTurnInjections};
use crate::platform::prefs::{CodePermissionPrefs, UserPrefs};
use std::collections::HashMap;
use std::io::ErrorKind;

/// Test-only crash seam for the shared→companion sidecar write pair
/// (round-9 minor 1, mirroring the rebind pair's
/// `inject_rebind_crash_after_legacy_rewrite`): armed by
/// [`SessionStore::inject_mode_pair_crash_after_shared_write`], consumed
/// once by `apply_session_mode_mutation_locked` right after the shared
/// write lands, simulating a process crash in the load-bearing window.
/// Production never arms it, so the pair is a no-op there.
#[cfg(test)]
static MODE_PAIR_CRASH_AFTER_SHARED_WRITE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

// ── 类型定义（session 域聚合，由 sessions 正式 re-export） ──

/// A knowledge collection mounted into a session with an enabled flag, so the
/// UI can toggle a mount on/off without losing its position in the ordered
/// list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MountedCollection {
    pub collection_id: i64,
    pub enabled: bool,
}

/// 会话挂载的远程知识集。服务器 ID 与服务端集合 ID 共同构成稳定身份。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MountedRemoteCollection {
    pub server_id: String,
    pub collection_id: i64,
    pub enabled: bool,
}

/// Revisioned snapshot of every mounted collection for one session. The
/// revision bumps on any mutation so the Tauri boundary can publish a single
/// revisioned event that lets concurrent clients reconcile out-of-order
/// updates.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MountedCollectionsSnapshot {
    pub revision: u64,
    pub collections: Vec<MountedCollection>,
}

/// 单 session 的 mode 状态。前端通过 `get_mode_state` 拉取，
/// `set_plan_mode_next` / `accept_plan` 等命令修改。
///
/// 底座 `AppMode` 非 Serialize，经 `SerializableMode` 镜像单向序列化给前端;
/// SessionModeState 不需要从前端 deserialize 回来(它通过 set_*_state 命令逐字段写)。
#[derive(Debug, Clone, Serialize)]
pub struct SessionModeState {
    /// Side B 卡片池:当前加持的专家卡 id(远端 persona 体系)。
    #[serde(default)]
    pub active_persona: Option<String>,
    /// Side B:待一次性注入的人设 body。
    #[serde(default, skip)]
    pub pending_persona_body: Option<String>,
    /// 当前激活 mode。`build_send_message_op` 用这个值。
    pub mode: SerializableMode,
    /// 会话级"守卫命中时询问"开关（tri-mode 安全模型）。开启时 execpolicy
    /// 拒绝面 / 非旁路 holds / typed ask 命中会升级为审批卡（Ask 姿态天然
    /// 弹卡），关闭时按姿态硬拒。默认取全局 `ask_on_guardrail_default`
    /// （true），新会话重置。独立翻转对**无 mode 条目**的会话同样持久：
    /// `set_ask_on_guardrail` 会把显式覆盖写进 `_ask_on_guardrail.json`
    /// 侧车（不铸造 mode 条目），boot 时与内存覆盖镜像合并生效——
    /// "只有存在 mode 条目的会话才有独立开关"是 PR 前语义，已被
    /// `ask_on_guardrail_toggle_persists_for_entry_less_sessions` 钉死的
    /// 现行为取代（round-8 L2-4 文档修正）。
    #[serde(default = "default_true")]
    pub ask_on_guardrail: bool,
    /// accept_plan 后回到的 mode（进入 Plan 前的 mode）。仅驻内存：重启后
    /// Plan 会话经 `resolved_default_mode` 解析，不钉进程相对值（见
    /// `durable_mode_entry` 的理由）。`None` = 未进入过 Plan；accept 回退到
    /// Auto（新工作车道默认）。
    #[serde(skip)]
    pub(crate) pre_plan_mode: Option<SerializableMode>,
    /// 当前可操作方案的服务端 ticket。新 plan_ready 会替换旧 ticket；
    /// accept/discard 必须 compare-and-consume，防多端旧卡重复执行。
    #[serde(default)]
    pub pending_plan_id: Option<String>,
    /// accept 已原子 claim、但 Engine mailbox 尚未确认的 ticket。
    /// 仅用于进程内失败回滚，不暴露给前端。
    #[serde(skip)]
    pub(crate) plan_claim_in_flight: Option<String>,
    /// 该 session 挂载的本地知识集 id(会话级粘连)。`None` = 未挂载。
    /// 挂载生效走 Agentic RAG:engine 注入 `kb_search` 工具,由模型自主检索,
    /// 不再按消息自动注入片段(注入式已废弃,见 `commands::chat`)。与
    /// `active_persona` 一样仅驻内存,不落盘——重启 app 后回到未挂载。
    #[serde(default)]
    pub mounted_collection: Option<i64>,
    /// 多知识库挂载事实源。旧单库字段保留给旧前端/远程端兼容读取。
    #[serde(default)]
    pub mounted_collections: Vec<MountedCollection>,
    /// 远程知识集挂载，与本地挂载可以同时启用并由检索工具统一合并。
    #[serde(default)]
    pub mounted_remote_collections: Vec<MountedRemoteCollection>,
    /// 仅驻内存的并发版本号；通过专用 snapshot 命令对外提供，不混入 mode_state 协议。
    #[serde(skip)]
    pub mounted_collections_revision: u64,
    /// 多智能体模式开关（ADR-0006）：模型列表下方的会话级开关。开启后本会话
    /// 装配专家名册：引擎生成时注入一次蜂群契约（委派与否由模型按收益判断），
    /// 每轮仅附专家候选提醒；关闭停止注入并回收引擎，取消仍在后台运行的
    /// 子智能体（工具面不随开关变化，与主线一致）。会话级记忆，经
    /// `sessions/_multi_agent.json` sidecar 持久化，重启不丢。
    #[serde(default)]
    pub multi_agent: bool,
}

fn default_true() -> bool {
    true
}

impl Default for SessionModeState {
    fn default() -> Self {
        Self {
            mode: SerializableMode::Yolo,
            ask_on_guardrail: default_true(),
            pre_plan_mode: None,
            pending_plan_id: None,
            plan_claim_in_flight: None,
            active_persona: None,
            pending_persona_body: None,
            mounted_collection: None,
            mounted_collections: Vec::new(),
            mounted_remote_collections: Vec::new(),
            mounted_collections_revision: 0,
            multi_agent: false,
        }
    }
}

impl SessionModeState {
    /// The execution mode an `accept_plan` on this state would resume into
    /// (`claim_pending_plan`'s restore projection: `pre_plan_mode`, falling
    /// back to Auto — tri-mode safety, "approving a plan approves the work,
    /// not full-auto permission"). Mirrors the claim line exactly, including
    /// the rebooted-Plan fallback (a restored session has `pre_plan_mode =
    /// None` and accepts into Auto). Surfaced on the `get_mode_state` view
    /// so the web face can refuse an accept whose effective execution mode
    /// is Auto BEFORE a turn opens (review zhuowp 5780927743 third: the
    /// plan-mode admission guard only read the current `mode == "plan"` and
    /// waved a card through that then executed under Auto — a posture the
    /// web face cannot serve, since it has no approval-card decision
    /// surface).
    pub fn post_accept_mode(&self) -> SerializableMode {
        if self.mode == SerializableMode::Plan {
            self.pre_plan_mode.unwrap_or(SerializableMode::Auto)
        } else {
            self.mode
        }
    }

    /// A fresh mode-state entry for a session being materialized NOW (the
    /// `mode_state` fallback, `mode_state_entry`, and every `or_insert_with`
    /// mint site). Identical to [`Default::default`] except the per-session
    /// ask-on-guardrail toggle seeds from [`Self::ask_on_guardrail_seed`]
    /// (explicit override first, then the global
    /// `code_permission.ask_on_guardrail_default` preference; tri-mode):
    /// sessions absent from `_ask_on_guardrail.json` follow that global
    /// default — hardcoding `true` here ignored the user's "silent
    /// hard-deny" opt-out until the session got an explicit toggle write.
    /// Explicit per-session overrides keep winning, and an
    /// already-materialized entry is never re-seeded by this ctor.
    ///
    /// The ask-on-guardrail seeding point for fresh entries lives in
    /// [`Self::ask_on_guardrail_seed`] (explicit override first, then the
    /// pref); the runtime mint sites route through that helper, so a pref
    /// change only has one production seeding contract to update.
    /// (`Default::default()` stays for serde round-trips and field-spread
    /// bases only.)
    fn default_for_new_session(ask_on_guardrail_default: bool) -> Self {
        Self {
            ask_on_guardrail: ask_on_guardrail_default,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod type_tests {
    use super::*;
    use deepseek_tui::AppMode;

    #[test]
    fn default_is_yolo() {
        let s = SessionModeState::default();
        assert_eq!(s.mode, SerializableMode::Yolo);
    }

    #[test]
    fn mode_round_trips_to_app_mode() {
        assert!(matches!(
            SerializableMode::Plan.to_app_mode(),
            AppMode::Plan
        ));
        assert!(matches!(
            SerializableMode::Yolo.to_app_mode(),
            AppMode::Agent
        ));
        assert!(matches!(
            SerializableMode::Auto.to_app_mode(),
            AppMode::Agent
        ));
    }

    #[test]
    fn serializes_to_snake_case() {
        let s = SessionModeState {
            mode: SerializableMode::Plan,
            ask_on_guardrail: true,
            pre_plan_mode: None,
            pending_plan_id: Some("plan-1".to_string()),
            plan_claim_in_flight: None,
            active_persona: None,
            pending_persona_body: None,
            mounted_collection: None,
            mounted_collections: Vec::new(),
            mounted_remote_collections: Vec::new(),
            mounted_collections_revision: 0,
            multi_agent: false,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"mode\":\"plan\""));
        assert!(json.contains("\"pending_plan_id\":\"plan-1\""));
        assert!(!json.contains("plan_claim_in_flight"));
    }
}

/// Fold a mode state's collections into one effective list: the multi-mount
/// list is the source of truth; when it is empty, the legacy single-mount
/// field is promoted into a one-element list (old sessions only carry
/// `mounted_collection`).
fn folded_collections(state: &SessionModeState) -> Vec<MountedCollection> {
    if !state.mounted_collections.is_empty() {
        state.mounted_collections.clone()
    } else {
        state
            .mounted_collection
            .map(|collection_id| MountedCollection {
                collection_id,
                enabled: true,
            })
            .into_iter()
            .collect()
    }
}

// ── SessionStore 行为 impl ──

impl SessionStore {
    /// Test-only: arm the crash seam in
    /// [`Self::apply_session_mode_mutation_locked`] so the next mode-pair
    /// persist returns right after the shared `_session_mode_states.json`
    /// write, leaving the companion `_session_auto_modes.json` untouched —
    /// the exact between-writes crash window whose durable residue the
    /// write-order invariant constrains.
    #[cfg(test)]
    pub(crate) fn inject_mode_pair_crash_after_shared_write() {
        MODE_PAIR_CRASH_AFTER_SHARED_WRITE.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    /// The session's entry in the durable `_session_mode_states.json`, if any.
    ///
    /// This — not [`Self::mode_state`]'s resolved fallback — is what a failed
    /// run's mode restore must put back: the fallback is process-relative (a
    /// headless process installs no code-session predicate, so it resolves
    /// Yolo for a code session whose GUI default is Plan), and durably pinning
    /// that misresolution is exactly the unsafe reopen divergence the Plan
    /// persist exists to prevent.
    ///
    /// Reads the FILE, not the boot-time cache. The cache is this process's
    /// view from startup, so against a live GUI it goes stale the moment the
    /// user switches a mode — and the restore would then write that stale
    /// value back over the newer one, or (when the cache never had an entry)
    /// clear a mode the GUI had just set. Both are the divergence above, only
    /// caused by the code meant to prevent it. The cache remains the fallback
    /// for an unreadable file, where a stale answer still beats inventing an
    /// absence.
    pub fn durable_mode_entry(&self, id: &str) -> Option<SerializableMode> {
        // The shared-map + companion read pair must be atomic against the
        // writers: every mutator RMWs the two files under
        // `session_mode_states_io` (`apply_session_mode_mutation_locked`'s
        // caller contract), and this reader is the one consumer that
        // re-persists what it read (headless `PlanModeRestore`). Without the
        // lock a read straddling the Auto upsert's two writes resolves the
        // stale arm, and a failed run then restore-persists the pre-switch
        // mode over the user's fresher choice (round-13 review). Short
        // two-file read; writers already tolerate blocking on this mutex.
        // No internal caller holds it (grep: only the leaf writers do).
        let _io = self.session_mode_states_io.lock();
        // Accessor, not the literal: the quarantine flag compares against
        // `shared_mode_map_file_name()`, so a rename must move both sites
        // together (round-10 review).
        let file = crate::platform::paths::sessions_root()
            .join(crate::features::sessions::sidecars::shared_mode_map_file_name());
        match std::fs::read_to_string(&file) {
            // A parse failure falls through to the cache rather than
            // reporting an absence the file does not actually state.
            Ok(content) => {
                if let Ok(mut entries) = serde_json::from_str::<
                    std::collections::HashMap<String, SerializableMode>,
                >(&content)
                {
                    if let Some(mode) = entries.remove(id) {
                        // Same companion overlay as the boot load: `plan` +
                        // `true` flag reads back as the explicit Auto choice.
                        return Some(Self::resolve_sidecar_mode(
                            mode,
                            &Self::read_session_auto_modes(),
                            id,
                        ));
                    }
                    return None;
                }
            }
            // Absent is authoritative — the mutation path removes the file
            // when its last entry goes away. NOT after a quarantine though
            // (round-9 minor): quarantining a corrupt parse also removes the
            // file, and that absence is unknown state, not a statement any
            // writer made — inventing an absence here lets a headless Plan
            // run "repair" it by clearing the durable mode, a widening when
            // the resolved default is Yolo. Same fault discipline as the
            // durable pin reader: stale cache beats invented absence, and a
            // one-way process-level quarantine flag keeps the conservative
            // answer until the next restart.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if crate::features::sessions::sidecars::shared_mode_map_quarantined() {
                    // fall through to the cache below
                } else {
                    return None;
                }
            }
            Err(_) => {}
        }
        self.session_mode_states.read().get(id).cloned()
    }

    pub fn mode_state(&self, id: &str) -> SessionModeState {
        // Resolved BEFORE the read lock: the fallback entry's ask_on_guardrail
        // seeds through `ask_on_guardrail_seed` (explicit override first —
        // see `default_for_new_session`).
        let fallback = SessionModeState {
            mode: self.resolved_default_mode(id),
            ask_on_guardrail: self.ask_on_guardrail_seed(id),
            ..SessionModeState::default()
        };
        self.mode_states.read().get(id).cloned().unwrap_or(fallback)
    }

    pub(crate) fn is_code_session(&self, id: &str) -> bool {
        self.code_session_predicate
            .read()
            .as_ref()
            .is_some_and(|predicate| predicate(id))
    }

    /// Default mode resolution when no entry exists: code sessions, or plain chat
    /// sessions bound to a user working directory (a real directory means
    /// mis-operation and injection surface; the safety posture follows the
    /// binding, not the mode), fall back to the global `code_permission.last_mode`
    /// (None = the user has never used it → read-only Plan on first use);
    /// unbound plain sessions default to Auto (tri-mode per-call approval).
    /// An explicitly chosen work lane global default (plan/yolo/auto) is
    /// applied by the frontend at session materialization — it becomes the
    /// session's own record, so this resolution only covers the "no explicit
    /// default" case; the backend itself does not distinguish a plain-side
    /// lane (design merged into work, see `set_mode_default`).
    pub(crate) fn resolved_default_mode(&self, id: &str) -> SerializableMode {
        if self.is_code_session(id) || self.session_workspace_binding(id).is_some() {
            let cp = self.code_permission.read();
            // Tri-mode (M2): the explicit-Auto choice lives in the companion
            // flag; the legacy field never carries "auto" (settings.json
            // vocabulary stays parseable by pre-tri-mode binaries).
            if cp.last_mode_auto {
                return SerializableMode::Auto;
            }
            cp.last_mode.unwrap_or(SerializableMode::Plan)
        } else {
            // Tri-mode safety: new unbound plain sessions start in Auto
            // (per-call approval cards, in-workspace writes modal-free), not
            // in full-auto Yolo.
            SerializableMode::Auto
        }
    }

    /// The store's canonical fresh-entry mint for an id-level read-modify-
    /// write: materializes with the caller's resolved default mode and seeds
    /// `ask_on_guardrail` from the global preference (one seeding point —
    /// [`Self::default_for_new_session`]). An existing entry is returned
    /// untouched, so a later preference change never re-arms a session that
    /// already materialized.
    ///
    /// Instance method (not an associated fn) precisely so the seed cannot
    /// drift from `code_permission_prefs()`. Lock discipline, stated as it
    /// actually is (round-8 L2-1; corrected round-11): `clear_mode_and_persist`
    /// / `set_multi_agent` hoist the seed resolution BEFORE their io lock
    /// (predicate + sidecar reason, see their docs); `set_mode` /
    /// `set_mode_and_persist` do not route through this helper — they mint
    /// inline in `set_mode_in_memory` — while THIS helper's callers (the
    /// loaders and `set_multi_agent`) resolve the seed either before or
    /// without the io lock. The seed is safe today only because the
    /// predicate reads in-memory state plus one workspace-sidecar read and
    /// the pref is a plain mirror read, but a predicate that ever re-entered
    /// a mode writer would self-deadlock on the non-reentrant io mutex. Keep
    /// the predicate side-effect-free, or hoist the resolution like the
    /// persist paths.
    pub(crate) fn mode_state_entry<'m>(
        &self,
        states: &'m mut HashMap<String, SessionModeState>,
        id: &str,
        default_mode: SerializableMode,
    ) -> &'m mut SessionModeState {
        states
            .entry(id.to_string())
            .or_insert_with(|| SessionModeState {
                mode: default_mode,
                ask_on_guardrail: self.ask_on_guardrail_seed(id),
                ..SessionModeState::default()
            })
    }

    /// 设置 mode。砍 PlanPhase 后是 Plan/Yolo 唯一 setter(流转命令都调它),
    /// changes only mode, leaving other fields untouched.
    ///
    /// Per-session persistence (two-lane semantics): every session writes
    /// `_session_mode_states.json` (reopening restores its own last mode);
    /// global lane defaults are **no longer** updated here — they are written
    /// only by an explicit draft-state switch via `set_mode_default`.
    /// ACP 会话不经此命令（有自己的权限模式）。落盘失败只记日志不打断交互
    /// ——内存切换已生效，落盘只做尽力持久化。
    pub fn set_mode(&self, id: &str, mode: SerializableMode) -> Result<()> {
        // One critical section for the in-memory switch and the durable RMW,
        // same as set_multi_agent (a concurrent purge or headless Plan
        // persist must not interleave its file RMW with this one).
        let _io = self.session_mode_states_io.lock();
        self.set_mode_in_memory(id, mode.clone());
        if let Err(error) = Self::apply_session_mode_mutation_locked(&[(id.to_string(), mode)], &[])
        {
            // The session id is deliberately kept out of the message, matching
            // persist_accepted_yolo_mode below: boot logs persist to disk and
            // the CodeQL cleartext-logging gate flags ids on stderr.
            eprintln!("[sessions] persist mode failed: {error:#}");
        }
        Ok(())
    }

    /// In-memory half of [`Self::set_mode`]: switches the session's live
    /// state and the durable-read cache (`session_mode_states`) without
    /// touching the sidecar file.
    fn set_mode_in_memory(&self, id: &str, mode: SerializableMode) {
        // Fresh-entry seed resolved BEFORE the write lock (same reason as
        // `clear_mode_and_persist`: the pref/predicate mirror reads must not
        // sit under the mode-states guard). A minted entry resolves its mode
        // from `resolved_default_mode` — the same default every other mint
        // site uses, i.e. the mode the UI actually showed before this write.
        // (Minting the historical `Default` sentinel Yolo here made a
        // first-ever Plan write record `pre_plan_mode = Yolo`, and plan
        // acceptance then landed in persisted full-auto — precisely what the
        // tri-mode accept contract forbids.)
        let resolved_default = self.resolved_default_mode(id);
        let ask_on_guardrail_seed = self.ask_on_guardrail_seed(id);
        {
            let mut m = self.mode_states.write();
            let entry = m.entry(id.to_string()).or_insert_with(|| SessionModeState {
                mode: resolved_default,
                ask_on_guardrail: ask_on_guardrail_seed,
                ..SessionModeState::default()
            });
            // Tri-mode safety: remember the mode Plan was entered from so
            // `accept_plan` returns execution there (fallback Auto). A
            // re-entry into Plan refreshes the record; leaving Plan for
            // anything other than accept leaves the record (harmless —
            // only accept consumes it).
            if mode == SerializableMode::Plan && entry.mode != SerializableMode::Plan {
                entry.pre_plan_mode = Some(entry.mode);
            }
            entry.mode = mode;
            entry.pending_plan_id = None;
            entry.plan_claim_in_flight = None;
        }
        self.session_mode_states
            .write()
            .insert(id.to_string(), mode);
    }

    /// [`Self::set_mode`] for headless runners: additionally requires the
    /// durable `_session_mode_states.json` write to land. A caller that
    /// reports success while the mode would not survive the process hands
    /// back a session that reopens in the stale mode (Plan → Yolo is the
    /// unsafe divergence), so the runner treats a failed persist as a
    /// failed run. The interactive GUI keeps the lenient [`Self::set_mode`]
    /// path, where the in-memory switch already took effect.
    pub fn set_mode_and_persist(&self, id: &str, mode: SerializableMode) -> Result<()> {
        let _io = self.session_mode_states_io.lock();
        self.set_mode_in_memory(id, mode.clone());
        Self::apply_session_mode_mutation_locked(&[(id.to_string(), mode)], &[])
            .context("persist session mode states")
    }

    /// Removes the session's durable mode entry and resets the in-memory
    /// caches to the resolved default. The restore half of a Plan persist
    /// whose run never started on a session that had no durable entry:
    /// re-persisting a resolved default would freeze a value the session
    /// was only borrowing (and in a headless process that resolution can
    /// not even see the GUI's code-session default), so restoring the
    /// pre-run state means removing the entry the failed run added.
    pub fn clear_mode_and_persist(&self, id: &str) -> Result<()> {
        // Resolved BEFORE the io lock: `resolved_default_mode` invokes the
        // host-registered code-session predicate and reads the workspace
        // binding sidecar, so computing it under the mode-states lock would
        // hold that lock across an extension callback and disk I/O — and a
        // predicate that ever touches a mode writer would deadlock on a
        // non-reentrant mutex. The value does not depend on the removal.
        let default = self.resolved_default_mode(id);
        let _io = self.session_mode_states_io.lock();
        Self::apply_session_mode_mutation_locked(&[], &[id.to_string()])
            .context("persist session mode states")?;
        if let Some(entry) = self.mode_states.write().get_mut(id) {
            entry.mode = default.clone();
            entry.pending_plan_id = None;
            entry.plan_claim_in_flight = None;
        }
        self.session_mode_states.write().remove(id);
        Ok(())
    }

    pub fn set_multi_agent(&self, id: &str, enabled: bool) -> Result<()> {
        // Resolved BEFORE the io lock, mirroring `clear_mode_and_persist`:
        // `resolved_default_mode` invokes the host-registered code-session
        // predicate and reads the workspace binding sidecar, so computing it
        // under the multi-agent lock would hold that lock across an extension
        // callback — and a predicate that ever touches a mode or multi-agent
        // writer would deadlock on a non-reentrant mutex.
        let default = self.resolved_default_mode(id);
        let _io = self.multi_agent_flags_io.lock();
        let previous = {
            let mut m = self.mode_states.write();
            // 与本文件其他 setter 同惯例：物化条目时用解析出的默认 mode，
            // 不得 or_default()——那会把从未切换过 mode 的 code 会话从
            // Plan 只读默认静默翻成 Yolo 自动批准。
            let entry = self.mode_state_entry(&mut m, id, default);
            let previous = entry.multi_agent;
            entry.multi_agent = enabled;
            previous
        };
        if let Err(error) = Self::apply_multi_agent_mutation_locked(&[(id, enabled)], &[]) {
            let mut m = self.mode_states.write();
            if let Some(entry) = m.get_mut(id) {
                entry.multi_agent = previous;
            }
            return Err(error).context("persist multi-agent flag");
        }
        Ok(())
    }

    /// Boot-only whole-list rewrite of `_multi_agent.json` from the
    /// multi-agent flag snapshot, used after ghost cleanup re-derives the
    /// full id list from the file just read plus the sessions directory.
    /// Production single-flag mutations must use
    /// `apply_multi_agent_mutation_locked` so entries another process added
    /// after this one booted are preserved instead of reverted by a stale
    /// snapshot.
    ///
    /// Takes the store's `multi_agent_flags_io` mutex itself (the contract
    /// the former `save_multi_agent_flags_locked` left to its caller): the
    /// durable whole-list write must not interleave with a concurrent id-level
    /// read-modify-write, or the older snapshot lands last and resurrects
    /// removed flags.
    pub fn save_multi_agent_flags(&self) -> Result<()> {
        let _io = self.multi_agent_flags_io.lock();
        let file = crate::platform::paths::sessions_root().join("_multi_agent.json");
        let mut ids: Vec<String> = {
            let m = self.mode_states.read();
            m.iter()
                .filter(|(_, state)| state.multi_agent)
                .map(|(id, _)| id.clone())
                .collect()
        };
        ids.sort();
        if ids.is_empty() {
            return match std::fs::remove_file(&file) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error).context("remove _multi_agent.json"),
            };
        }
        let json = serde_json::to_string_pretty(&ids).context("serialize multi-agent flags")?;
        crate::platform::filesystem::atomic_write_private(&file, json.as_bytes())
            .context("persist _multi_agent.json failed")
    }

    /// Apply id-level enable/disable mutations directly to the
    /// `_multi_agent.json` id list, keeping entries written by another
    /// process after this one booted (same rationale as
    /// [`Self::apply_session_mode_mutation`]).
    ///
    /// The `_locked` suffix is the caller contract shared with the boot-time
    /// whole-list rewrite [`Self::save_multi_agent_flags`] (which takes the
    /// same mutex itself): the durable read-modify-write must run under the
    /// store's `multi_agent_flags_io` mutex, or a concurrent save's older
    /// snapshot lands last and resurrects removed flags.
    pub(crate) fn apply_multi_agent_mutation_locked(
        upserts: &[(&str, bool)],
        removes: &[&str],
    ) -> Result<()> {
        crate::features::sessions::sidecars::mutate_json_list_file(
            "_multi_agent.json",
            |ids: &mut Vec<String>| {
                let mut changed = false;
                for (id, enabled) in upserts {
                    let exists = ids.iter().any(|entry| entry == id);
                    if *enabled && !exists {
                        ids.push((*id).to_string());
                        ids.sort();
                        changed = true;
                    } else if !*enabled && exists {
                        ids.retain(|entry| entry != id);
                        changed = true;
                    }
                }
                for id in removes {
                    let before = ids.len();
                    ids.retain(|entry| entry != id);
                    changed |= ids.len() != before;
                }
                changed
            },
        )
    }

    pub fn load_multi_agent_flags(&self) {
        let file = crate::platform::paths::sessions_root().join("_multi_agent.json");
        let Ok(content) = std::fs::read_to_string(&file) else {
            return;
        };
        let ids: Vec<String> = match serde_json::from_str(&content) {
            Ok(ids) => ids,
            Err(e) => {
                eprintln!("[sessions] load_multi_agent_flags failed: {e}");
                return;
            }
        };
        let sessions_dir = self.manager.sessions_dir().to_path_buf();
        let mut ghosts = false;
        {
            let mut m = self.mode_states.write();
            for id in ids {
                if !session_record_absent_for_sweep(&sessions_dir, &id) {
                    // 同 set_multi_agent：物化条目用解析出的默认 mode，
                    // 避免 or_default() 把 code 会话翻成 Yolo。
                    self.mode_state_entry(&mut m, &id, self.resolved_default_mode(&id))
                        .multi_agent = true;
                } else {
                    ghosts = true;
                }
            }
        }
        if ghosts {
            if let Err(error) = self.save_multi_agent_flags() {
                eprintln!(
                    "[sessions] rewrite _multi_agent.json after ghost cleanup failed: {error:#}"
                );
            }
        }
    }

    pub(crate) fn register_pending_plan(
        &self,
        id: &str,
        plan_id: String,
    ) -> Option<SessionModeState> {
        let default_mode = self.resolved_default_mode(id);
        let mut states = self.mode_states.write();
        let entry = self.mode_state_entry(&mut states, id, default_mode);
        if entry.mode != SerializableMode::Plan || entry.plan_claim_in_flight.is_some() {
            return None;
        }
        entry.pending_plan_id = Some(plan_id);
        Some(entry.clone())
    }

    pub(crate) fn claim_pending_plan(&self, id: &str, plan_id: &str) -> Result<PendingPlanClaim> {
        let accepted_state = {
            let default_mode = self.resolved_default_mode(id);
            let mut states = self.mode_states.write();
            let entry = self.mode_state_entry(&mut states, id, default_mode);
            if entry.mode != SerializableMode::Plan
                || entry.pending_plan_id.as_deref() != Some(plan_id)
                || entry.plan_claim_in_flight.is_some()
            {
                bail!("plan_not_active");
            }
            // Tri-mode safety: execution after plan approval returns to the
            // mode the session was in before entering Plan (fallback Auto) —
            // approving a plan approves the work, not full-auto permission.
            entry.mode = entry.pre_plan_mode.unwrap_or(SerializableMode::Auto);
            entry.pending_plan_id = None;
            entry.plan_claim_in_flight = Some(plan_id.to_string());
            entry.clone()
        };
        Ok(PendingPlanClaim {
            store: self.clone(),
            session_id: id.to_string(),
            plan_id: plan_id.to_string(),
            accepted_state,
            settled: false,
        })
    }

    pub(crate) fn finish_pending_plan_claim(&self, id: &str, plan_id: &str) {
        let mut states = self.mode_states.write();
        let Some(entry) = states.get_mut(id) else {
            return;
        };
        if entry.plan_claim_in_flight.as_deref() == Some(plan_id) {
            entry.plan_claim_in_flight = None;
        }
    }

    pub(crate) fn restore_pending_plan_claim(&self, id: &str, plan_id: &str) -> Result<()> {
        let default_mode = self.resolved_default_mode(id);
        let mut states = self.mode_states.write();
        let entry = self.mode_state_entry(&mut states, id, default_mode);
        let claimed_mode = entry.pre_plan_mode.unwrap_or(SerializableMode::Auto);
        if entry.mode != claimed_mode
            || entry.pending_plan_id.is_some()
            || entry.plan_claim_in_flight.as_deref() != Some(plan_id)
        {
            bail!("restore plan claim conflict");
        }
        entry.mode = SerializableMode::Plan;
        entry.pending_plan_id = Some(plan_id.to_string());
        entry.plan_claim_in_flight = None;
        Ok(())
    }

    pub(crate) fn discard_pending_plan(&self, id: &str, plan_id: &str) -> Result<SessionModeState> {
        let default_mode = self.resolved_default_mode(id);
        let mut states = self.mode_states.write();
        let entry = self.mode_state_entry(&mut states, id, default_mode);
        if entry.mode != SerializableMode::Plan
            || entry.pending_plan_id.as_deref() != Some(plan_id)
            || entry.plan_claim_in_flight.is_some()
        {
            bail!("plan_not_active");
        }
        entry.pending_plan_id = None;
        Ok(entry.clone())
    }

    pub(crate) fn take_pending_turn_injections(&self, id: &str) -> PendingTurnInjections {
        let persona = self.mode_states.write().get_mut(id).and_then(|state| {
            state
                .pending_persona_body
                .take()
                .map(|body| (state.active_persona.clone(), body))
        });
        PendingTurnInjections {
            store: self.clone(),
            session_id: id.to_string(),
            persona,
            committed: false,
        }
    }

    pub(crate) fn restore_pending_turn_injections(
        &self,
        id: &str,
        persona: Option<(Option<String>, String)>,
    ) {
        let Some((persona_id, body)) = persona else {
            return;
        };
        let mut states = self.mode_states.write();
        let Some(state) = states.get_mut(id) else {
            return;
        };
        if state.active_persona == persona_id && state.pending_persona_body.is_none() {
            state.pending_persona_body = Some(body);
        }
    }

    /// Test-only granular setter; production paths must publish through the
    /// atomic `set_persona` so persona id and one-shot body never tear.
    #[cfg(test)]
    pub fn set_active_persona(&self, id: &str, persona_id: Option<String>) {
        let default_mode = self.resolved_default_mode(id);
        self.mode_state_entry(&mut self.mode_states.write(), id, default_mode)
            .active_persona = persona_id;
    }

    /// Publish the selected persona and its one-shot body as one state change.
    pub fn set_persona(&self, id: &str, persona_id: Option<String>, pending_body: Option<String>) {
        let default_mode = self.resolved_default_mode(id);
        let mut states = self.mode_states.write();
        let state = self.mode_state_entry(&mut states, id, default_mode);
        state.active_persona = persona_id;
        state.pending_persona_body = pending_body;
    }

    pub fn active_persona_id(&self, id: &str) -> Option<String> {
        self.mode_states.read().get(id)?.active_persona.clone()
    }

    /// Test-only granular setter; production paths must publish through the
    /// atomic `set_persona` so persona id and one-shot body never tear.
    #[cfg(test)]
    pub fn set_pending_persona_body(&self, id: &str, body: Option<String>) {
        let default_mode = self.resolved_default_mode(id);
        self.mode_state_entry(&mut self.mode_states.write(), id, default_mode)
            .pending_persona_body = body;
    }

    /// Clear a deleted persona and its one-shot body from every session that
    /// still references it. Returning the affected IDs lets callers publish a
    /// presentation update for each session after the atomic state change.
    pub fn remove_persona_from_all(&self, persona_id: &str) -> Vec<String> {
        let mut states = self.mode_states.write();
        let mut changed = Vec::new();
        for (session_id, state) in states.iter_mut() {
            if state.active_persona.as_deref() != Some(persona_id) {
                continue;
            }
            state.active_persona = None;
            state.pending_persona_body = None;
            changed.push(session_id.clone());
        }
        changed.sort();
        changed
    }

    pub fn set_mounted_collection(&self, id: &str, collection_id: Option<i64>) {
        let mounted = collection_id
            .filter(|collection_id| *collection_id > 0)
            .map(|collection_id| MountedCollection {
                collection_id,
                enabled: true,
            })
            .into_iter()
            .collect();
        self.set_mounted_collections(id, mounted);
    }

    pub fn set_mounted_collections(
        &self,
        id: &str,
        collections: Vec<MountedCollection>,
    ) -> MountedCollectionsSnapshot {
        self.update_mounted_collections(id, |_| collections)
    }

    pub fn add_mounted_collection(
        &self,
        id: &str,
        collection_id: i64,
    ) -> MountedCollectionsSnapshot {
        self.update_mounted_collections(id, |mut collections| {
            if let Some(collection) = collections
                .iter_mut()
                .find(|collection| collection.collection_id == collection_id)
            {
                collection.enabled = true;
            } else {
                collections.push(MountedCollection {
                    collection_id,
                    enabled: true,
                });
            }
            collections
        })
    }

    pub fn set_mounted_collection_enabled(
        &self,
        id: &str,
        collection_id: i64,
        enabled: bool,
    ) -> MountedCollectionsSnapshot {
        self.update_mounted_collections(id, |mut collections| {
            if let Some(collection) = collections
                .iter_mut()
                .find(|collection| collection.collection_id == collection_id)
            {
                collection.enabled = enabled;
            }
            collections
        })
    }

    pub fn remove_mounted_collection(
        &self,
        id: &str,
        collection_id: i64,
    ) -> MountedCollectionsSnapshot {
        self.update_mounted_collections(id, |mut collections| {
            collections.retain(|collection| collection.collection_id != collection_id);
            collections
        })
    }

    /// Remove one local collection from every in-memory session. Shares the
    /// generic "sweep all sessions" core with the remote-collection family;
    /// only the per-session removal and its snapshot write-back differ.
    pub fn remove_mounted_collection_from_all(
        &self,
        collection_id: i64,
    ) -> Vec<(String, MountedCollectionsSnapshot)> {
        let mut states = self.mode_states.write();
        Self::remove_from_all_sessions_where(&mut states, |state| {
            let mut collections = folded_collections(state);
            let previous_len = collections.len();
            collections.retain(|collection| collection.collection_id != collection_id);
            (collections.len() != previous_len).then(|| {
                state.mounted_collection = collections
                    .iter()
                    .find(|collection| collection.enabled)
                    .map(|collection| collection.collection_id);
                state.mounted_collections = collections.clone();
                state.mounted_collections_revision =
                    state.mounted_collections_revision.wrapping_add(1).max(1);
                MountedCollectionsSnapshot {
                    revision: state.mounted_collections_revision,
                    collections,
                }
            })
        })
    }

    pub(crate) fn update_mounted_collections<F>(
        &self,
        id: &str,
        update: F,
    ) -> MountedCollectionsSnapshot
    where
        F: FnOnce(Vec<MountedCollection>) -> Vec<MountedCollection>,
    {
        let default_mode = self.resolved_default_mode(id);
        let mut states = self.mode_states.write();
        let state = self.mode_state_entry(&mut states, id, default_mode);
        let current = folded_collections(state);
        let mut normalized = Vec::new();
        for collection in update(current) {
            if collection.collection_id <= 0
                || normalized.iter().any(|mounted: &MountedCollection| {
                    mounted.collection_id == collection.collection_id
                })
            {
                continue;
            }
            normalized.push(collection);
        }
        let legacy = normalized
            .iter()
            .find(|collection| collection.enabled)
            .map(|collection| collection.collection_id);
        state.mounted_collection = legacy;
        state.mounted_collections = normalized.clone();
        state.mounted_collections_revision =
            state.mounted_collections_revision.wrapping_add(1).max(1);
        MountedCollectionsSnapshot {
            revision: state.mounted_collections_revision,
            collections: normalized,
        }
    }

    pub fn mounted_collections(&self, id: &str) -> Vec<MountedCollection> {
        self.mounted_collections_snapshot(id).collections
    }

    pub fn mounted_collections_snapshot(&self, id: &str) -> MountedCollectionsSnapshot {
        let states = self.mode_states.read();
        let Some(state) = states.get(id) else {
            return MountedCollectionsSnapshot {
                revision: 0,
                collections: Vec::new(),
            };
        };
        let collections = folded_collections(state);
        MountedCollectionsSnapshot {
            revision: state.mounted_collections_revision,
            collections,
        }
    }

    pub fn mounted_collection_ids(&self, id: &str) -> Vec<i64> {
        self.mounted_collections(id)
            .into_iter()
            .filter(|collection| collection.enabled)
            .map(|collection| collection.collection_id)
            .collect()
    }

    pub fn mounted_collection(&self, id: &str) -> Option<i64> {
        self.mounted_collection_ids(id).into_iter().next()
    }

    pub fn mounted_remote_collections(&self, id: &str) -> Vec<MountedRemoteCollection> {
        self.mode_states
            .read()
            .get(id)
            .map(|state| state.mounted_remote_collections.clone())
            .unwrap_or_default()
    }

    pub fn add_mounted_remote_collection(
        &self,
        id: &str,
        server_id: String,
        collection_id: i64,
    ) -> Vec<MountedRemoteCollection> {
        self.update_mounted_remote_collections(id, |mut collections| {
            if let Some(collection) = collections.iter_mut().find(|collection| {
                collection.server_id == server_id && collection.collection_id == collection_id
            }) {
                collection.enabled = true;
            } else {
                collections.push(MountedRemoteCollection {
                    server_id,
                    collection_id,
                    enabled: true,
                });
            }
            collections
        })
    }

    pub fn set_mounted_remote_collection_enabled(
        &self,
        id: &str,
        server_id: &str,
        collection_id: i64,
        enabled: bool,
    ) -> Vec<MountedRemoteCollection> {
        self.update_mounted_remote_collections(id, |mut collections| {
            if let Some(collection) = collections.iter_mut().find(|collection| {
                collection.server_id == server_id && collection.collection_id == collection_id
            }) {
                collection.enabled = enabled;
            }
            collections
        })
    }

    pub fn remove_mounted_remote_collection(
        &self,
        id: &str,
        server_id: &str,
        collection_id: i64,
    ) -> Vec<MountedRemoteCollection> {
        self.update_mounted_remote_collections(id, |mut collections| {
            collections.retain(|collection| {
                collection.server_id != server_id || collection.collection_id != collection_id
            });
            collections
        })
    }

    /// Shared core for the "remove from every session" collection sweeps (the
    /// local `MountedCollection` snapshot family and the remote
    /// `MountedRemoteCollection` family): runs `sweep` on every in-memory
    /// session, skips sessions where it reports no change (`None`), and
    /// returns the changed session ids (sorted) with the sweep's per-session
    /// result. Sessions without a match are left untouched.
    fn remove_from_all_sessions_where<R>(
        states: &mut HashMap<String, SessionModeState>,
        mut sweep: impl FnMut(&mut SessionModeState) -> Option<R>,
    ) -> Vec<(String, R)> {
        let mut changed = Vec::new();
        for (session_id, state) in states.iter_mut() {
            if let Some(result) = sweep(state) {
                changed.push((session_id.clone(), result));
            }
        }
        changed.sort_by(|left, right| left.0.cmp(&right.0));
        changed
    }

    /// Remote-collection specialization of [`Self::remove_from_all_sessions_where`]:
    /// drops mounts matching `should_remove` from all in-memory sessions and
    /// returns the changed session ids (sorted) with their post-removal
    /// collections. Sessions without a match are left untouched.
    fn remove_mounted_remote_collections_where(
        states: &mut HashMap<String, SessionModeState>,
        should_remove: impl Fn(&MountedRemoteCollection) -> bool,
    ) -> Vec<(String, Vec<MountedRemoteCollection>)> {
        Self::remove_from_all_sessions_where(states, |state| {
            let previous_len = state.mounted_remote_collections.len();
            state
                .mounted_remote_collections
                .retain(|collection| !should_remove(collection));
            (state.mounted_remote_collections.len() != previous_len)
                .then(|| state.mounted_remote_collections.clone())
        })
    }

    /// Remove one remote collection from every in-memory session.
    pub fn remove_mounted_remote_collection_from_all(
        &self,
        server_id: &str,
        collection_id: i64,
    ) -> Vec<(String, Vec<MountedRemoteCollection>)> {
        let mut states = self.mode_states.write();
        Self::remove_mounted_remote_collections_where(&mut states, |collection| {
            collection.server_id == server_id && collection.collection_id == collection_id
        })
    }

    /// Remove every mount belonging to a disconnected remote server from all sessions.
    pub fn remove_remote_server_mounts(
        &self,
        server_id: &str,
    ) -> Vec<(String, Vec<MountedRemoteCollection>)> {
        let mut states = self.mode_states.write();
        Self::remove_mounted_remote_collections_where(&mut states, |collection| {
            collection.server_id == server_id
        })
    }

    fn update_mounted_remote_collections<F>(
        &self,
        id: &str,
        update: F,
    ) -> Vec<MountedRemoteCollection>
    where
        F: FnOnce(Vec<MountedRemoteCollection>) -> Vec<MountedRemoteCollection>,
    {
        let default_mode = self.resolved_default_mode(id);
        let mut states = self.mode_states.write();
        let state = self.mode_state_entry(&mut states, id, default_mode);
        let mut normalized = Vec::new();
        for collection in update(state.mounted_remote_collections.clone()) {
            if collection.server_id.trim().is_empty()
                || collection.collection_id <= 0
                || normalized.iter().any(|mounted: &MountedRemoteCollection| {
                    mounted.server_id == collection.server_id
                        && mounted.collection_id == collection.collection_id
                })
            {
                continue;
            }
            normalized.push(collection);
        }
        state.mounted_remote_collections = normalized.clone();
        normalized
    }

    // ===================== per-session mode 持久化（所有会话） =====================

    /// Apply id-level mode upserts/removes directly to
    /// `_session_mode_states.json`, leaving every entry this process never
    /// touched exactly as another process wrote it. The in-memory map is a
    /// boot-time snapshot plus this process's own changes, so rewriting it
    /// wholesale onto the shared store would silently revert a mode switch
    /// persisted by a concurrent GUI/headless process after this one booted
    /// — e.g. a headless Plan run flipping a just-GUI-set Yolo session back.
    ///
    /// 原子写 + 失败可见：直接 `std::fs::write` 在进程中断时可能留下截断文件，
    /// 而 `load_session_mode_states` 对损坏文件是静默跳过——一次中断写入会让所有
    /// per-session mode 记录永久丢失，表现为「显式切过 mode，重启后回 Plan」。
    ///
    /// The `_locked` suffix is the caller contract shared with the other
    /// sidecar mutators: the durable RMW must run under the store's
    /// `session_mode_states_io` mutex (`set_mode`, `set_mode_and_persist`,
    /// `persist_accepted_yolo_mode`, and the retention purge all do), or two
    /// concurrent RMWs interleave and the later write lands over the earlier
    /// one's change.
    pub(crate) fn apply_session_mode_mutation_locked(
        upserts: &[(String, SerializableMode)],
        removes: &[String],
    ) -> Result<()> {
        // Downgrade compatibility (reviewer P2, mirroring the settings.json
        // lane's `*_auto` companion discipline): the shared map keeps the
        // pre-tri-mode vocabulary — an explicit Auto is durably `"plan"`
        // there plus a `true` flag in `_session_auto_modes.json`, so a
        // pre-tri-mode binary sharing PINVOU3_HOME strict-parses the whole
        // map instead of quarantining it and rebuilding every other session's
        // choice from empty on its next write; Auto itself degrades
        // conservatively to Plan in that binary's view.
        //
        // The map write lands first: a crash between the two writes leaves
        // the conservative `plan` durable without the flag (reads back as
        // Plan), never an Auto flag over an un-collapsed entry — that is the
        // AUTO upsert's window. The Plan downshift of a flagged Auto is the
        // one residual: the shared-map entry already reads `plan` (the
        // collapse value), so the RMW is a no-op there and the crash window
        // is the entire companion removal — a stale `true` flag over `plan`
        // reads back as Auto on a new binary (old binary sees Plan; the
        // widening stays below Yolo, docs/safety-modes.md §9 old-writer
        // boundary). Both RMWs run under the caller's io mutex, so the pair
        // cannot interleave with another mutator's write.
        let mut companion: Vec<(String, bool)> = Vec::with_capacity(upserts.len());
        let mapped: Vec<(String, SerializableMode)> = upserts
            .iter()
            .map(|(id, mode)| {
                companion.push((id.clone(), *mode == SerializableMode::Auto));
                (id.clone(), Self::collapse_auto_for_sidecar(*mode))
            })
            .collect();
        crate::features::sessions::sidecars::mutate_json_map_file(
            crate::features::sessions::sidecars::shared_mode_map_file_name(),
            |entries: &mut std::collections::HashMap<String, SerializableMode>| {
                let mut changed = false;
                for (id, mode) in &mapped {
                    if entries.get(id) != Some(mode) {
                        entries.insert(id.clone(), mode.clone());
                        changed = true;
                    }
                }
                for id in removes {
                    changed |= entries.remove(id).is_some();
                }
                changed
            },
        )?;
        // Test-only crash seam (round-9 minor 1, mirroring the rebind pair's
        // `inject_rebind_crash_after_legacy_rewrite`): the shared→companion
        // write order is the load-bearing downgrade invariant — the crash
        // window must leave the conservative `plan` durable WITHOUT the flag,
        // never the reverse pair. The seam stops here with the shared write
        // already on disk so a reorder-to-companion-first regression fails
        // the crash-window test instead of passing invisibly.
        #[cfg(test)]
        if MODE_PAIR_CRASH_AFTER_SHARED_WRITE.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        crate::features::sessions::sidecars::mutate_json_map_file(
            "_session_auto_modes.json",
            |entries: &mut std::collections::HashMap<String, bool>| {
                let mut changed = false;
                for (id, auto) in &companion {
                    changed |= if *auto {
                        entries.insert(id.clone(), true) != Some(true)
                    } else {
                        entries.remove(id).is_some()
                    };
                }
                for id in removes {
                    changed |= entries.remove(id).is_some();
                }
                changed
            },
        )
    }

    /// The durable map value for a mode: the shared `_session_mode_states.json`
    /// keeps the pre-tri-mode vocabulary (`plan`/`yolo` only — see the
    /// `SerializableMode` compat doc), so an explicit Auto collapses to `plan`
    /// there; the companion flag in `_session_auto_modes.json` carries the
    /// actual choice.
    fn collapse_auto_for_sidecar(mode: SerializableMode) -> SerializableMode {
        match mode {
            SerializableMode::Auto => SerializableMode::Plan,
            other => other,
        }
    }

    /// The durable explicit-Auto flags (`_session_auto_modes.json`):
    /// `true` restores the collapsed `plan` map entry as Auto on every read
    /// path ([`Self::load_session_mode_states`], [`Self::durable_mode_entry`]).
    /// Unreadable/absent file reads as empty — the collapsed `plan` value is
    /// the safe degradation, never a silent widening.
    fn read_session_auto_modes() -> std::collections::HashMap<String, bool> {
        let file = crate::platform::paths::sessions_root().join("_session_auto_modes.json");
        match std::fs::read_to_string(&file) {
            // Absent file = no explicit Auto overrides recorded: the normal
            // quiet path, not a fault.
            Err(_) => Default::default(),
            Ok(content) => match serde_json::from_str(&content) {
                Ok(parsed) => parsed,
                // Corrupt companion: fail conservative (empty map, every
                // recorded Auto degrades to Plan) but LOUD, same as the
                // sibling loaders — silent skips are exactly what makes
                // "explicitly switched, reverted after restart"
                // undebuggable (sidecar discipline doc). The write path
                // quarantines loudly; only the corrupt-before-first-write
                // window lands here (round-8 L2-2).
                Err(error) => {
                    eprintln!(
                        "[sessions] corrupt _session_auto_modes.json (Auto overrides degrade to Plan): {error}"
                    );
                    Default::default()
                }
            },
        }
    }

    /// Resolve one map entry against the companion flags: `plan` + `true`
    /// flag is the durable spelling of an explicit Auto choice. A flag
    /// without a map entry resolves to `mode` unchanged — the map entry is
    /// the existence authority (the funnel removes both together; a crash
    /// between them can only leave a stale flag, which the next upsert of
    /// the session recomputes).
    ///
    /// The flag upgrades only an entry still holding the Auto-collapse
    /// value (`plan`). An entry that reads `yolo` was written after the
    /// Auto choice by the only writer that cannot know the companion file
    /// — a pre-tri-mode binary's explicit switch — and that acknowledged
    /// map write wins: overlaying the stale flag would silently resurrect
    /// Auto over the persisted Yolo (review P2). Residual (unresolvable
    /// without a format change): a pre-tri-mode binary explicitly writing
    /// `plan` over a recorded Auto is byte-identical to the collapse value,
    /// so that explicit Plan still resolves back to Auto; the new UI can
    /// re-pick Plan with one switch.
    fn resolve_sidecar_mode(
        mode: SerializableMode,
        auto_flags: &std::collections::HashMap<String, bool>,
        id: &str,
    ) -> SerializableMode {
        if mode == SerializableMode::Plan && auto_flags.get(id).copied().unwrap_or(false) {
            SerializableMode::Auto
        } else {
            mode
        }
    }

    /// Restore every session's per-session mode at boot: merge into
    /// `mode_states`, so reopening a session restores the mode it last
    /// explicitly used.
    ///
    /// Compatibility: when the new file is absent, fall back to reading the
    /// legacy `_code_mode_states.json` (a remnant of the code-session-only
    /// era) and **immediately persist it into the new file** to complete the
    /// migration; the old file is not deleted.
    ///
    /// Materializing the legacy map here is load-bearing, not tidiness. Every
    /// durable mode write is now an id-level read-modify-write against
    /// `_session_mode_states.json` (`apply_session_mode_mutation_locked`),
    /// and an absent file starts that RMW from an empty map. So without this
    /// step the first mode switch after an upgrade would write a single-entry
    /// file, and because the legacy fallback above is keyed on the new file
    /// being absent, it would never be consulted again: every pre-upgrade
    /// per-session mode silently lost, and a session the user had pinned to
    /// Plan reopening under `resolved_default_mode` — the exact reopen
    /// divergence this persistence exists to prevent. (The whole-map writer
    /// this replaced completed the migration implicitly by rewriting the
    /// in-memory map on every save.)
    pub fn load_session_mode_states(&self) {
        let states_file = crate::platform::paths::sessions_root()
            .join(crate::features::sessions::sidecars::shared_mode_map_file_name());
        let legacy_file = crate::platform::paths::sessions_root().join("_code_mode_states.json");
        let (source, from_legacy) = if states_file.exists() {
            (states_file, false)
        } else if legacy_file.exists() {
            (legacy_file, true)
        } else {
            return;
        };
        let content = match std::fs::read_to_string(&source) {
            Ok(c) => c,
            Err(_) => return,
        };
        let mut modes: std::collections::HashMap<String, SerializableMode> =
            match serde_json::from_str(&content) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("[sessions] load_session_mode_states failed: {e}");
                    return;
                }
            };
        // Companion-flag overlay before any consumer: `plan` + `true` flag is
        // the durable spelling of an explicit Auto (the shared map keeps the
        // pre-tri-mode vocabulary for downgrade parsing — see
        // `apply_session_mode_mutation_locked`). Applied before the legacy
        // migration so the materialized map, the persisted mirror, and the
        // mint loop below all see the resolved mode.
        {
            let auto_flags = Self::read_session_auto_modes();
            if !auto_flags.is_empty() {
                for (id, mode) in modes.iter_mut() {
                    *mode = Self::resolve_sidecar_mode(*mode, &auto_flags, id);
                }
            }
        }
        // Boot-time ghost sweep (retention parity with the multi-agent flags
        // loader): a crash between the session-record delete and the sidecar
        // purge leaves a dead id's entry behind, and a recycled id would
        // silently inherit its mode — and, via the seeding point, its
        // ask-on-guardrail override — through the boot mint. The shared map
        // entry is the existence authority for the companion flag, so the
        // funnel removes both together. Boot-time only: no concurrent writer
        // holds the io mutex yet.
        let sessions_dir = crate::platform::paths::sessions_root();
        let ghost_ids: Vec<String> = modes
            .keys()
            .filter(|id| session_record_absent_for_sweep(&sessions_dir, id))
            .cloned()
            .collect();
        if !ghost_ids.is_empty() {
            match Self::apply_session_mode_mutation_locked(&[], &ghost_ids) {
                Ok(()) => {
                    for id in &ghost_ids {
                        modes.remove(id);
                    }
                    eprintln!(
                        "[sessions] swept {} ghost session mode entries",
                        ghost_ids.len()
                    );
                }
                Err(error) => {
                    // Keep booting with the in-memory map intact: modes for
                    // live sessions are still correct and the next boot
                    // retries the sweep (same tolerance as the legacy
                    // migration below).
                    eprintln!("[sessions] sweep ghost session mode entries failed: {error:#}");
                }
            }
        }
        if from_legacy && !modes.is_empty() {
            let upserts: Vec<(String, SerializableMode)> = modes
                .iter()
                .map(|(id, mode)| (id.clone(), mode.clone()))
                .collect();
            // Boot-time, before any writer can race: the io mutex exists for
            // concurrent mutators and there are none yet.
            if let Err(error) = Self::apply_session_mode_mutation_locked(&upserts, &[]) {
                // Keep booting with the in-memory map: the modes are still
                // correct for this run, and the next boot retries the
                // migration. Residual: if a LATER mode write in this run
                // succeeds, it creates the new file and the absence-keyed
                // legacy fallback never fires again — modes that only lived
                // in the legacy file are then lost at the next restart. Both
                // this failure and that later write are logged; closing the
                // window needs migration-retry state that outlives the boot
                // attempt.
                eprintln!("[sessions] migrate _code_mode_states.json failed: {error:#}");
            }
        }
        {
            let mut persisted = self.session_mode_states.write();
            *persisted = modes.clone();
        }
        // Boot minting seeds the toggle from the CURRENT global default: the
        // override map is not loaded yet at this point (its file is merged
        // AFTER this by `load_ask_on_guardrail_overrides`, which also applies
        // explicit choices to the entries minted here). Later mints in this
        // run go through `ask_on_guardrail_seed` and consult the loaded map.
        // Resolved before the write lock — the mirror read is in-memory only.
        let ask_on_guardrail_default = self.code_permission_prefs().ask_on_guardrail_default;
        let mut states = self.mode_states.write();
        for (id, mode) in modes {
            states
                .entry(id)
                .or_insert_with(|| {
                    SessionModeState::default_for_new_session(ask_on_guardrail_default)
                })
                .mode = mode;
        }
    }

    pub fn code_permission_prefs(&self) -> CodePermissionPrefs {
        *self.code_permission.read()
    }

    /// 命令线（get_code_permission_prefs / confirm_code_yolo /
    /// set_ask_on_guardrail_default）回给前端的折叠视图，与
    /// [`Self::mode_defaults`] 同一折叠：显式 Auto 选择以 `last_mode_auto`
    /// 伴随旗持久化、legacy 字段永不携带 "auto"，但视图把旗折回
    /// `last_mode = Some(Auto)`——codex 草稿芯片按 `last_mode` 解析全局默认
    /// （`nativeModeFallback`），只读 legacy 字段会把重启后的显式 Auto 显示
    /// 成 Plan，与 `resolved_default_mode` 的实际行为背离（display-vs-behavior
    /// 分叉，且分叉方向是更宽的 Auto）。原始镜像仍走
    /// [`Self::code_permission_prefs`]（companion-flag 磁盘形状测试依赖）。
    pub fn code_permission_prefs_view(&self) -> CodePermissionPrefs {
        let mut prefs = self.code_permission_prefs();
        if prefs.last_mode_auto {
            prefs.last_mode = Some(SerializableMode::Auto);
        }
        prefs
    }

    /// View of the per-lane global default modes (in-memory mirror; the disk
    /// truth for work lives in settings.json `mode_defaults.work[_auto]`, for
    /// code in `code_permission.last_mode[_auto]`). An explicit Auto choice
    /// is folded back into the view here so callers keep seeing one
    /// vocabulary: the legacy fields deliberately never carry "auto"
    /// (pre-tri-mode binaries would fail the whole settings.json parse).
    pub fn mode_defaults(&self) -> ModeDefaultsView {
        let defaults = self.mode_defaults.read();
        let code = self.code_permission.read();
        ModeDefaultsView {
            work: if defaults.work_auto {
                Some(SerializableMode::Auto)
            } else {
                defaults.work
            },
            code: if code.last_mode_auto {
                Some(SerializableMode::Auto)
            } else {
                code.last_mode
            },
        }
    }

    /// An explicit draft-state switch writes the matching lane's global
    /// default mode (two-lane semantics: switches in already-materialized
    /// sessions never touch this). Persists settings.json in a field-level
    /// transaction and publishes the in-memory mirror inside the same
    /// critical section (post_commit — mirror and disk stay totally ordered
    /// against concurrent transactions). A write failure returns `Err` and
    /// leaves both untouched — the failure MUST reach the command layer
    /// (review JensenChen28 5780647695 + zhuowp 5780927743 second): the
    /// log-only tolerance left the caller answering `Ok(stale defaults)`,
    /// so both IPC bridges stored the old default while the draft chip was
    /// already drawn as the target — a later session materialized with the
    /// OLD backend default while the UI claimed the switch landed. Unlike
    /// `set_mode` (session-local, lenient), this value drives the
    /// draft→session admission posture, so the caller decides the user
    /// face: keep the authoritative state and surface the error.
    pub fn set_mode_default(&self, lane: ModeLane, mode: SerializableMode) -> Result<(), String> {
        // Tri-mode (M2): `Auto` never enters the legacy `last_mode`/`work`
        // vocabulary — a pre-tri-mode binary parsing the unknown enum variant
        // fails the WHOLE settings.json (every unrelated preference falls
        // back to system-locale defaults, and its next settings save then
        // durably rewrites them). The choice rides the dedicated `*_auto`
        // companion flags, which old binaries simply drop (unknown bool);
        // the legacy fields only ever carry Plan/Yolo.
        let legacy = if mode == SerializableMode::Auto {
            None
        } else {
            Some(mode)
        };
        let auto_flag = mode == SerializableMode::Auto;
        // 镜像发布走 post_commit（round-11 评审）：与 confirm_code_yolo /
        // set_ask_on_guardrail_default 同一临界区语义——锁外先写镜像时，两笔
        // 交错事务的镜像/磁盘会各停在不同提交上，读分裂直到重启。提交失败
        // 时镜像不发布（本 run 答陈旧值直到下次成功提交，与磁盘一致）；
        // 失败沿命令返回路径传播（评审 5780647695），桥/前端失败时保持权
        // 威态并显式报错，而不是把旧默认当成功收下、同时把芯片画成 target。
        UserPrefs::update_transaction_with_post_commit(
            move |prefs| {
                match lane {
                    ModeLane::Code => {
                        prefs.code_permission.last_mode_auto = auto_flag;
                        prefs.code_permission.last_mode = legacy;
                    }
                    ModeLane::Work => {
                        prefs.mode_defaults.work_auto = auto_flag;
                        prefs.mode_defaults.work = legacy;
                    }
                }
                Ok(())
            },
            move || match lane {
                ModeLane::Code => {
                    let mut cp = self.code_permission.write();
                    cp.last_mode_auto = auto_flag;
                    cp.last_mode = legacy;
                }
                ModeLane::Work => {
                    let mut md = self.mode_defaults.write();
                    md.work_auto = auto_flag;
                    md.work = legacy;
                }
            },
        )
        .map(|_| ())
        .map_err(|error| {
            eprintln!("[sessions] persist mode default for {lane:?} failed: {error}");
            error
        })
    }

    /// After the accept flow (`claim_pending_plan` switching to Yolo) is
    /// confirmed and committed, records the task-level switch in per-session
    /// persistence: writes `_session_mode_states.json` (reopening the session,
    /// or switching away and back, restores Yolo). Global lane defaults are
    /// **not** updated (two-lane semantics: a switch in an
    /// already-materialized session writes only that session's own record).
    ///
    /// Called only from `PendingPlanClaim::commit` (engine commit confirmed):
    /// the mode is remembered once the task actually starts executing; a
    /// failed commit rolls back via `restore_pending_plan_claim` without
    /// touching disk, and the in-memory return to Plan stays consistent with
    /// disk.
    pub(crate) fn persist_accepted_yolo_mode(&self, id: &str) {
        // Tri-mode safety: plan approval returns to the pre-plan mode
        // (fallback Auto), so the persisted value is whatever the in-memory
        // claim recorded — the function name is historical. The claim read
        // sits INSIDE the io critical section (round-9 minor, same order as
        // `set_mode`): reading it before the lock let a concurrent set_mode
        // landing in the read→lock window be overwritten here by the stale
        // value, both in the mirror and on disk, reopening the session in
        // its pre-switch mode.
        let _io = self.session_mode_states_io.lock();
        let accepted_mode = {
            let states = self.mode_states.read();
            states
                .get(id)
                .map(|entry| entry.mode)
                .unwrap_or(SerializableMode::Auto)
        };
        self.session_mode_states
            .write()
            .insert(id.to_string(), accepted_mode);
        if let Err(error) =
            Self::apply_session_mode_mutation_locked(&[(id.to_string(), accepted_mode)], &[])
        {
            // The session id is deliberately kept out of the message: boot
            // logs persist to disk and must not accumulate raw session
            // identifiers (CodeQL cleartext-logging gate).
            eprintln!("[sessions] persist accepted plan mode failed: {error:#}");
        }
    }

    /// Set the session's ask-on-guardrail toggle (tri-mode safety). The
    /// toggle rides the in-memory mode-state entry when one exists and
    /// persists into its own `_ask_on_guardrail.json` sidecar (a
    /// `HashMap<String, bool>` of explicit overrides — sessions absent from
    /// the file follow the global `ask_on_guardrail_default`). A session
    /// with NO materialized mode entry still records the toggle in the
    /// sidecar and the in-memory override mirror — the toggle is durable
    /// user intent and must not silently vanish; it does **not** mint a mode
    /// entry (that would pin a mode the user never chose), and the next
    /// mint consults the override via `ask_on_guardrail_seed` (crate-private).
    /// Same critical-section discipline as [`Self::set_mode`]: the durable
    /// RMW runs under the shared io mutex — but unlike `set_mode`
    /// (log-only), a failed persist returns `Err`. The sidecar RMW runs
    /// FIRST and the in-memory entry/override mirror is published only after
    /// it succeeds (reviewer P3): releasing those locks before the write let
    /// a concurrent reader consume the new value even when the write
    /// subsequently failed and rolled back — rollback does not retract an
    /// already-consumed posture. The RMW depends on nothing in memory, so
    /// the reorder is behavior-neutral on success and makes the failure path
    /// a no-op: no reader ever sees a value the sidecar does not carry
    /// (fail-consistent; docs §3.1 of safety-modes.md). The command layer
    /// publishes its broadcast only on `Ok`, so the frontend's existing
    /// catch rolls back its optimistic toggle and surfaces the failure.
    pub fn set_ask_on_guardrail(&self, id: &str, enabled: bool) -> Result<()> {
        let _io = self.session_mode_states_io.lock();
        Self::apply_ask_on_guardrail_mutation_locked(&[(id, enabled)], &[])?;
        // Post-success publication: an entry-less session records the
        // override only (sidecar + mirror below) and does not mint a mode
        // entry — the next mint consults the mirror via
        // [`Self::ask_on_guardrail_seed`].
        if let Some(entry) = self.mode_states.write().get_mut(id) {
            entry.ask_on_guardrail = enabled;
        }
        self.ask_on_guardrail_overrides
            .write()
            .insert(id.to_string(), enabled);
        Ok(())
    }

    /// THE ask-on-guardrail seeding point for fresh-entry mints: an explicit
    /// per-session override (recorded by [`Self::set_ask_on_guardrail`] even
    /// before the session had a mode entry, and mirrored from the sidecar at
    /// boot) wins over the global `ask_on_guardrail_default` preference.
    /// Every mint site must seed through this — seeding from the bare
    /// preference silently dropped a toggle recorded for an entry-less
    /// session the moment the entry materialized (or at the boot merge,
    /// which only touches entries that already exist).
    pub(crate) fn ask_on_guardrail_seed(&self, id: &str) -> bool {
        if let Some(enabled) = self.ask_on_guardrail_overrides.read().get(id) {
            return *enabled;
        }
        self.code_permission_prefs().ask_on_guardrail_default
    }

    /// Id-level RMW for `_ask_on_guardrail.json`; caller holds
    /// `session_mode_states_io`.
    pub(crate) fn apply_ask_on_guardrail_mutation_locked(
        upserts: &[(&str, bool)],
        removals: &[&str],
    ) -> Result<()> {
        crate::features::sessions::sidecars::mutate_json_map_file(
            "_ask_on_guardrail.json",
            |entries: &mut std::collections::HashMap<String, bool>| {
                let mut changed = false;
                for (id, enabled) in upserts {
                    if entries.get(*id) != Some(enabled) {
                        entries.insert((*id).to_string(), *enabled);
                        changed = true;
                    }
                }
                for id in removals {
                    changed |= entries.remove(*id).is_some();
                }
                changed
            },
        )
    }

    /// Boot merge of `_ask_on_guardrail.json`: applies explicit toggles to
    /// the mode-state entries boot minting materialized AND fills the
    /// in-memory override mirror, so entries minted LATER in this run
    /// (toggle written while entry-less, then a mode write materializes the
    /// entry) seed through `ask_on_guardrail_seed` (crate-private) and keep the
    /// recorded choice. Called after `load_session_mode_states`.
    pub fn load_ask_on_guardrail_overrides(&self) {
        let file = crate::platform::paths::sessions_root().join("_ask_on_guardrail.json");
        // 文件缺失是常态（无开关翻写的会话家），静默；读 IO 失败（权限/IO
        // 错误）与损坏一样响亮记日志——同一纪律：静默跳过会让「重启后开关
        // 全回默认」无从排查（round-9 minor）。
        let content = match std::fs::read_to_string(&file) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                eprintln!(
                    "[sessions] _ask_on_guardrail.json read failed, skipping override merge: \
                     {file:?}: {error}"
                );
                return;
            }
        };
        // 损坏/被截断则响亮记日志——与 load_session_mode_states 的读侧纪律
        // 一致，静默跳过会让「重启后开关全回默认」无从排查。
        let Ok(mut overrides) =
            serde_json::from_str::<std::collections::HashMap<String, bool>>(&content)
        else {
            eprintln!(
                "[sessions] _ask_on_guardrail.json unreadable, skipping override merge: {file:?}"
            );
            return;
        };
        // Boot-time ghost sweep for the override mirror (the mode-map
        // loader's sweep cannot own this: it early-returns when its own
        // sidecar file is absent, so an override ghost left by the same
        // crash window — record deleted, purge interrupted — would survive
        // forever and re-arm a recycled id through the seeding point
        // (`ask_on_guardrail_seed`). Sweeping here pins the invariant
        // unconditionally: after boot, no override survives for an id
        // without a session record file. Boot-time only: no concurrent
        // writer holds the io mutex yet.
        let sessions_dir = crate::platform::paths::sessions_root();
        let ghost_ids: Vec<String> = overrides
            .keys()
            .filter(|id| session_record_absent_for_sweep(&sessions_dir, id))
            .cloned()
            .collect();
        if !ghost_ids.is_empty() {
            let ghost_refs: Vec<&str> = ghost_ids.iter().map(String::as_str).collect();
            match Self::apply_ask_on_guardrail_mutation_locked(&[], &ghost_refs) {
                Ok(()) => {
                    for id in &ghost_ids {
                        overrides.remove(id);
                    }
                    eprintln!(
                        "[sessions] swept {} ghost ask-on-guardrail overrides",
                        ghost_ids.len()
                    );
                }
                Err(error) => {
                    // Keep booting with the parsed map intact (same
                    // tolerance as the mode-map loaders); the next boot
                    // retries the sweep.
                    eprintln!(
                        "[sessions] sweep ghost ask-on-guardrail overrides failed: {error:#}"
                    );
                }
            }
        }
        {
            let mut mirror = self.ask_on_guardrail_overrides.write();
            *mirror = overrides.clone();
        }
        let mut states = self.mode_states.write();
        for (id, enabled) in overrides {
            if let Some(entry) = states.get_mut(&id) {
                entry.ask_on_guardrail = enabled;
            }
        }
    }

    pub fn confirm_code_yolo(&self) -> Result<CodePermissionPrefs, String> {
        // 镜像发布走 post_commit（round-10 评审）：放在 USER_PREFS_LOCK 临界区
        // 内与并发事务全序化——锁外发布时，两笔交错事务的镜像/磁盘会各停在
        // 不同提交上（读分裂直到重启）。与 update_transaction_with_post_commit
        // 的 keyring 清理钩子同一临界区语义。
        UserPrefs::update_transaction_with_post_commit(
            |prefs| {
                prefs.code_permission.yolo_confirmed = true;
                Ok(())
            },
            || self.code_permission.write().yolo_confirmed = true,
        )?;
        Ok(self.code_permission_prefs_view())
    }

    /// Tri-mode safety: set the global ask-on-guardrail default (new
    /// sessions seed their per-session toggle from it; sessions absent from
    /// `_ask_on_guardrail.json` resolve against it). Same two-step shape as
    /// [`Self::confirm_code_yolo`]: settings.json field-level transaction
    /// first, then the in-memory mirror — the mirror is what
    /// `code_permission_prefs()` readers (and the session-materialization
    /// path) see, so skipping it would leave this run answering the stale
    /// value until restart.
    pub fn set_ask_on_guardrail_default(
        &self,
        enabled: bool,
    ) -> Result<CodePermissionPrefs, String> {
        // 镜像发布走 post_commit（round-10 评审）：镜像写留在锁外时，两个
        // 并发 toggle 的镜像可能停在先提交的那笔、磁盘停在后提交的那笔，
        // getter 与播种点此后一直答陈旧值直到重启。临界区内发布后镜像与
        // settings.json 全序一致（该值交替变化，不能依赖幂等性——与
        // confirm_code_yolo 的恒 true 写不同）。
        UserPrefs::update_transaction_with_post_commit(
            |prefs| {
                prefs.code_permission.ask_on_guardrail_default = enabled;
                Ok(())
            },
            || self.code_permission.write().ask_on_guardrail_default = enabled,
        )?;
        Ok(self.code_permission_prefs_view())
    }
}

/// Boot 清扫的存在性判定（round-11 评审）：与
/// `SessionStore::durable_session_record_is_absent` 同一纪律——只有 NotFound
/// 才算「会话记录已删」，任何瞬时 stat 故障（EIO/EACCES/ENOTDIR）按存活处理
/// （unknown is never absent in a destructive path）。`Path::is_file()` 对一切
/// stat 故障都返回 false，会把活会话的显式 mode / ask 侧车当幽灵删掉。
fn session_record_absent_for_sweep(sessions_dir: &std::path::Path, id: &str) -> bool {
    matches!(
        std::fs::metadata(sessions_dir.join(format!("{id}.json"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

#[cfg(test)]
mod sweep_probe_tests {
    use super::session_record_absent_for_sweep;

    /// NotFound → 幽灵（可清扫）；记录存在 → 存活；其余 stat 故障 → 按存活
    /// 处理（fail-closed：清扫宁漏不误删）。非 NotFound 故障臂用含 NUL 的
    /// 会话 id 确定性构造：std 在各平台自己的路径转换里拒绝内部 NUL（Unix
    /// 走 `run_with_cstr`、Windows 走 WTF-8→UTF-16 的 `to_u16s`，两处都在
    /// 任何 OS 调用之前产出 `InvalidInput`）——ENOTDIR 形状（「sessions_dir
    /// 本身是一个文件」）是 POSIX 专属的，Windows 把「路径组件是文件」映射
    /// 为 NotFound，无法在该平台表达同一故障。
    #[test]
    fn sweep_absence_probe_is_not_found_only() {
        let root = std::env::temp_dir().join(format!(
            "pinvou-sweep-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        std::fs::write(root.join("live.json"), b"{}").expect("live record");

        assert!(session_record_absent_for_sweep(&root, "ghost000000000000"));
        assert!(!session_record_absent_for_sweep(&root, "live"));
        assert!(!session_record_absent_for_sweep(
            &root,
            "transient\u{0}fault"
        ));
        std::fs::remove_dir_all(&root).ok();
    }
}
