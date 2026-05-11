//! PlatformEngine — 简化的 agent loop，封装平台上下文注入。
//!
//! 与原有 Engine 的区别:
//! - 不处理 LSP、沙箱、子代理、周期管理
//! - 在每次 LLM 调用前注入: app system prompt + ConversationState 上下文 + 里程碑提示
//! - 更轻量，专门服务于平台模式

use std::path::PathBuf;
use std::sync::Arc;

use super::agent_registry::AgentRegistry;
use super::combined_planner::{CombinedPlanner, PlannedMilestone};
use super::contract::contract_for_mode;
use super::harness::{AgentHarness, ChatRequest, Checkpoint, HistoryMessage, ModelInfo, ToolDef};
use super::workflow::{ConversationState, GlobalMode, Milestone};
use anyhow::Result;

const AWAITING_START_MILESTONE_KEY: &str = "_awaiting_start_milestone";

// === Platform Engine ===

/// 平台引擎 — 包装 AgentHarness 实现，加入平台上下文
pub struct PlatformEngine<H: AgentHarness> {
    /// 底层 agent harness
    pub harness: H,
    /// Agent 注册表（从 prompts/*.md 加载）
    pub agents: Option<Arc<AgentRegistry>>,
    /// 当前对话状态
    pub conv_state: Option<ConversationState>,
    /// 工作目录
    pub workspace: PathBuf,
    /// 连续越界计数器
    pub consecutive_out_of_scope: u32,
    /// 对话消息历史（对标 deepseek-tui Session.messages）
    pub messages: Vec<HistoryMessage>,
}

/// `ensure_combined_plan` 的执行结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CombinedPlanOutcome {
    /// 已经初始化过，本次未触发拆解
    AlreadyPlanned,
    /// 落到 Q&A 路径，没有 milestone
    QaMode { agent_id: String },
    /// 落到场景路径，挂上了 milestones
    ScenarioMode {
        agent_id: String,
        milestone_count: usize,
        used_fallback: bool,
    },
}

/// 用户在 choice card 中提交的一项选择。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserChoiceAnswer {
    pub id: String,
    pub label: String,
    pub value: String,
}

/// 平台层消费用户事件后的状态推进结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MilestoneAdvanceResult {
    pub completed_milestone_id: Option<String>,
    pub completed_milestone_label: Option<String>,
    pub next_milestone_id: Option<String>,
    pub next_milestone_label: Option<String>,
    pub summary: String,
}

impl<H: AgentHarness> PlatformEngine<H> {
    pub fn new(harness: H, workspace: PathBuf) -> Self {
        Self {
            harness,
            agents: None,
            conv_state: None,
            workspace,
            consecutive_out_of_scope: 0,
            messages: Vec::new(),
        }
    }

    /// 注入 AgentRegistry（新设计入口）。
    /// 在调用 `ensure_combined_plan` 前必须先调用此方法。
    pub fn set_agent_registry(&mut self, agents: Arc<AgentRegistry>) {
        self.agents = Some(agents);
    }

    /// 新设计：单次 LLM 调用完成「分类 + 拆解」，并初始化 ConversationState。
    ///
    /// 行为：
    /// - 若 `conv_state.plan_initialized == true`：返回 `AlreadyPlanned`
    /// - 否则调用 `harness.chat()` 拿到 JSON，用 `CombinedPlanner::parse_plan` 校验
    /// - 校验失败 → 使用 `CombinedPlanner::fallback_plan()`
    /// - 根据 `agent_id`：
    ///   - `"qa"` → 创建 `ConversationState::new_qa()`
    ///   - 其他 → 把 `PlannedMilestone` 转成 `Milestone`，创建 Executing 状态
    pub async fn ensure_combined_plan(
        &mut self,
        user_message: &str,
    ) -> Result<CombinedPlanOutcome> {
        let agents = self
            .agents
            .clone()
            .ok_or_else(|| anyhow::anyhow!("AgentRegistry 未注入，先调用 set_agent_registry"))?;

        if self
            .conv_state
            .as_ref()
            .map(|cs| cs.plan_initialized)
            .unwrap_or(false)
        {
            return Ok(CombinedPlanOutcome::AlreadyPlanned);
        }

        // 关键：只把 harness 实际注册的工具名传给 planner，否则 LLM 会被诱导
        // 调用伪工具（输出形如 [web_search: ...] 的纯文本）。
        let available_tools: Vec<String> = self
            .harness
            .tools()
            .into_iter()
            .map(|t| t.name)
            .collect();

        let prompt = CombinedPlanner::build_prompt(user_message, &agents, &available_tools);
        eprintln!(
            "[planner] user_message={user_message:?} available_tools={available_tools:?}"
        );
        let raw = self
            .harness
            .chat(ChatRequest {
                user_message: prompt,
                platform_system_prompt: Some(
                    "你是任务拆解器。严格只输出 JSON，不要任何其他文本。".into(),
                ),
                context: Default::default(),
                tools: vec![],
                model: None,
                session_id: None,
                previous_messages: vec![],
            })
            .await?;
        eprintln!("[planner] raw LLM 返回:\n{raw}");

        let (plan, used_fallback) =
            match CombinedPlanner::parse_plan(&raw, &agents, &available_tools) {
                Ok(p) => (p, false),
                Err(e) => {
                    eprintln!("[planner] parse_plan 失败，使用 fallback。err={e}");
                    (CombinedPlanner::fallback_plan(), true)
                }
            };
        eprintln!(
            "[planner] 解析结果: agent={} used_fallback={} milestones={}",
            plan.agent_id,
            used_fallback,
            plan.milestones.len()
        );
        for (i, pm) in plan.milestones.iter().enumerate() {
            eprintln!(
                "[planner]   #{i} label={:?} mode={:?} tools={:?} hint={:?}",
                pm.label, pm.mode, pm.tools, pm.prompt_hint
            );
        }

        if plan.is_qa() {
            let mut state = ConversationState::new_qa(&plan.agent_id);
            state.turn_count = 1;
            let agent_id = plan.agent_id.clone();
            self.conv_state = Some(state);
            return Ok(CombinedPlanOutcome::QaMode { agent_id });
        }

        let milestones = plan
            .milestones
            .iter()
            .enumerate()
            .map(|(idx, pm)| planned_to_milestone(idx, pm))
            .collect::<Vec<_>>();

        let milestone_count = milestones.len();
        let mut state = ConversationState::new(plan.agent_id.clone(), milestones);
        state.set_agent(&plan.agent_id);
        state.plan_initialized = true;
        state.global_mode = GlobalMode::Executing;
        let agent_id = plan.agent_id.clone();
        self.conv_state = Some(state);

        Ok(CombinedPlanOutcome::ScenarioMode {
            agent_id,
            milestone_count,
            used_fallback,
        })
    }

