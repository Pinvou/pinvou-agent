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
//! - `types` —— 9 实体 struct/enum、`MemoryReviewModel` trait、常量、字段归一化
//! - `util` —— 文本清洗 / stable id / 原子写盘等跨模块底层原语
//! - `io` —— profile 单文件 + 4 JSONL store + 2 目录 store 的读写
//! - `llm_review` —— LLM 后台记忆复盘（提示词、调用、清洗、自动落库）
//! - `organize` —— full-pass memory organize (batch-apply LLM delete/update/merge) + report history
//! - `render` —— 注入块 / 设备快照文档 / runtime prompt 文件管理
//!
//! pub 面在本文件集中 re-export，外部 `crate::features::memory::X` 调用路径不变。

mod io;
mod llm_review;
mod organize;
mod render;
mod types;
mod util;

// ---- 实体类型与 trait（types）----
pub use self::types::{
    InjectedMemoryItem, MemoryProfile, MemoryReviewModel, MemoryReviewOutcome, MemorySuggestion,
    MemoryTextPatch, MemoryWriteEvent, NeverMemoryItem, PendingMemoryItem, PreferenceFile,
    ProfileConventions, ProfileIdentity, ProfilePatch, RecentWorkItem, RuntimeMemorySnapshot,
    TimedMemoryItem, TopicMutation, TopicRead, TurnMemoryCapture, WorkContextFile,
};
// Round-40 review: the CLI's `memory profile set` refuses a label the
// profile normalization would silently empty, by value — a replicated rule
// copy would drift from the store's own normalizer.
pub use self::types::profile_label_would_be_wiped;

// ---- 路径访问器（io）----
// E2E 集成测试（src-tauri/tests/memory_e2e.rs）经 crate 根使用其中一部分，
// 因此这些保持 pub；仅内部使用的访问器（organize_history_path /
// pending_memory_path / never_memory_path）已在 io 内降为 pub(crate)，
// 调用方经 `io::` 路径使用，无需 re-export。
pub use self::io::{
    current_focus_path, profile_path, recent_activity_path, recent_work_path, runtime_prompt_path,
    snapshot_path, work_context_dir,
};

// ---- 实体存储读写 pub 入口（io）----
pub use self::io::{
    PendingIgnoreOutcome, append_turn_assistant, archive_recent_work, confirm_pending_memory,
    delete_preference, delete_timed_memory, delete_work_context, discard_turn_capture,
    enqueue_memory_candidate, ignore_pending_memory, list_preferences,
    list_preferences_with_cleanup, load_current_focus, load_never_memory, load_pending_memory,
    load_profile, load_recent_activity, load_recent_work, load_work_context,
    load_work_context_with_cleanup, memory_enabled, never_pending_memory,
    record_turn_tool_complete, record_turn_tool_start, record_turn_user, take_turn_capture,
    update_preference, update_profile, update_timed_memory, update_work_context,
};

// ---- cross-process organize busy marker (io) ----
// The CLI maps this marker BY VALUE to `memory_organize_busy`; a local copy
// would let the two surfaces' busy codes drift apart.
pub use self::io::ORGANIZE_LOCK_BUSY;

// ---- Stored text length cap (io) ----
// The CLI's `memory add` validation must use the same cap constant as the
// write side; a local copy would reintroduce a spurious
// `memory_add_not_materialized` failure whenever the cap changes.
pub use self::io::WORK_CONTEXT_TEXT_MAX_CHARS;

// ---- stored-text normalization (util) ----
// The CLI predicts the text `memory add` stores for work context and the text
// `memory update` stores in every editable store; those writers normalize it
// with this function, so a local copy would drift into false
// "not materialized" failures.
pub use self::util::clean_candidate_sentence;
// The CLI `memory add` predicts the enqueue normalization (and `update`
// predicts the per-store writer caps) from the original input — the store-
// authoritative contract tests read the store back as the authority, and
// these re-exports keep the CLI from hand-mirroring the values (round-41
// review).
pub use self::io::{PREFERENCE_TEXT_MAX_CHARS, TIMED_TEXT_MAX_CHARS};
pub use self::util::clean_text;
// Round-45 review: the CLI's content-entry lanes (`memory add`, `update`,
// `pending never --reason`) refuse text carrying a memory-block marker —
// the same boundary the organize validator and review sanitizer enforce —
// instead of storing it into the model-visible block verbatim.
pub use self::util::contains_memory_block_marker;
// The CLI `memory add` rejects profile-shaped preference text before
// enqueueing (the confirm path marks it confirmed but writes nothing), and
// `memory pending confirm` reports that no-op instead of printing success.
pub use self::io::confirmed_pending_memory_is_materialized;
pub use self::types::looks_like_profile_preference_text;

// ---- LLM 后台复盘（llm_review）----
pub use self::llm_review::review_turn_candidates_with_llm;

// ---- Full-pass memory organize (organize) ----
pub use self::organize::{MemoryOrganizeReport, load_organize_history, organize_memory_with_llm};

// ---- 渲染 / runtime prompt 文件管理（render）----
// render_memory_block 仅记忆模块内部（渲染与测试）使用，已在 render 内降为
// pub(crate)，调用方经 `render::` 路径使用，无需 re-export。
pub use self::render::{ensure_runtime_prompt, runtime_snapshot, write_memory_snapshot_document};

#[cfg(test)]
mod tests;
