//! 工作流状态机 — 对话状态跟踪 + 里程碑导航。
//!
//! 不是硬性阶段 gate，而是可选侧边栏建议。

#![allow(dead_code)] // Phase 1 定义，Phase 2 使用

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::contract::MilestoneContract;

/// 里程碑 —— 一段任务编排单元。
///
/// 由 `CombinedPlanner` 拆解生成（每个 milestone 选一个 `MilestoneMode`），
/// 也可能是 fallback 计划中的静态项。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Milestone {
    /// 唯一标识（在一次会话内不重复）
    pub id: String,
    /// 用户可见的中文标签
    pub label: String,
    /// LLM 阶段提示（行为引导，由拆解器填）
    #[serde(default)]
    pub prompt_hint: Option<String>,
    /// 可选 UI 图标
    #[serde(default)]
    pub icon: Option<String>,
    /// 阶段契约（由 mode → 内置规则 + 拆解器选的工具组合而成）
    #[serde(default)]
    pub contract: MilestoneContract,
}

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

/// 全局会话状态（取代旧的"必有 app"假设）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalMode {
    /// 简单问答：无 milestone，纯流式对话
    QnA,
    /// 动态拆解中（短暂）
    Planning,
    /// 按 milestone 推进
    Executing,
    /// 用户触发 /replan 重拆解中
    Replan,
    /// 全部完成
    Done,
}

impl Default for GlobalMode {
    fn default() -> Self {
        // 旧 checkpoint 默认按 Executing 还原（保留行为）
        GlobalMode::Executing
    }
}

