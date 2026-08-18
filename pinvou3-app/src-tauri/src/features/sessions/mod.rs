//! 多对话管理 wrapper（facade + 子模块）。
//!
//! 复用 deepseek-tui 上游 [`SessionManager`]（已支持 `new(custom_dir)`），
//! 把 sessions 目录定向到 `~/.pinvou3/sessions/`（隔离 `~/.deepseek/`）。
//!
//! 暴露给 pinvou3-app Tauri commands 的能力：
//! - `list` —— 列出所有会话元数据（前端历史面板）
//! - `create_new` —— 新建空会话（首次未发送消息前）
//! - `load` —— 读完整对话（切换 session 时给 engine 通过 `Op::SyncSession` 注入）
//! - `save` —— 持久化（每轮 turn 完成 auto-save）
//! - `delete` —— 删除会话 + artifacts 目录
//! - `set_title` —— 重命名
//! - `active_id` / `set_active` —— 跟踪当前 active session（chat command 用）
//!
//! **Arc + RwLock 包装**：所有字段都是 `Arc`，整个 `SessionStore` 可以
//! 廉价 Clone 进 Tauri State + 多个 task 共享。
//!
//! 历史上本文件是 3700+ 行的 god-module，混了 7 类职责。Wave 2 任务 2d 把它
//! 拆成 facade（本文件，保留 struct 定义 + 常量）+ 子模块：
//!
//! - [`store`] —— 会话存储 CRUD / 生命周期 / engine-state 持久化入口
//! - [`scheduled`] —— 定时运行 profile / engine-state 类型与 registry
//! - [`retention`] —— 保留策略与 `persist_then_reconcile` 系列 helper
//! - [`transcript`] —— transcript revision / 截断保护
//! - [`mode_state`] —— per-session 模式状态机（mode/plan/persona/skill）
//! - [`injections`] —— 一次性注入与 plan-claim 的事务 checkout guard
//! - [`sidecars`] —— skill 绑定 / 模型 / 置顶 / 收起 的独立 sidecar 落盘
//! - [`validators`] —— id / workspace / 路径校验与小型 helper
//!
//! 子模块通过 `impl SessionStore` 续写方法（Rust 允许同一 struct 的 impl 块
//! 散布在子模块里），并直接读 `&self` 的私有字段——struct 字段对后代模块
//! 可见。pub 面在本文件集中 re-export，保持外部调用路径不变。

mod injections;
pub(crate) mod mode_state;
mod retention;
mod scheduled;
mod sidecars;
mod store;
mod transcript;
mod validators;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

pub use crate::core::mode_state::{ModeLane, SerializableMode};
use crate::platform::paths;
use crate::platform::prefs::{CodePermissionPrefs, ModeDefaultPrefs};

/// Re-export the session-domain mode-state types so they are owned by the
/// sessions feature. These were historically re-exported through a `core`
/// shim (`core::mode_state`), but they are feature-domain aggregates
/// (persona/review/knowledge) and must not form a `core → features`
/// reverse dependency. Consumers should import from
/// `crate::features::sessions::{...}`.
pub use self::mode_state::{
    MountedCollection, MountedCollectionsSnapshot, MountedRemoteCollection, SessionModeState,
};
/// Re-export scheduled-run types so the historical
/// `crate::features::sessions::X` paths stay stable.
pub use self::scheduled::{
    ChatEngineState, ScheduledEngineState, ScheduledRunMode, ScheduledRunProfile,
    ScheduledTokenAccounting,
};
/// Re-export transcript helpers (consumed across engine / remote-control).
pub use self::transcript::transcript_revision;
/// Re-export the crate-visible session-id validator (used by commands). It is
/// `pub(crate)` so it stays out of the crate's public API surface.
pub(crate) use self::validators::validate_session_id;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Chat,
    ScheduledRun,
}