    /// 拿到当前 agent 的 system prompt 正文（替代 app_system_prompt）
    pub fn agent_system_prompt(&self) -> Option<String> {
        let cs = self.conv_state.as_ref()?;
        let agent_id = cs.agent_id.as_deref()?;
        let agents = self.agents.as_ref()?;
        agents.get(agent_id).map(|a| a.body.clone())
    }

    /// 将 choice card 的结果作为平台事件消费，而不是只作为普通聊天文本。
    pub fn apply_choice_result(
        &mut self,
        call_id: &str,
        answers: &[UserChoiceAnswer],
        skip: bool,
    ) -> MilestoneAdvanceResult {
        let active_before = self
            .conv_state
            .as_ref()
            .and_then(|cs| cs.active_milestone().cloned());

        // Review 模式有三个状态机分支（满意/微调/重做），靠 label 前缀识别。
        // 不是 Review milestone 时走原来的"mark_done + advance"路径。
        let review_branch: Option<ReviewBranch> =
            if !skip && is_review_milestone(active_before.as_ref()) {
                Some(classify_review_branch(answers))
            } else {
                None
            };

        let tool_msg = if skip {
            format!("[tool_result call_id={call_id}] 用户跳过了此选择器，继续推进。")
        } else {
            format!(
                "[tool_result call_id={call_id}] 用户选择:\n{}",
                answers
                    .iter()
                    .map(|a| format!("  {} ({}): {}", a.id, a.label, a.value))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        self.messages.push(HistoryMessage {
            role: "tool".into(),
            content: tool_msg,
        });

        if let Some(ref mut cs) = self.conv_state {
            if skip {
                cs.set_context("last_choice_skipped", "true");
            } else {
                for answer in answers {
                    if !answer.id.trim().is_empty() {
                        cs.set_context(answer.id.trim(), answer.value.trim());
                    }
                }
                let choice_summary = choice_summary(answers);
                if !choice_summary.is_empty() {
                    cs.set_context("last_choice_summary", choice_summary);
                }
            }

            // Review 分支处理：满意/重做 走默认 mark_done；微调走 rewind_to(final_output)
            match review_branch {
                Some(ReviewBranch::Tweak) => {
                    if let Some(ref active) = active_before {
                        let final_output_id = cs
                            .milestones
                            .iter()
                            .find(|(m, _)| m.contract.mode == crate::contract::MilestoneMode::FinalOutput)
                            .map(|(m, _)| m.id.clone());
                        if let Some(final_id) = final_output_id {
                            // rewind 会清受影响 milestone 的 context，所以反馈在 rewind 之后再 set
                            let feedback_label = answers
                                .first()
                                .map(|a| a.label.clone())
                                .unwrap_or_default();
                            cs.rewind_to(&final_id);
                            if !feedback_label.is_empty() {
                                cs.set_context("review_feedback", feedback_label);
                            }
                        } else {
                            // 异常：review 阶段但没找到 final_output，按默认 mark_done 兜底
                            cs.mark_done(&active.id);
                        }
                    }
                }
                Some(ReviewBranch::Redo) => {
                    // 重做：标 review 完成（已完成它的工作 — 收到了用户的"重做"决策）
                    // 同时记录信号，让 web 层在 summary 提示用户走 /replan
                    if let Some(ref active) = active_before {
                        cs.set_context("review_outcome", "redo");
                        cs.mark_done(&active.id);
                    }
                }
                Some(ReviewBranch::Accept) | None => {
                    if let Some(ref active) = active_before {
                        cs.mark_done(&active.id);
                    }
                }
            }
        }

        let active_after = self
            .conv_state
            .as_ref()
            .and_then(|cs| cs.active_milestone().cloned());

        if self.is_fine_granularity() {
            if let (Some(cs), Some(next)) = (&mut self.conv_state, &active_after) {
                cs.set_context(AWAITING_START_MILESTONE_KEY, next.id.clone());
            }
        }

        let summary = match review_branch {
            Some(ReviewBranch::Tweak) => format!(
                "已记录你的微调意向：{}。系统将重新生成最终产物。",
                answers
                    .first()
                    .map(|a| a.label.as_str())
                    .unwrap_or("（未识别）")
            ),
            Some(ReviewBranch::Redo) => {
                "已记录「重新规划」意向。请发送 `/replan` 重新拆解任务。".to_string()
            }
            Some(ReviewBranch::Accept) | None => {
                choice_event_summary(skip, answers, active_before.as_ref(), active_after.as_ref())
            }
        };

        MilestoneAdvanceResult {
            completed_milestone_id: active_before.as_ref().map(|m| m.id.clone()),
            completed_milestone_label: active_before.as_ref().map(|m| m.label.clone()),
            next_milestone_id: active_after.as_ref().map(|m| m.id.clone()),
            next_milestone_label: active_after.as_ref().map(|m| m.label.clone()),
            summary,
        }
    }

    /// 新设计默认 fine 粒度（用户每步都需要显式"继续"或前端 auto-continue）。
    /// 保留方法是为了未来支持 coarse / medium 时方便扩展。
    pub fn is_fine_granularity(&self) -> bool {
        self.agents.is_some()
    }

    pub fn consume_continue_command(
        &mut self,
        user_message: &str,
    ) -> Option<MilestoneAdvanceResult> {
        if !is_continue_command(user_message) {
            return None;
        }

        // Q&A 模式：不消费"继续"，让普通 chat 路径处理
        if self
            .conv_state
            .as_ref()
            .map(|cs| cs.global_mode == GlobalMode::QnA)
            .unwrap_or(false)
        {
            return None;
        }

        if !self.is_fine_granularity() {
            return None;
        }

        if self.clear_awaiting_start_marker() {
            return None;
        }

        let active_before = self
            .conv_state
            .as_ref()
            .and_then(|cs| cs.active_milestone().cloned())?;

        if let Some(ref mut cs) = self.conv_state {
            cs.mark_done(&active_before.id);
        }

        let active_after = self
            .conv_state
            .as_ref()
            .and_then(|cs| cs.active_milestone().cloned());

        let summary = if let Some(ref next) = active_after {
            format!("已进入「{}」。", next.label)
        } else {
            "所有步骤已完成。".to_string()
        };

        Some(MilestoneAdvanceResult {
            completed_milestone_id: Some(active_before.id),
            completed_milestone_label: Some(active_before.label),
            next_milestone_id: active_after.as_ref().map(|m| m.id.clone()),
            next_milestone_label: active_after.as_ref().map(|m| m.label.clone()),
            summary,
        })
    }

    /// 清除“下一阶段等待开始”标记。
    ///
    /// choice_result 已经把下一阶段设为 active；此时用户输入“继续”表示开始该阶段，
    /// 不是完成该阶段。
    pub fn clear_awaiting_start_marker(&mut self) -> bool {
        let active_id = self
            .conv_state
            .as_ref()
            .and_then(|cs| cs.active_milestone())
            .map(|m| m.id.clone());

        let Some(active_id) = active_id else {
            return false;
        };

        let Some(ref mut cs) = self.conv_state else {
            return false;
        };

        if cs
            .context
            .get(AWAITING_START_MILESTONE_KEY)
            .map(|id| id == &active_id)
            .unwrap_or(false)
        {
            cs.context.remove(AWAITING_START_MILESTONE_KEY);
            return true;
        }

        false
    }

    pub fn build_next_contract_prompt(
        &self,
        user_message: &str,
    ) -> Result<super::step_builder::StepPrompt> {
        let cs = self
            .conv_state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("没有对话状态"))?;
        let milestone = cs
            .active_milestone()
            .ok_or_else(|| anyhow::anyhow!("没有活跃里程碑"))?;
        let directive =
            crate::contract_runtime::ContractRuntime::next_directive(milestone, cs, user_message)?;
        match directive {
            crate::contract_runtime::TurnDirective::CallLlm(prompt) => {
                Ok(super::step_builder::StepBuilder::build_contract_prompt(
                    &prompt,
                    &cs.context,
                    self.agent_system_prompt().as_deref(),
                ))
            }
            _ => anyhow::bail!("当前 directive 不是 CallLlm"),
        }
    }

    /// 构建增强的聊天请求
    pub fn build_request(&self, user_message: &str, tools: Vec<ToolDef>) -> ChatRequest {
        let mut context = std::collections::HashMap::new();

        // 注入对话状态上下文
        if let Some(ref cs) = self.conv_state {
            for (k, v) in &cs.context {
                context.insert(k.clone(), v.clone());
            }
            // 注入当前阶段信息
            if let Some(active) = cs.active_milestone() {
                context.insert("current_phase".to_string(), active.label.clone());
                if let Some(ref hint) = active.prompt_hint {
                    context.insert("phase_hint".to_string(), hint.clone());
                }
            }
        }

        ChatRequest {
            user_message: user_message.to_string(),
            platform_system_prompt: self.agent_system_prompt(),
            context,
            tools,
            model: None,
            session_id: self.conv_state.as_ref().map(|cs| {
                let scope = cs.agent_id.as_deref().unwrap_or(cs.app_id.as_str());
                format!("{}-{}", scope, cs.turn_count)
            }),
            previous_messages: self.messages.clone(),
        }
    }

    /// 处理用户消息并返回完整响应
    pub async fn process_message(&mut self, user_message: &str) -> Result<String> {
        let tools = self.resolve_tools();
        let request = self.build_request(user_message, tools);

        // 更新轮数
        if let Some(ref mut cs) = self.conv_state {
            cs.increment_turn();
        }

        let response = self.harness.chat(request).await?;

        // 追加到消息历史
        self.messages.push(HistoryMessage {
            role: "user".into(),
            content: user_message.to_string(),
        });
        self.messages.push(HistoryMessage {
            role: "assistant".into(),
            content: response.clone(),
        });

        // 更新上下文（从响应中提取关键信息 — 目前简化处理）
        self.extract_context_from_response(&response);

        Ok(response)
    }

    /// 解析当前会话可用工具列表（harness 注册的全部工具）。
    /// 进一步的 contract 级过滤在 web/mod.rs 的 `filter_tools_for_contract` 完成。
    fn resolve_tools(&self) -> Vec<ToolDef> {
        self.harness.tools()
    }

    /// 从 AI 响应中提取上下文信息（关键词 → 对话状态）
    fn extract_context_from_response(&mut self, response: &str) {
        if let Some(ref mut cs) = self.conv_state {
            // 简单的关键词提取，后续可替换为 LLM 判断
            if response.contains("行数") || response.contains("列名") {
                cs.set_context("data_explored", "true");
            }
            if response.contains("方案") || response.contains("对比") {
                cs.set_context("options_presented", "true");
            }
        }
    }

    /// 保存当前状态为检查点
    pub fn save_checkpoint(&self) -> Result<()> {
        if let Some(ref cs) = self.conv_state {
            let checkpoint = Checkpoint {
                session_id: format!("{}-{}", cs.app_id, cs.turn_count),
                app_id: cs.app_id.clone(),
                conversation_state: serde_json::to_value(cs)?,
                created_at: chrono::Utc::now().timestamp(),
            };
            self.harness.save_checkpoint(&checkpoint)?;
        }
        Ok(())
    }

    /// 从检查点恢复
    pub fn restore_from_checkpoint(&mut self, session_id: &str) -> Result<bool> {
        if let Some(checkpoint) = self.harness.load_checkpoint(session_id)? {
            let cs: ConversationState = serde_json::from_value(checkpoint.conversation_state)?;
            self.conv_state = Some(cs);
            return Ok(true);
        }
        Ok(false)
    }

    /// 列出可用模型
    pub fn available_models(&self) -> Vec<ModelInfo> {
        self.harness.models()
    }

}

/// Review milestone 选择题的三种语义分支。
///
/// 通过 LLM 输出的选项 label 前缀识别：
/// - "满意" 开头  → Accept（接受产物，正常完成）
/// - "重做" 开头  → Redo（用户想重新规划，走 /replan）
/// - 其他       → Tweak（微调，回退到 final_output 重做）
///
/// 这个识别契约写在 contract_runtime 的 Review mode prompt 里强制 LLM 遵守。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewBranch {
    Accept,
    Redo,
    Tweak,
}