/// 对话状态 — 贯穿一次应用会话的核心状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationState {
    /// 当前加载的应用 ID
    pub app_id: String,
    /// 当前选定的 agent ID（None 表示未初始化或 Q&A 模式）
    #[serde(default)]
    pub agent_id: Option<String>,
    /// 全局会话模式
    #[serde(default)]
    pub global_mode: GlobalMode,
    /// 各里程碑状态
    pub milestones: Vec<(Milestone, MilestoneStatus)>,
    /// 对话中累积的上下文（键值对）
    ///
    /// 例如:
    /// - 数据分析: {"columns": "...", "row_count": "1200", "file_path": "sales.csv"}
    /// - 文档生成: {"doc_type": "周报", "collected_info": "..."}
    /// - 计划敲定: {"options": "[A, B]", "constraints": "..."}
    pub context: HashMap<String, String>,
    /// 每个 context key 由哪个 milestone 产出（用于精准回退清理）
    #[serde(default)]
    pub context_attribution: HashMap<String, String>,
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
            agent_id: None,
            global_mode: GlobalMode::Executing,
            milestones: milestone_states,
            context: HashMap::new(),
            context_attribution: HashMap::new(),
            plan_initialized: false,
            question_counts: HashMap::new(),
            turn_count: 0,
            current_phase: None,
        }
    }

    /// 创建 Q&A 模式的会话（无 milestone）
    pub fn new_qa(agent_id: impl Into<String>) -> Self {
        Self {
            app_id: String::new(),
            agent_id: Some(agent_id.into()),
            global_mode: GlobalMode::QnA,
            milestones: Vec::new(),
            context: HashMap::new(),
            context_attribution: HashMap::new(),
            plan_initialized: true,
            question_counts: HashMap::new(),
            turn_count: 0,
            current_phase: None,
        }
    }

    /// 设置 agent_id
    pub fn set_agent(&mut self, agent_id: impl Into<String>) {
        self.agent_id = Some(agent_id.into());
    }

    /// 设置 context 项并记录产出方
    pub fn set_context_with_origin(
        &mut self,
        key: impl Into<String>,
        value: impl Into<String>,
        produced_by: impl Into<String>,
    ) {
        let key = key.into();
        self.context.insert(key.clone(), value.into());
        self.context_attribution.insert(key, produced_by.into());
    }

    /// 清除归属于某个 milestone 的所有 context（用于 /redo）
    pub fn clear_context_by_milestone(&mut self, milestone_id: &str) {
        let to_remove: Vec<String> = self
            .context_attribution
            .iter()
            .filter(|(_, owner)| owner.as_str() == milestone_id)
            .map(|(k, _)| k.clone())
            .collect();
        for k in to_remove {
            self.context.remove(&k);
            self.context_attribution.remove(&k);
        }
    }

    /// 清除归属于一组 milestone 的所有 context（用于 /back）
    pub fn clear_context_by_milestones(&mut self, milestone_ids: &[String]) {
        let to_remove: Vec<String> = self
            .context_attribution
            .iter()
            .filter(|(_, owner)| milestone_ids.iter().any(|id| id == owner.as_str()))
            .map(|(k, _)| k.clone())
            .collect();
        for k in to_remove {
            self.context.remove(&k);
            self.context_attribution.remove(&k);
        }
    }

    /// 回退到指定 milestone：将它及之后的 Done 标回 Active/Pending，并清相应 context
    ///
    /// - 目标 milestone 标 Active
    /// - 目标之后的 Done 标 Pending
    /// - 当前 Active 标 Pending（如果不在目标之前）
    pub fn rewind_to(&mut self, milestone_id: &str) -> bool {
        let target_idx = self.milestones.iter().position(|(m, _)| m.id == milestone_id);
        let Some(target_idx) = target_idx else {
            return false;
        };

        let mut affected_ids: Vec<String> = Vec::new();
        for (i, (m, status)) in self.milestones.iter_mut().enumerate() {
            match i.cmp(&target_idx) {
                std::cmp::Ordering::Less => { /* 保持原状 */ }
                std::cmp::Ordering::Equal => {
                    affected_ids.push(m.id.clone());
                    *status = MilestoneStatus::Active;
                }
                std::cmp::Ordering::Greater => {
                    affected_ids.push(m.id.clone());
                    *status = MilestoneStatus::Pending;
                }
            }
        }

        // 清除受影响 milestone 的 context 和提问计数
        self.clear_context_by_milestones(&affected_ids);
        for id in &affected_ids {
            self.question_counts.remove(id);
        }
        true
    }

    /// 跳过当前活跃 milestone（标 Skipped 并推进）
    pub fn skip_active(&mut self) -> Option<String> {
        let active_id = self.active_milestone().map(|m| m.id.clone())?;
        self.skip(&active_id);
        Some(active_id)
    }

    /// 当前活跃 milestone 重做：清自身 context，状态仍为 Active
    pub fn redo_active(&mut self) -> Option<String> {
        let active_id = self.active_milestone().map(|m| m.id.clone())?;
        self.clear_context_by_milestone(&active_id);
        self.question_counts.remove(&active_id);
        Some(active_id)
    }

    /// 在指定 milestone 之前插入新 milestone。
    ///
    /// 主要用于 Review tweak 路径：动态插入 `PatchOutput` milestone 让 LLM
    /// 做精确局部修订，不重写整个 final_output。
    ///
    /// 行为：
    /// - 新 milestone 状态设为 `Active`
    /// - 目标 milestone（review）若是 `Active` 改 `Pending` —— patch 完成后会再次轮到它
    /// - 其他 milestone 状态不变
    ///
    /// 返回 true = 找到目标并完成插入；false = target_id 不存在。
    pub fn insert_milestone_before(&mut self, target_id: &str, new_ms: Milestone) -> bool {
        let target_idx = self
            .milestones
            .iter()
            .position(|(m, _)| m.id == target_id);
        let Some(idx) = target_idx else {
            return false;
        };

        // 目标若是 Active，改 Pending（patch 完成后会重新成为 Active）
        if let Some((_, status)) = self.milestones.get_mut(idx) {
            if matches!(status, MilestoneStatus::Active) {
                *status = MilestoneStatus::Pending;
            }
        }

        // 插入新 milestone，状态 Active
        self.milestones
            .insert(idx, (new_ms, MilestoneStatus::Active));
        true
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
        assert_eq!(state.agent_id, None);
        assert!(state.context_attribution.is_empty());
        assert_eq!(state.global_mode, GlobalMode::Executing); // 默认值
    }

    // === 新增：context attribution + rewind 测试 ===

    #[test]
    fn set_context_with_origin_records_attribution() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        state.set_context_with_origin("structure", "三段式", "a");
        assert_eq!(state.context.get("structure").unwrap(), "三段式");
        assert_eq!(state.context_attribution.get("structure").unwrap(), "a");
    }

    #[test]
    fn clear_context_by_milestone_only_removes_owned_keys() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        state.set_context_with_origin("k1", "v1", "a");
        state.set_context_with_origin("k2", "v2", "b");
        state.set_context_with_origin("k3", "v3", "a");

        state.clear_context_by_milestone("a");

        assert!(!state.context.contains_key("k1"));
        assert!(state.context.contains_key("k2"));
        assert!(!state.context.contains_key("k3"));
        assert!(!state.context_attribution.contains_key("k1"));
        assert!(state.context_attribution.contains_key("k2"));
    }

    #[test]
    fn rewind_to_restores_active_and_clears_later_context() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        // 走完 a 和 b
        state.set_context_with_origin("ka", "va", "a");
        state.mark_done("a");
        state.set_context_with_origin("kb", "vb", "b");
        state.mark_done("b");
        // 此时 c 是 Active
        assert_eq!(state.milestones[2].1, MilestoneStatus::Active);

        // 回退到 a
        assert!(state.rewind_to("a"));
        assert_eq!(state.milestones[0].1, MilestoneStatus::Active);
        assert_eq!(state.milestones[1].1, MilestoneStatus::Pending);
        assert_eq!(state.milestones[2].1, MilestoneStatus::Pending);

        // a 自身的 context 被清，因为它也是受影响的（Active）
        // b 的 context 被清
        assert!(!state.context.contains_key("ka"));
        assert!(!state.context.contains_key("kb"));
    }

    #[test]
    fn rewind_to_unknown_milestone_returns_false() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        assert!(!state.rewind_to("nonexistent"));
    }

    fn make_milestone(id: &str, label: &str) -> Milestone {
        Milestone {
            id: id.into(),
            label: label.into(),
            prompt_hint: None,
            icon: None,
            contract: crate::contract::MilestoneContract::default(),
            ..Default::default()
        }
    }

    #[test]
    fn insert_before_active_pushes_active_to_pending_and_takes_active() {
        // 场景：[a:Done, b:Active, c:Pending]，在 b 之前插入 patch
        // 期望：[a:Done, patch:Active, b:Pending, c:Pending]
        let mut state = ConversationState::new("test".into(), sample_milestones());
        state.mark_done("a"); // 现在 b 是 Active
        let patch = make_milestone("patch", "局部修订");
        let ok = state.insert_milestone_before("b", patch);
        assert!(ok);

        // 注意 sample_milestones 顺序：a/b/c
        assert_eq!(state.milestones[0].0.id, "a");
        assert_eq!(state.milestones[0].1, MilestoneStatus::Done);
        assert_eq!(state.milestones[1].0.id, "patch");
        assert_eq!(state.milestones[1].1, MilestoneStatus::Active);
        assert_eq!(state.milestones[2].0.id, "b");
        assert_eq!(state.milestones[2].1, MilestoneStatus::Pending,
            "b 原本是 Active，被 patch 顶到前面后应改 Pending");
        assert_eq!(state.milestones[3].0.id, "c");
    }

    #[test]
    fn insert_before_unknown_target_returns_false() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        let patch = make_milestone("patch", "");
        assert!(!state.insert_milestone_before("nonexistent", patch));
        assert_eq!(state.milestones.len(), 3, "失败时不修改列表");
    }

    #[test]
    fn insert_before_pending_target_doesnt_change_target_status() {
        // 目标若是 Pending，插入新 Active 后目标仍 Pending（不该被改）
        let mut state = ConversationState::new("test".into(), sample_milestones());
        // a 还是 Active，b 是 Pending
        let patch = make_milestone("patch", "");
        let ok = state.insert_milestone_before("b", patch);
        assert!(ok);
        // 现在有两个 Active（a 和 patch）—— 这是允许的，因为 active_milestone()
        // 返回第一个 Active，patch 不在最前
        // 实际场景中 review tweak 是在 review (Active) 之前插入，原 Active 会
        // 转 Pending，所以本测试的"两 Active"是 corner case 不会发生。
        assert_eq!(state.milestones[1].0.id, "patch");
        assert_eq!(state.milestones[1].1, MilestoneStatus::Active);
        assert_eq!(state.milestones[2].0.id, "b");
        assert_eq!(state.milestones[2].1, MilestoneStatus::Pending,
            "b 本来就是 Pending，插入操作不改它");
    }

    #[test]
    fn skip_active_advances_to_next() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        let skipped = state.skip_active().unwrap();
        assert_eq!(skipped, "a");
        assert_eq!(state.milestones[0].1, MilestoneStatus::Skipped);
        assert_eq!(state.milestones[1].1, MilestoneStatus::Active);
    }

    #[test]
    fn redo_active_clears_own_context_but_keeps_prior() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        state.set_context_with_origin("ka", "va", "a");
        state.mark_done("a"); // 现在 b 是 Active
        state.set_context_with_origin("kb", "vb", "b");
        state.increment_question_count("b");

        let redone = state.redo_active().unwrap();
        assert_eq!(redone, "b");
        assert_eq!(state.milestones[1].1, MilestoneStatus::Active); // 仍是 Active
        assert!(state.context.contains_key("ka")); // a 的不动
        assert!(!state.context.contains_key("kb")); // b 的清
        assert_eq!(state.question_count("b"), 0); // 计数清零
    }

    #[test]
    fn qa_constructor_creates_state_with_no_milestones() {
        let state = ConversationState::new_qa("qa");
        assert_eq!(state.global_mode, GlobalMode::QnA);
        assert_eq!(state.agent_id.as_deref(), Some("qa"));
        assert!(state.milestones.is_empty());
        assert!(state.plan_initialized);
    }
}