/// pinvou3 session 存储：包 SessionManager + active id 跟踪 + per-session mode 状态。
///
/// mode 状态分两层（三分 lane 语义，复审拍板）：
/// - per-session：任何会话显式切 mode（`set_mode`）都持久化到
///   `~/.pinvou3/sessions/_session_mode_states.json`（重开会话恢复它自己上次的
///   mode）；已生成会话的切换**不再**触碰任何全局默认。
/// - 全局默认按 lane 三分：工作/设计 → settings.json `mode_defaults.work/design`，
///   代码 → `code_permission.last_mode`（从未用过 code 模式 → Plan 只读）。
///   只由对应 lane **草稿态**的显式切换写入（`set_mode_default`）；新会话默认
///   mode = 本 lane 全局默认（code 由 `resolved_default_mode` 解析，work/design
///   由前端在会话物化时应用）。yolo 一次性确认标志
///   `code_permission.yolo_confirmed` 同样在 settings.json，由确认命令写入。
/// 其余运行时交互状态（pending_plan、persona、知识库挂载等）仍 in-memory only。
///
/// `auto_continue_count`：M2 弱模型加固——Executing 态 LLM 调一次工具就停时,
/// bridge 自动 send "继续"消息驱动 agent loop。每个用户主动消息重置为 0,
#[derive(Clone)]
pub struct SessionStore {
    /// Underlying upstream session manager (durable JSON store).
    pub(crate) manager: Arc<deepseek_tui::session_manager::SessionManager>,
    /// Scheduled-run profiles keyed by session id (`sched-*`).
    pub(crate) scheduled_profiles: Arc<RwLock<HashMap<String, ScheduledRunProfile>>>,
    /// Path to the scheduled-profile registry JSON.
    pub(crate) scheduled_profiles_path: Arc<PathBuf>,
    /// Root for scheduled-task workspaces (`<root>/<task_id>/workspace`).
    pub(crate) scheduled_root: Arc<PathBuf>,
    /// Coarse mutation lock for scheduled create / delete / reconcile.
    pub(crate) scheduled_mutation: Arc<Mutex<()>>,
    /// Currently active session id (chat command surface).
    pub(crate) active: Arc<RwLock<Option<String>>>,
    /// Per-session runtime mode state (mode, plan, persona, ...).
    pub(crate) mode_states: Arc<RwLock<HashMap<String, SessionModeState>>>,
    /// per-session 模型绑定:session_id → SavedModel.id。某 session 显式选过模型
    /// 才有条目;没选的回退全局 active_model_id。落盘到 `_session_models.json`
    /// 底座 SavedSession 不能加字段，故通过独立 sidecar 存储。
    pub(crate) session_models: Arc<RwLock<HashMap<String, String>>>,
    /// 历史对话置顶表:session_id -> pinned_at。独立落盘到 `_pinned_sessions.json`,
    /// 不改 SavedSession 结构。
    pub(crate) pinned_sessions: Arc<RwLock<HashMap<String, String>>>,
    /// 从左侧任务列表收起的会话:session_id -> hidden_at。独立落盘到
    /// `_hidden_sessions.json`,不改 SavedSession 结构。
    pub(crate) hidden_sessions: Arc<RwLock<HashMap<String, String>>>,
    /// 原生代码会话绑定的项目目录解析器,由 app 组合根(lib.rs)在 AcpPool 就绪
    /// 后注入;None = 无代码会话项目绑定,所有会话的执行根都是会话私有目录。
    /// 账本根(附件/审计/产物/远程授权)不受其影响,恒为会话私有目录。
    pub(crate) execution_root_resolver: Arc<RwLock<Option<ExecutionRootResolver>>>,
    /// 品悟原生 code 会话判定（ACP 会话恒为 plain，见 codex_acp store）。
    /// 与 Engine bridge / 远程端共用同一份 `SessionAgentStore` 闭包，由 app 组合根
    /// (lib.rs) 注入；None = 无 code 会话判定（测试/启动早期），全部按 plain 语义。
    code_session_predicate: Arc<RwLock<Option<CodeSessionPredicate>>>,
    /// `_session_mode_states.json` 的内存事实源：存所有会话的显式 mode（三分
    /// lane 语义后 plain 会话也持久化）。启动时 load 合并进 `mode_states`；
    /// set_mode / 删除会话 / 保留策略清理时维护并落盘。
    session_mode_states: Arc<RwLock<HashMap<String, SerializableMode>>>,
    /// settings.json `code_permission` 的进程内镜像。`mode_state` 在 chat 发送
    /// 路径上每轮被调，默认值解析只读这块内存（加锁读，不触盘）；写入经
    /// `UserPrefs::update_transaction` 落盘后同步本镜像。
    code_permission: Arc<RwLock<CodePermissionPrefs>>,
    /// settings.json `mode_defaults`（work/design lane 全局默认）的进程内镜像，
    /// 与 `code_permission` 同款镜像语义。
    mode_defaults: Arc<RwLock<ModeDefaultPrefs>>,
    /// `_multi_agent.json` 的持久化互斥：内存快照与 tmp+rename 必须在同一临界
    /// 区内完成。少了它，两个并发保存会各自读到不同时刻的快照，**后完成写盘的
    /// 旧快照**会覆盖新快照——重启后部分会话的开关状态消失。
    multi_agent_flags_io: Arc<Mutex<()>>,
}

/// 原生代码会话(品悟 Engine)的执行根解析器:绑定了项目目录的原生代码会话
/// 返回 `Some(项目目录)`;其余会话返回 `None`,调用方回退到会话私有目录。
///
/// 用闭包而非直接依赖 `codex_acp::SessionAgentStore`:`sessions` 与 `codex_acp`
/// 两个 feature 互相引用会成环,解析器由 app 组合根(lib.rs)注入并共享 AcpPool
/// 持有的同一份 store(clone 共享 Arc,运行时读到最新绑定)。
pub type ExecutionRootResolver = Arc<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// 品悟原生 code 会话判定闭包：与 `ExecutionRootResolver` 同样的注入理由
/// （避免 sessions ↔ codex_acp 成环），由 lib.rs 共享同一份 `SessionAgentStore`。
/// ACP 会话在其 store 里恒为 plain（`bind_*` 时显式重置），故本判定命中即
/// "品悟原生 code 会话"，不会误伤 ACP 会话自己的权限模式。
pub type CodeSessionPredicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// 一个会话的两个根:
/// - `execution`:Engine cwd / shell 执行目录。绑了项目目录的原生代码会话 = 项目
///   目录;其余会话 = 会话私有目录(scheduled 会话 = 其 automation workspace)。
/// - `ledger`:应用账本根(附件/审计/产物/远程授权)。绑了项目目录的原生代码会话
///   恒为会话私有目录(不污染用户项目);其余会话与 execution 相同。
///
/// 由 [`SessionStore::session_roots`] 统一解析,调用方按用途显式选择用哪个根,
/// 避免把执行根误当账本根写盘(或反之)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRoots {
    pub execution: PathBuf,
    pub ledger: PathBuf,
}

/// 两个根的纯解析:给定原生代码会话绑定的项目目录(无绑定传 `None`),返回
/// 执行根与账本根。不感知 scheduled 会话——scheduled 的两个根都是其
/// automation workspace,由 [`SessionStore::session_roots`] 在上层处理。
pub fn session_roots_for(session_id: &str, bound_project_root: Option<PathBuf>) -> SessionRoots {
    let private = paths::session_workspace_dir(session_id);
    match bound_project_root {
        Some(project) => SessionRoots {
            execution: project,
            ledger: private,
        },
        None => SessionRoots {
            execution: private.clone(),
            ledger: private,
        },
    }
}