fn is_review_milestone(active: Option<&Milestone>) -> bool {
    active
        .map(|m| m.contract.mode == crate::contract::MilestoneMode::Review)
        .unwrap_or(false)
}

fn classify_review_branch(answers: &[UserChoiceAnswer]) -> ReviewBranch {
    let label = answers
        .first()
        .map(|a| a.label.as_str().trim_start())
        .unwrap_or("");
    if label.starts_with("满意") {
        ReviewBranch::Accept
    } else if label.starts_with("重做") {
        ReviewBranch::Redo
    } else {
        ReviewBranch::Tweak
    }
}

fn is_continue_command(user_message: &str) -> bool {
    matches!(
        user_message.trim(),
        "继续" | "下一步" | "进入下一步" | "开始下一步" | "继续下一步" | "请继续"
    )
}

fn choice_summary(answers: &[UserChoiceAnswer]) -> String {
    answers
        .iter()
        .map(|a| {
            if a.label == a.value {
                a.value.clone()
            } else {
                format!("{}: {}", a.label, a.value)
            }
        })
        .collect::<Vec<_>>()
        .join("，")
}

fn choice_event_summary(
    skip: bool,
    answers: &[UserChoiceAnswer],
    completed: Option<&Milestone>,
    next: Option<&Milestone>,
) -> String {
    let mut lines = Vec::new();
    if skip {
        lines.push("已跳过当前选择，系统将根据已有信息继续。".to_string());
    } else {
        let summary = choice_summary(answers);
        if summary.is_empty() {
            lines.push("已记录你的选择。".to_string());
        } else {
            lines.push(format!("已记录你的选择：{summary}。"));
        }
    }

    if let Some(ms) = completed {
        lines.push(format!("当前阶段「{}」已完成。", ms.label));
    }

    if let Some(ms) = next {
        // 不再要求用户手动输入"继续"——前端应自动让 LLM 进入下一阶段
        lines.push(format!("→ 进入「{}」。", ms.label));
    } else {
        lines.push("所有步骤已完成。".to_string());
    }

    lines.join("\n\n")
}

