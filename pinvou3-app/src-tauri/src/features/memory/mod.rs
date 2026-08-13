//! pinvou3 user memory P0: profile storage + per-session runtime prompt.
//!
//! The structured files are the source of truth. `runtime/<session_id>.md` is
//! only a prompt cache consumed through `InstructionSource::File`.
// architecture-guard: allow-target-cfg -- 记忆持久化测试需用 Windows 独占句柄覆盖 ReplaceFileW 恢复路径
//!
//! 历史上本文件是 4750+ 行的 god-module，混了 9 实体 + 4 JSONL store +
//! 2 目录 store + LLM review + 渲染六类职责。Wave 2 任务 2c 把它拆成
//! facade（本文件，集中 pub 面与 re-export）+ 子模块：
//!
//! - [`types`] —— 9 实体 struct/enum、`MemoryReviewModel` trait、常量、字段归一化
//! - [`util`] —— 文本清洗 / stable id / 原子写盘等跨模块底层原语
//! - [`io`] —— profile 单文件 + 4 JSONL store + 2 目录 store 的读写
//! - [`llm_review`] —— LLM 后台记忆复盘（提示词、调用、清洗、自动落库）+ 启发式兜底
//! - [`render`] —— 注入块 / 设备快照文档 / runtime prompt 文件管理
//!
//! pub 面在本文件集中 re-export，外部 `crate::features::memory::X` 调用路径不变。

mod io;
mod llm_review;
mod render;
mod types;
mod util;

// ---- 实体类型与 trait（types）----
pub use self::types::{
    InjectedMemoryItem, MemoryProfile, MemoryReviewModel, MemoryReviewOutcome, MemorySuggestion,
    MemoryTextPatch, MemoryWriteEvent, NeverMemoryItem, PendingMemoryItem,
    PendingSensitiveIdentity, PreferenceFile, ProfileConventions, ProfileIdentity, ProfilePatch,
    RecentWorkItem, RecentWorkPatch, RuntimeMemorySnapshot, TimedMemoryItem, TopicMutation,
    TopicRead, TurnMemoryCapture, WorkContextFile,
};

// ---- 路径访问器（io）----
pub use self::io::{
    current_focus_path, never_memory_path, pending_memory_path, profile_path, recent_activity_path,
    recent_work_path, runtime_prompt_path, snapshot_path, work_context_dir,
};

// ---- 实体存储读写 pub 入口（io）----
pub use self::io::{
    append_turn_assistant, archive_recent_work, clear_profile, confirm_pending_memory,
    delete_preference, delete_timed_memory, delete_work_context, discard_turn_capture,
    enqueue_memory_candidate, ignore_pending_memory, list_preferences,
    list_preferences_with_cleanup, load_current_focus, load_never_memory, load_pending_memory,
    load_profile, load_recent_activity, load_recent_work, load_work_context,
    load_work_context_with_cleanup, memory_enabled, never_pending_memory,
    record_turn_tool_complete, record_turn_tool_start, record_turn_user, save_profile,
    take_turn_capture, update_preference, update_profile, update_timed_memory, update_work_context,
    upsert_recent_work,
};

// ---- LLM 后台复盘（llm_review）----
pub use self::llm_review::review_turn_candidates_with_llm;

// ---- 渲染 / runtime prompt 文件管理（render）----
pub use self::render::{
    ensure_runtime_prompt, refresh_runtime_prompt, render_memory_block, runtime_snapshot,
    write_memory_snapshot_document,
};

#[cfg(test)]
mod tests;
