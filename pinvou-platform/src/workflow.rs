//! 工作流状态机 — 对话状态跟踪 + 里程碑导航。
//!
//! 不是硬性阶段 gate，而是可选侧边栏建议。

#![allow(dead_code)] // Phase 1 定义，Phase 2 使用

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::app::Milestone;

/// 里程碑状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MilestoneStatus {
    /// 未开始
    Pending,
    /// 当前活跃（侧边栏高亮）
    Active,
    /// 已完成
    Done,
    /// 已跳过
    Skipped,
}

/// 对话状态 — 贯穿一次应用会话的核心状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationState {
    /// 当前加载的应用 ID
    pub app_id: String,
    /// 各里程碑状态
    pub milestones: Vec<(Milestone, MilestoneStatus)>,
    /// 对话中累积的上下文（键值对）
    ///
    /// 例如:
    /// - 数据分析: {"columns": "...", "row_count": "1200", "file_path": "sales.csv"}
    /// - 文档生成: {"doc_type": "周报", "collected_info": "..."}
    /// - 计划敲定: {"options": "[A, B]", "constraints": "..."}
    pub context: HashMap<String, String>,
    /// 是否已经初始化动态计划
    #[serde(default)]
    pub plan_initialized: bool,
    /// 每个里程碑已提问次数
    #[serde(default)]
    pub question_counts: HashMap<String, u8>,
    /// 对话轮数
    pub turn_count: u32,
    /// 当前阶段（用于 prompt 注入）
    pub current_phase: Option<String>,
}

impl ConversationState {
    /// 从应用配置创建初始状态
    pub fn new(app_id: String, milestones: Vec<Milestone>) -> Self {
        let milestone_states: Vec<_> = milestones
            .into_iter()
            .enumerate()
            .map(|(i, m)| {
                let status = if i == 0 {
                    MilestoneStatus::Active
                } else {
                    MilestoneStatus::Pending
                };
                (m, status)
            })
            .collect();

        Self {
            app_id,
            milestones: milestone_states,
            context: HashMap::new(),
            plan_initialized: false,
            question_counts: HashMap::new(),
            turn_count: 0,
            current_phase: None,
        }
    }

    /// 标记里程碑为完成，激活下一个
    pub fn mark_done(&mut self, milestone_id: &str) {
        self.update_milestone(milestone_id, MilestoneStatus::Done);
        self.activate_next();
    }

    /// 跳过里程碑
    pub fn skip(&mut self, milestone_id: &str) {
        self.update_milestone(milestone_id, MilestoneStatus::Skipped);
        self.activate_next();
    }

    /// 更新上下文
    pub fn set_context(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.context.insert(key.into(), value.into());
    }

    /// 增加轮数
    pub fn increment_turn(&mut self) {
        self.turn_count += 1;
    }

    /// 获取指定里程碑已提问次数
    pub fn question_count(&self, milestone_id: &str) -> u8 {
        self.question_counts.get(milestone_id).copied().unwrap_or(0)
    }

    /// 增加指定里程碑提问次数
    pub fn increment_question_count(&mut self, milestone_id: &str) {
        let entry = self
            .question_counts
            .entry(milestone_id.to_string())
            .or_insert(0);
        *entry = entry.saturating_add(1);
    }

    /// 获取当前活跃里程碑
    pub fn active_milestone(&self) -> Option<&Milestone> {
        self.milestones
            .iter()
            .find(|(_, s)| *s == MilestoneStatus::Active)
            .map(|(m, _)| m)
    }

    /// 获取下一步建议列表（供侧边栏显示）
    pub fn suggestions(&self) -> Vec<MilestoneSuggestion> {
        self.milestones
            .iter()
            .map(|(m, s)| MilestoneSuggestion {
                id: m.id.clone(),
                label: m.label.clone(),
                icon: m.icon.clone(),
                status: s.clone(),
            })
            .collect()
    }