/// 把 `PlannedMilestone` 转换为 `Milestone`，挂上 mode 默认 contract + LLM 选的 tools。
fn planned_to_milestone(idx: usize, pm: &PlannedMilestone) -> Milestone {
    let mut contract = contract_for_mode(pm.mode.clone());
    contract.allowed_tools = pm.tools.clone();
    contract.required_context = pm.required_context.clone();
    contract.produced_context = pm.produced_context.clone();
    Milestone {
        id: format!("ms_{idx}"),
        label: pm.label.clone(),
        prompt_hint: pm.prompt_hint.clone(),
        icon: None,
        contract,
        ..Default::default()
    }
}

// === Mock AgentHarness（用于测试和开发） ===

/// Mock 工具 —— 单元测试 + 集成测试共用。
///
/// 非 cfg(test) 门控，以便 `tests/` 下的集成测试访问。结构简单，
/// 生产代码不会调用它（除非显式构造）。
pub mod mock {
    use super::*;
    use async_trait::async_trait;
    use futures_util::stream;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 简单的 Mock harness，按预设序列返回响应。
    pub struct MockHarness {
        pub tools: Vec<ToolDef>,
        pub models: Vec<ModelInfo>,
        pub responses: Vec<String>,
        call_count: AtomicUsize,
    }

