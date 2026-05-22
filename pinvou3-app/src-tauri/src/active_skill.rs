//! 工作流 Phase 可视化 MVP1 — 已激活 skill 的 instruction 缓冲。
//!
//! 用户在「工作流」视图点「启用」一个有 phases 的 skill (例如 h3c-ppt) →
//! `commands::set_active_skill` 把 skill.body + phase 追踪规则文案存到这里 →
//! 下次 `engine::send_user_message` 之前 prepend 到 content 前 (one-shot,
//! 后续 turn 靠 LLM session 上下文保持)。
//!
//! 设计要点:
//! - 不动 DeepSeek-TUI 底座 `app.active_skill` 字段 (EngineHandle 模式接不到
//!   `&mut App`),改成 pinvou3-app 在 send 切点自己 prepend。
//! - 切 skill 时 `set_active_skill(Some(new))` 覆写 / `None` 清空,前后端
//!   phase 状态同步 reset (前端 deltaBuffer / reachedPhaseIds 也清)。
//! - 全局单例 (Tauri State),MVP1 不做 per-session;多 session 共用同一个
//!   active_skill 是已知 trade-off (UI 提示)。

use parking_lot::Mutex;

#[derive(Default)]
pub struct ActiveSkillStore {
    inner: Mutex<Option<ActiveSkill>>,
}

#[derive(Debug, Clone)]
pub struct ActiveSkill {
    pub name: String,
    /// skill.body 拼上 phase 追踪规则文案的完整 instruction。下次 send 时
    /// prepend 到 user content 前,只 prepend 一次,后续 turn 不重复。
    pub injected_instruction: String,
    /// 是否已经 prepend 过。`set_active_skill` 时 false,
    /// `engine::send_user_message` 用了之后 true。
    pub already_sent: bool,
}

impl ActiveSkillStore {
    pub fn set(&self, skill: ActiveSkill) {
        *self.inner.lock() = Some(skill);
    }

    pub fn clear(&self) {
        *self.inner.lock() = None;
    }

    /// 取出当次 send 需要 prepend 的 instruction (consume 一次性标记)。
    /// 若 active_skill 为空或 already_sent=true 返回 None — 调用方原样发。
    pub fn take_pending_instruction(&self) -> Option<String> {
        let mut guard = self.inner.lock();
        let skill = guard.as_mut()?;
        if skill.already_sent {
            return None;
        }
        skill.already_sent = true;
        Some(skill.injected_instruction.clone())
    }

    pub fn current_name(&self) -> Option<String> {
        self.inner.lock().as_ref().map(|s| s.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_then_take_returns_once_then_none() {
        let store = ActiveSkillStore::default();
        store.set(ActiveSkill {
            name: "h3c-ppt".into(),
            injected_instruction: "INJECTED".into(),
            already_sent: false,
        });
        assert_eq!(store.take_pending_instruction().as_deref(), Some("INJECTED"));
        assert_eq!(store.take_pending_instruction(), None);
    }

    #[test]
    fn clear_removes_skill() {
        let store = ActiveSkillStore::default();
        store.set(ActiveSkill {
            name: "x".into(),
            injected_instruction: "I".into(),
            already_sent: false,
        });
        assert_eq!(store.current_name().as_deref(), Some("x"));
        store.clear();
        assert!(store.current_name().is_none());
        assert_eq!(store.take_pending_instruction(), None);
    }

    #[test]
    fn set_overwrites_and_resets_already_sent() {
        let store = ActiveSkillStore::default();
        store.set(ActiveSkill {
            name: "a".into(),
            injected_instruction: "A".into(),
            already_sent: true, // 模拟已发过
        });
        // 切到新 skill 应该能 prepend 新 instruction
        store.set(ActiveSkill {
            name: "b".into(),
            injected_instruction: "B".into(),
            already_sent: false,
        });
        assert_eq!(store.take_pending_instruction().as_deref(), Some("B"));
    }
}