    /// 构建注入 prompt 的上下文文本
    pub fn context_prompt(&self) -> Option<String> {
        if self.context.is_empty() {
            return None;
        }
        let items: Vec<_> = self
            .context
            .iter()
            .map(|(k, v)| format!("- **{k}**: {v}"))
            .collect();
        Some(format!(
            "## 已知上下文\n\n以下是从对话中已获知的信息：\n\n{}",
            items.join("\n")
        ))
    }

    /// 构建当前阶段提示
    pub fn phase_prompt(&self) -> Option<String> {
        let active = self.active_milestone()?;
        let hint = active.prompt_hint.as_ref()?;
        Some(format!(
            "当前阶段是「{}」，已获取的上下文可参考。提示: {hint}",
            active.label
        ))
    }

    // --- private ---

    fn update_milestone(&mut self, id: &str, status: MilestoneStatus) {
        for (m, s) in &mut self.milestones {
            if m.id == id {
                *s = status;
                break;
            }
        }
    }

    fn activate_next(&mut self) {
        // 先将当前 Active 翻为 Done（可能已被 update_milestone 处理过）
        for (_, status) in &mut self.milestones {
            if *status == MilestoneStatus::Active {
                *status = MilestoneStatus::Done;
                break;
            }
        }
        // 再激活第一个 Pending
        for (_, status) in &mut self.milestones {
            if *status == MilestoneStatus::Pending {
                *status = MilestoneStatus::Active;
                break;
            }
        }
    }
}

/// 侧边栏建议项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MilestoneSuggestion {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    pub status: MilestoneStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_milestones() -> Vec<Milestone> {
        vec![
            Milestone {
                id: "a".into(),
                label: "Step A".into(),
                prompt_hint: None,
                icon: None,
                ..Default::default()
            },
            Milestone {
                id: "b".into(),
                label: "Step B".into(),
                prompt_hint: None,
                icon: None,
                ..Default::default()
            },
            Milestone {
                id: "c".into(),
                label: "Step C".into(),
                prompt_hint: None,
                icon: None,
                ..Default::default()
            },
        ]
    }

    #[test]
    fn test_initial_state() {
        let state = ConversationState::new("test".into(), sample_milestones());
        assert_eq!(state.milestones.len(), 3);
        assert_eq!(state.milestones[0].1, MilestoneStatus::Active);
        assert_eq!(state.milestones[1].1, MilestoneStatus::Pending);
    }

    #[test]
    fn test_mark_done_advances() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        state.mark_done("a");
        assert_eq!(state.milestones[0].1, MilestoneStatus::Done);
        assert_eq!(state.milestones[1].1, MilestoneStatus::Active);
    }

    #[test]
    fn test_skip_advances() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        state.skip("a");
        assert_eq!(state.milestones[0].1, MilestoneStatus::Skipped);
        assert_eq!(state.milestones[1].1, MilestoneStatus::Active);
    }

    #[test]
    fn test_context_accumulation() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        state.set_context("columns", "name, age, city");
        state.set_context("row_count", "1200");
        assert!(state.context_prompt().is_some());
    }

    #[test]
    fn test_question_count_defaults_to_zero_and_increments() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        assert!(!state.plan_initialized);
        assert_eq!(state.question_count("a"), 0);

        state.increment_question_count("a");
        state.increment_question_count("a");

        assert_eq!(state.question_count("a"), 2);
        assert_eq!(state.question_count("missing"), 0);
    }

    #[test]
    fn test_increment_question_count_saturates_at_u8_max() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        state.question_counts.insert("a".into(), u8::MAX);

        state.increment_question_count("a");

        assert_eq!(state.question_count("a"), u8::MAX);
    }

    #[test]
    fn test_old_checkpoint_json_defaults_runtime_bookkeeping() {
        let json = r#"{
            "app_id": "test",
            "milestones": [],
            "context": {},
            "turn_count": 3,
            "current_phase": null
        }"#;

        let state: ConversationState = serde_json::from_str(json).unwrap();

        assert!(!state.plan_initialized);
        assert!(state.question_counts.is_empty());
    }
}