    impl MockHarness {
        pub fn new() -> Self {
            Self {
                tools: vec![ToolDef {
                    name: "file_read".into(),
                    description: "Read files".into(),
                    parameters: serde_json::json!({}),
                }],
                models: vec![ModelInfo {
                    id: "mock-model".into(),
                    provider: "mock".into(),
                    capability: "medium".into(),
                }],
                responses: vec!["Mock response".into()],
                call_count: AtomicUsize::new(0),
            }
        }

        pub fn with_responses(responses: Vec<String>) -> Self {
            Self {
                // 给测试用的工具池：覆盖测试常用的工具名
                tools: vec![
                    ToolDef {
                        name: "request_user_input".into(),
                        description: "ask user".into(),
                        parameters: serde_json::json!({}),
                    },
                    ToolDef {
                        name: "read_file".into(),
                        description: "read file".into(),
                        parameters: serde_json::json!({}),
                    },
                    ToolDef {
                        name: "write_file".into(),
                        description: "write file".into(),
                        parameters: serde_json::json!({}),
                    },
                    ToolDef {
                        name: "web_search".into(),
                        description: "search web".into(),
                        parameters: serde_json::json!({}),
                    },
                    ToolDef {
                        name: "exec_shell".into(),
                        description: "run shell".into(),
                        parameters: serde_json::json!({}),
                    },
                    // 保留 file_write 兼容旧测试中可能用到的名字
                    ToolDef {
                        name: "file_write".into(),
                        description: "legacy".into(),
                        parameters: serde_json::json!({}),
                    },
                ],
                models: vec![ModelInfo {
                    id: "mock-model".into(),
                    provider: "mock".into(),
                    capability: "medium".into(),
                }],
                responses,
                call_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl AgentHarness for MockHarness {
        async fn chat_stream(
            &self,
            _req: ChatRequest,
        ) -> Result<
            Box<
                dyn futures_util::stream::Stream<Item = Result<crate::harness::StreamEvent>>
                    + Send
                    + Unpin,
            >,
        > {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            let response = self
                .responses
                .get(idx)
                .cloned()
                .unwrap_or_else(|| "Mock response".into());

            let events: Vec<Result<crate::harness::StreamEvent>> = vec![
                Ok(crate::harness::StreamEvent::TextDelta { content: response }),
                Ok(crate::harness::StreamEvent::Done),
            ];

            Ok(Box::new(stream::iter(events)))
        }

        fn tools(&self) -> Vec<ToolDef> {
            self.tools.clone()
        }

        fn models(&self) -> Vec<ModelInfo> {
            self.models.clone()
        }

        fn save_checkpoint(&self, _state: &Checkpoint) -> Result<()> {
            Ok(())
        }

        fn load_checkpoint(&self, _id: &str) -> Result<Option<Checkpoint>> {
            Ok(None)
        }

        fn list_sessions(&self) -> Result<Vec<String>> {
            Ok(vec![])
        }

        fn workspace_dir(&self) -> PathBuf {
            PathBuf::from(".")
        }
    }

    #[tokio::test]
    async fn test_platform_engine_basic() {
        let mock = MockHarness::new();
        let mut engine = PlatformEngine::new(mock, PathBuf::from("."));

        let response = engine.process_message("hello").await.unwrap();
        assert_eq!(response, "Mock response");
    }

    /// 设置一个 doc_generation 场景：collect → freeform → final_output，第一阶段 Active。
    async fn engine_in_scenario() -> PlatformEngine<MockHarness> {
        let mock = MockHarness::with_responses(vec![
            r#"{
                "agent": "doc_generation",
                "milestones": [
                    {"label": "明确需求", "mode": "collect", "tools": ["request_user_input"]},
                    {"label": "生成草稿", "mode": "freeform", "tools": []},
                    {"label": "定稿", "mode": "final_output", "tools": ["file_write"]}
                ]
            }"#
            .into(),
        ]);
        let mut engine = PlatformEngine::new(mock, PathBuf::from("."));
        engine.set_agent_registry(agents_registry());
        engine.ensure_combined_plan("帮我写周报").await.unwrap();
        engine
    }

    /// 4 阶段场景：collect → freeform → final_output → review，并把前 3 阶段标 Done，
    /// review 阶段 Active。用于测 Review 分支三种走向。
    async fn engine_at_review_stage() -> PlatformEngine<MockHarness> {
        let mock = MockHarness::with_responses(vec![
            r#"{
                "agent": "planning",
                "milestones": [
                    {"label": "收集偏好", "mode": "collect", "tools": ["request_user_input"]},
                    {"label": "生成草稿", "mode": "freeform", "tools": []},
                    {"label": "定稿", "mode": "final_output", "tools": ["write_file"]},
                    {"label": "审核产物", "mode": "review", "tools": ["request_user_input"]}
                ]
            }"#
            .into(),
        ]);
        let mut engine = PlatformEngine::new(mock, PathBuf::from("."));
        engine.set_agent_registry(agents_registry());
        engine.ensure_combined_plan("帮我做计划").await.unwrap();
        let cs = engine.conv_state.as_mut().unwrap();
        // 强制把前 3 阶段标 Done，让 review (ms_3) active
        cs.mark_done("ms_0");
        cs.mark_done("ms_1");
        cs.mark_done("ms_2");
        engine
    }

    #[tokio::test]
    async fn review_accept_marks_done_and_ends_workflow() {
        let mut engine = engine_at_review_stage().await;
        let result = engine.apply_choice_result(
            "review-1",
            &[UserChoiceAnswer {
                id: "review".into(),
                label: "满意，按此输出".into(),
                value: "满意，按此输出".into(),
            }],
            false,
        );
        assert_eq!(result.completed_milestone_id.as_deref(), Some("ms_3"));
        assert_eq!(result.next_milestone_id, None, "Review 结束应该 AllDone");
        let cs = engine.conv_state.as_ref().unwrap();
        assert_eq!(cs.milestones[3].1, crate::workflow::MilestoneStatus::Done);
    }

    #[tokio::test]
    async fn review_tweak_rewinds_to_final_output_and_records_feedback() {
        let mut engine = engine_at_review_stage().await;
        let result = engine.apply_choice_result(
            "review-1",
            &[UserChoiceAnswer {
                id: "review".into(),
                label: "调整时间安排：把上午徒步改到下午".into(),
                value: "调整时间安排：把上午徒步改到下午".into(),
            }],
            false,
        );
        let cs = engine.conv_state.as_ref().unwrap();
        // final_output (ms_2) 回到 Active；review (ms_3) 回到 Pending
        assert_eq!(cs.milestones[2].1, crate::workflow::MilestoneStatus::Active);
        assert_eq!(cs.milestones[3].1, crate::workflow::MilestoneStatus::Pending);
        // 反馈作为 context 注入
        assert_eq!(
            cs.context.get("review_feedback").map(String::as_str),
            Some("调整时间安排：把上午徒步改到下午"),
            "用户的微调意向应作为反馈注入 context"
        );
        // active_after 是 final_output
        assert_eq!(result.next_milestone_id.as_deref(), Some("ms_2"));
        assert!(
            result.summary.contains("微调") || result.summary.contains("重新生成"),
            "summary 应提示微调走向: {}",
            result.summary
        );
    }

    #[tokio::test]
    async fn review_redo_marks_done_and_hints_replan() {
        let mut engine = engine_at_review_stage().await;
        let result = engine.apply_choice_result(
            "review-1",
            &[UserChoiceAnswer {
                id: "review".into(),
                label: "重做，重新规划".into(),
                value: "重做，重新规划".into(),
            }],
            false,
        );
        let cs = engine.conv_state.as_ref().unwrap();
        assert_eq!(cs.milestones[3].1, crate::workflow::MilestoneStatus::Done);
        assert_eq!(
            cs.context.get("review_outcome").map(String::as_str),
            Some("redo"),
            "应该记录 redo 信号便于上层 UI 提示"
        );
        assert!(
            result.summary.contains("/replan"),
            "summary 应提示用户走 /replan: {}",
            result.summary
        );
    }

    #[tokio::test]
    async fn apply_choice_result_records_context_and_advances_milestone() {
        let mut engine = engine_in_scenario().await;

        let result = engine.apply_choice_result(
            "choice-1",
            &[
                UserChoiceAnswer {
                    id: "doc_type".into(),
                    label: "周报".into(),
                    value: "周报".into(),
                },
                UserChoiceAnswer {
                    id: "audience".into(),
                    label: "团队".into(),
                    value: "团队".into(),
                },
            ],
            false,
        );

        let cs = engine.conv_state.as_ref().unwrap();
        assert_eq!(cs.context.get("doc_type").map(String::as_str), Some("周报"));
        assert_eq!(cs.context.get("audience").map(String::as_str), Some("团队"));
        // 拆解里第一个 milestone 是 "明确需求"（id=ms_0）
        assert_eq!(result.completed_milestone_id.as_deref(), Some("ms_0"));
        assert_eq!(result.next_milestone_id.as_deref(), Some("ms_1"));
        assert_eq!(
            cs.active_milestone().map(|m| m.id.as_str()),
            Some("ms_1")
        );
        assert!(engine.messages.iter().any(|m| {
            m.role == "tool" && m.content.contains("choice-1") && m.content.contains("doc_type")
        }));
    }

    #[tokio::test]
    async fn consume_continue_advances_in_agent_path() {
        let mut engine = engine_in_scenario().await;

        let result = engine.consume_continue_command("继续").unwrap();
        assert_eq!(result.completed_milestone_id.as_deref(), Some("ms_0"));
        assert_eq!(result.next_milestone_id.as_deref(), Some("ms_1"));

        let cs = engine.conv_state.as_ref().unwrap();
        assert_eq!(cs.active_milestone().map(|m| m.id.as_str()), Some("ms_1"));
    }

    #[tokio::test]
    async fn consume_continue_no_op_in_qa_mode() {
        let mock = MockHarness::with_responses(vec![
            r#"{"agent": "qa", "milestones": []}"#.into(),
        ]);
        let mut engine = PlatformEngine::new(mock, PathBuf::from("."));
        engine.set_agent_registry(agents_registry());
        engine.ensure_combined_plan("K-means 是什么").await.unwrap();

        // QnA 模式：「继续」不被消费，让普通 chat 路径处理
        assert!(engine.consume_continue_command("继续").is_none());
    }

    #[tokio::test]
    async fn continue_after_choice_starts_next_milestone_without_skipping_it() {
        let mut engine = engine_in_scenario().await;

        engine.apply_choice_result(
            "choice-1",
            &[UserChoiceAnswer {
                id: "doc_type".into(),
                label: "周报".into(),
                value: "周报".into(),
            }],
            false,
        );

        // choice_result 已经推进到 ms_1，AWAITING_START_MILESTONE_KEY 已设；
        // 此时"继续"用于"开始 ms_1"，被 clear_awaiting_start_marker 消费，返回 None
        assert!(engine.consume_continue_command("继续").is_none());
        assert_eq!(
            engine
                .conv_state
                .as_ref()
                .and_then(|cs| cs.active_milestone())
                .map(|m| m.id.as_str()),
            Some("ms_1")
        );
    }

    #[tokio::test]
    async fn build_next_contract_prompt_uses_runtime_directive() {
        let engine = engine_in_scenario().await;

        let prompt = engine.build_next_contract_prompt("hi").unwrap();
        assert!(prompt.system.contains("当前阶段") || prompt.system.contains("当前契约要求"));
    }

    // === ensure_combined_plan 测试 ===

    fn agents_registry() -> Arc<AgentRegistry> {
        use crate::agent_registry::AgentDefinition;
        let mut reg = AgentRegistry::default();
        for (id, body) in [
            ("qa", "Q&A body"),
            ("doc_generation", "Docs body"),
            ("data_analysis", "Data body"),
            ("planning", "Plans body"),
            ("generic", "Generic body"),
        ] {
            reg.register(AgentDefinition {
                id: id.into(),
                name: id.into(),
                description: format!("{id} agent"),
                emoji: None,
                body: body.into(),
            });
        }
        Arc::new(reg)
    }

    #[tokio::test]
    async fn ensure_combined_plan_qa_creates_qa_state() {
        let mock = MockHarness::with_responses(vec![
            r#"{"agent": "qa", "milestones": []}"#.into(),
        ]);
        let mut engine = PlatformEngine::new(mock, PathBuf::from("."));
        engine.set_agent_registry(agents_registry());

        let outcome = engine.ensure_combined_plan("K-means 是什么").await.unwrap();

        match outcome {
            CombinedPlanOutcome::QaMode { ref agent_id } => assert_eq!(agent_id, "qa"),
            other => panic!("expected QaMode, got {:?}", other),
        }

        let cs = engine.conv_state.as_ref().unwrap();
        assert_eq!(cs.global_mode, GlobalMode::QnA);
        assert_eq!(cs.agent_id.as_deref(), Some("qa"));
        assert!(cs.milestones.is_empty());
    }

    #[tokio::test]
    async fn ensure_combined_plan_scenario_attaches_milestones() {
        let mock = MockHarness::with_responses(vec![
            r#"{
                "agent": "doc_generation",
                "milestones": [
                    {"label": "确认结构", "mode": "produce_options", "tools": ["request_user_input"]},
                    {"label": "生成草稿", "mode": "freeform", "tools": []},
                    {"label": "定稿", "mode": "final_output", "tools": ["file_write"]}
                ]
            }"#.into(),
        ]);
        let mut engine = PlatformEngine::new(mock, PathBuf::from("."));
        engine.set_agent_registry(agents_registry());

        let outcome = engine.ensure_combined_plan("帮我写周报").await.unwrap();

        match outcome {
            CombinedPlanOutcome::ScenarioMode {
                ref agent_id,
                milestone_count,
                used_fallback,
            } => {
                assert_eq!(agent_id, "doc_generation");
                assert_eq!(milestone_count, 3);
                assert!(!used_fallback);
            }
            other => panic!("expected ScenarioMode, got {:?}", other),
        }

        let cs = engine.conv_state.as_ref().unwrap();
        assert_eq!(cs.global_mode, GlobalMode::Executing);
        assert_eq!(cs.agent_id.as_deref(), Some("doc_generation"));
        assert_eq!(cs.milestones.len(), 3);

        // 第一个 milestone 应该 Active
        assert!(matches!(
            cs.milestones[0].1,
            crate::workflow::MilestoneStatus::Active
        ));
        // 第一个 milestone 的 allowed_tools 应该是 LLM 选的
        assert_eq!(
            cs.milestones[0].0.contract.allowed_tools,
            vec!["request_user_input".to_string()]
        );
        // produce_options 的 question_budget 来自 mode 默认
        assert_eq!(cs.milestones[0].0.contract.question_budget, 1);
        // 最后一个 milestone 是 final_output
        let last = &cs.milestones[2].0.contract;
        assert!(matches!(
            last.mode,
            crate::contract::MilestoneMode::FinalOutput
        ));
    }

    #[tokio::test]
    async fn ensure_combined_plan_falls_back_on_invalid_json() {
        let mock = MockHarness::with_responses(vec!["this is not json".into()]);
        let mut engine = PlatformEngine::new(mock, PathBuf::from("."));
        engine.set_agent_registry(agents_registry());

        let outcome = engine.ensure_combined_plan("怎么办").await.unwrap();

        match outcome {
            CombinedPlanOutcome::ScenarioMode {
                ref agent_id,
                used_fallback,
                ..
            } => {
                assert_eq!(agent_id, "generic");
                assert!(used_fallback);
            }
            other => panic!("expected ScenarioMode w/ fallback, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn ensure_combined_plan_returns_already_planned_on_second_call() {
        let mock = MockHarness::with_responses(vec![
            r#"{"agent": "qa", "milestones": []}"#.into(),
        ]);
        let mut engine = PlatformEngine::new(mock, PathBuf::from("."));
        engine.set_agent_registry(agents_registry());

        let _ = engine.ensure_combined_plan("first").await.unwrap();
        let second = engine.ensure_combined_plan("second").await.unwrap();
        assert_eq!(second, CombinedPlanOutcome::AlreadyPlanned);
    }

    #[tokio::test]
    async fn ensure_combined_plan_without_registry_errors() {
        let mock = MockHarness::with_responses(vec![]);
        let mut engine = PlatformEngine::new(mock, PathBuf::from("."));
        // 没有调用 set_agent_registry
        let err = engine.ensure_combined_plan("hi").await.unwrap_err();
        assert!(err.to_string().contains("AgentRegistry"));
    }

    #[tokio::test]
    async fn agent_system_prompt_returns_body_for_current_agent() {
        let mock = MockHarness::with_responses(vec![
            r#"{"agent": "qa", "milestones": []}"#.into(),
        ]);
        let mut engine = PlatformEngine::new(mock, PathBuf::from("."));
        engine.set_agent_registry(agents_registry());

        engine.ensure_combined_plan("hi").await.unwrap();
        let body = engine.agent_system_prompt().unwrap();
        assert_eq!(body, "Q&A body");
    }

}
