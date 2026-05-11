//! PlatformEngine — 简化的 agent loop，封装平台上下文注入。
//!
//! 与原有 Engine 的区别:
//! - 不处理 LSP、沙箱、子代理、周期管理
//! - 在每次 LLM 调用前注入: app system prompt + ConversationState 上下文 + 里程碑提示
//! - 更轻量，专门服务于平台模式

use std::path::PathBuf;
use std::sync::Arc;

use super::agent_registry::AgentRegistry;
use super::app::{AppConfig, AppRegistry, Milestone};
use super::combined_planner::{CombinedPlanner, PlannedMilestone};
use super::contract::contract_for_mode;
use super::harness::{AgentHarness, ChatRequest, Checkpoint, HistoryMessage, ModelInfo, ToolDef};
use super::workflow::{ConversationState, GlobalMode};
use anyhow::Result;
use regex::Regex;

const AWAITING_START_MILESTONE_KEY: &str = "_awaiting_start_milestone";

// === Platform Engine ===

/// 平台引擎 — 包装 AgentHarness 实现，加入平台上下文
pub struct PlatformEngine<H: AgentHarness> {
    /// 底层 agent harness
    pub harness: H,
    /// 应用注册表（legacy，由 AgentRegistry 替代中）
    pub registry: Arc<AppRegistry>,
    /// Agent 注册表（新设计，从 prompts/*.md 加载）
    pub agents: Option<Arc<AgentRegistry>>,
    /// 当前对话状态
    pub conv_state: Option<ConversationState>,
    /// 当前应用配置（legacy）
    pub current_app: Option<AppConfig>,
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
    pub fn new(harness: H, registry: AppRegistry, workspace: PathBuf) -> Self {
        Self {
            harness,
            registry: Arc::new(registry),
            agents: None,
            conv_state: None,
            current_app: None,
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

        let (plan, used_fallback) = match CombinedPlanner::parse_plan(&raw, &agents, &available_tools) {
            Ok(p) => (p, false),
            Err(_) => (CombinedPlanner::fallback_plan(), true),
        };

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

            if let Some(ref active) = active_before {
                cs.mark_done(&active.id);
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

        let summary =
            choice_event_summary(skip, answers, active_before.as_ref(), active_after.as_ref());

        MilestoneAdvanceResult {
            completed_milestone_id: active_before.as_ref().map(|m| m.id.clone()),
            completed_milestone_label: active_before.as_ref().map(|m| m.label.clone()),
            next_milestone_id: active_after.as_ref().map(|m| m.id.clone()),
            next_milestone_label: active_after.as_ref().map(|m| m.label.clone()),
            summary,
        }
    }

    /// 在 fine granularity 下，将“继续”消费成显式里程碑推进。
    /// 是否为 fine 粒度（用户每步都需要显式"继续"）。
    /// - Legacy 路径：`current_app.granularity == "fine"`
    /// - 新设计路径：注入了 AgentRegistry 即视为 fine
    pub fn is_fine_granularity(&self) -> bool {
        match self.current_app.as_ref() {
            Some(app) => app.granularity.as_deref() == Some("fine"),
            None => self.agents.is_some(),
        }
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

    /// 加载应用并初始化对话状态
    pub fn load_app(&mut self, app_id: &str) -> Result<()> {
        let app = self
            .registry
            .find(app_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("应用不存在: {app_id}"))?;

        let conv_state = ConversationState::new(app_id.to_string(), app.milestones.clone());
        self.conv_state = Some(conv_state);
        self.current_app = Some(app);

        Ok(())
    }

    /// **LEGACY**：旧版动态拆解 + 静态 fallback。
    /// 新代码用 [`Self::ensure_combined_plan`]。
    #[allow(deprecated)]
    pub async fn ensure_plan_initialized(&mut self, user_message: &str) -> Result<()> {
        let Some(app) = self.current_app.clone() else {
            return Ok(());
        };
        if self
            .conv_state
            .as_ref()
            .map(|cs| cs.plan_initialized)
            .unwrap_or(false)
        {
            return Ok(());
        }

        let milestones = match app.planning.mode {
            crate::contract::PlanningMode::StaticOnly => app.milestones.clone(),
            crate::contract::PlanningMode::DynamicWithStaticFallback => {
                let prompt =
                    crate::dynamic_planner::DynamicPlanner::build_prompt(user_message, &app);
                match self
                    .harness
                    .chat(ChatRequest {
                        user_message: prompt,
                        platform_system_prompt: Some("你是流程拆解器。只输出 JSON。".into()),
                        context: Default::default(),
                        tools: vec![],
                        model: None,
                        session_id: None,
                        previous_messages: vec![],
                    })
                    .await
                    .and_then(|text| {
                        crate::dynamic_planner::DynamicPlanner::parse_plan(&text, &app)
                    }) {
                    Ok(plan) => plan,
                    Err(_) => app.milestones.clone(),
                }
            }
        };

        let mut conv_state = ConversationState::new(app.id.clone(), milestones);
        conv_state.plan_initialized = true;
        self.conv_state = Some(conv_state);
        Ok(())
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
                    self.app_system_prompt().as_deref(),
                ))
            }
            _ => anyhow::bail!("当前 directive 不是 CallLlm"),
        }
    }

    /// 获取应用的 system prompt
    fn app_system_prompt(&self) -> Option<String> {
        let app = self.current_app.as_ref()?;
        self.registry
            .resolve_prompt(&app.id)
            .or_else(|| app.prompt.clone())
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

        // 优先用 agent_system_prompt（新设计），回退到 app_system_prompt（legacy）
        let system_prompt = self
            .agent_system_prompt()
            .or_else(|| self.app_system_prompt());

        ChatRequest {
            user_message: user_message.to_string(),
            platform_system_prompt: system_prompt,
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

    /// 根据 app 配置的 tools 白名单解析实际工具列表
    fn resolve_tools(&self) -> Vec<ToolDef> {
        let app_tool_names: Vec<String> = self
            .current_app
            .as_ref()
            .map(|a| a.tools.clone())
            .unwrap_or_default();

        if app_tool_names.is_empty() {
            return self.harness.tools();
        }

        // 从 harness 提供的完整工具列表中过滤
        let all_tools = self.harness.tools();
        all_tools
            .into_iter()
            .filter(|t| app_tool_names.contains(&t.name))
            .collect()
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
            self.load_app(&cs.app_id)?;
            self.conv_state = Some(cs);
            return Ok(true);
        }
        Ok(false)
    }

    /// 列出可用模型
    pub fn available_models(&self) -> Vec<ModelInfo> {
        self.harness.models()
    }

    /// 任务拆解 + 审阅（完整编排流程）
    pub async fn decompose_and_execute(&mut self, user_message: &str) -> Result<DecomposeResult> {
        use super::reviewer::LLMReviewer;
        use super::step_builder::StepBuilder;

        let app = self
            .current_app
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("没有加载应用"))?;

        // Step 1: 拆解
        let tools: Vec<String> = self.resolve_tools().into_iter().map(|t| t.name).collect();
        let context_summary = self
            .conv_state
            .as_ref()
            .and_then(|cs| cs.context_prompt())
            .unwrap_or_default();

        let decomp_prompt =
            StepBuilder::build_decomposition(user_message, app, &tools, &context_summary);

        let decomposition = self
            .harness
            .chat(ChatRequest {
                user_message: decomp_prompt,
                platform_system_prompt: Some(
                    "你是一个任务拆解专家。用中文回复。只输出步骤列表。".into(),
                ),
                context: Default::default(),
                tools: vec![],
                model: None, // 使用 harness 默认模型
                session_id: None,
                previous_messages: vec![],
            })
            .await?;

        // Step 2: 可解析性检查 + LLM 审阅（带重试）
        let mut retries = 0u32;
        let mut final_decomposition = decomposition;

        let review = loop {
            if !Self::parsability_check(&final_decomposition) {
                if retries >= 2 {
                    return Ok(DecomposeResult {
                        decomposition: "（使用应用预定义步骤）".into(),
                        review_passed: true,
                        milestone_count: app.milestones.len(),
                    });
                }
                let retry_prompt = format!(
                    "上次输出格式有误，请重新输出。每行格式: \"N. {{具体动词+具体对象+明确产出}}\"\n\n用户需求: {user_message}",
                );
                final_decomposition = self
                    .harness
                    .chat(ChatRequest {
                        user_message: retry_prompt,
                        platform_system_prompt: Some("只输出步骤列表。".into()),
                        context: Default::default(),
                        tools: vec![],
                        model: None,
                        session_id: None,
                        previous_messages: vec![],
                    })
                    .await?;
                retries += 1;
                continue;
            }

            let review =
                LLMReviewer::review(&self.harness, &final_decomposition, user_message, &tools)
                    .await?;

            if review.ok {
                break review;
            }

            if retries >= 2 {
                break review;
            }

            let feedback = LLMReviewer::format_feedback(&review);
            let retry_prompt = format!(
                "上次拆解有这些问题:\n{feedback}\n\n用户需求: {user_message}\n\n请重新拆解。",
            );
            final_decomposition = self
                .harness
                .chat(ChatRequest {
                    user_message: retry_prompt,
                    platform_system_prompt: Some(
                        "你是一个任务拆解专家。用中文回复。只输出步骤列表。".into(),
                    ),
                    context: Default::default(),
                    tools: vec![],
                    model: None, // 使用 harness 默认模型
                    session_id: None,
                    previous_messages: vec![],
                })
                .await?;
            retries += 1;
        };

        Ok(DecomposeResult {
            decomposition: final_decomposition,
            review_passed: review.ok,
            milestone_count: review.issues.len(),
        })
    }

    /// 可解析性检查: 非空 + 有数字序号 + 能提取行
    fn parsability_check(text: &str) -> bool {
        if text.trim().is_empty() {
            return false;
        }
        let re = Regex::new(r"(?m)^\s*\d+[\.\)、]").unwrap();
        if !re.is_match(text) {
            let re2 = Regex::new(r"步骤\s*\d+").unwrap();
            if !re2.is_match(text) {
                return false;
            }
        }
        text.lines().filter(|l| !l.trim().is_empty()).count() >= 1
    }

    /// 对当前活跃里程碑执行逐步执行
    pub async fn step_execute(&mut self) -> Result<Option<super::response_checker::NextAction>> {
        use super::response_checker::ResponseChecker;
        use super::step_builder::StepBuilder;

        let app = self
            .current_app
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("没有加载应用"))?;

        // 提前 clone，避免后续借用冲突
        let granularity = app.granularity.clone();
        let confirm_at = app.confirm_at.clone();

        let milestone = match self
            .conv_state
            .as_ref()
            .and_then(|cs| cs.active_milestone())
        {
            Some(m) => m.clone(),
            None => return Ok(None),
        };

        let context = self
            .conv_state
            .as_ref()
            .map(|cs| cs.context.clone())
            .unwrap_or_default();

        let step_prompt = StepBuilder::build(&milestone, &context, "", app);

        let response = self
            .harness
            .chat(ChatRequest {
                user_message: "".into(),
                platform_system_prompt: Some(step_prompt.system),
                context: context.clone(),
                tools: self.resolve_tools(),
                model: None, // 使用 harness 默认模型
                session_id: self
                    .conv_state
                    .as_ref()
                    .map(|cs| format!("{}-{}", cs.app_id, cs.turn_count)),
                previous_messages: vec![],
            })
            .await?;

        let check = ResponseChecker::check(&response, &milestone, app);

        if check.out_of_scope {
            self.consecutive_out_of_scope += 1;
        } else {
            self.consecutive_out_of_scope = 0;
        }

        self.extract_context_from_response(&response);

        // granularity 控制自动推进:
        //   fine = 每步暂停等用户（覆盖 ResponseChecker 的 Advance）
        //   medium = 仅 confirm_at 中的里程碑暂停
        //   coarse = 全部自动推进
        //   None = 使用 ResponseChecker 的判断
        let is_advance = matches!(
            &check.next_action,
            super::response_checker::NextAction::Advance
        );

        let should_advance = match granularity.as_deref() {
            Some("fine") => is_advance && check.signal.is_some(),
            Some("medium") => !confirm_at
                .as_ref()
                .map(|ids| ids.contains(&milestone.id))
                .unwrap_or(false),
            Some("coarse") => true,
            _ => is_advance,
        };

        if should_advance {
            if let Some(ref mut cs) = self.conv_state {
                cs.mark_done(&milestone.id);
            }
        }

        if let Some(ref mut cs) = self.conv_state {
            cs.increment_turn();
        }

        Ok(Some(check.next_action))
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
    completed: Option<&super::app::Milestone>,
    next: Option<&super::app::Milestone>,
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

/// 拆解结果
#[derive(Debug, Clone)]
pub struct DecomposeResult {
    pub decomposition: String,
    pub review_passed: bool,
    pub milestone_count: usize,
}

// === Mock AgentHarness（用于测试和开发） ===

#[cfg(test)]
pub mod mock {
    use super::*;
    use async_trait::async_trait;
    use futures_util::stream;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 简单的 Mock Engine，返回预设响应
    pub struct MockHarness {
        pub tools: Vec<ToolDef>,
        pub models: Vec<ModelInfo>,
        pub responses: Vec<String>,
        call_count: AtomicUsize,
    }

    static REGISTRY_ROOT_SEQ: AtomicUsize = AtomicUsize::new(0);

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
        let registry = AppRegistry::default();
        let mut engine = PlatformEngine::new(mock, registry, PathBuf::from("."));

        let response = engine.process_message("hello").await.unwrap();
        assert_eq!(response, "Mock response");
    }

    #[tokio::test]
    async fn test_platform_engine_with_app() {
        let mock = MockHarness::new();
        let mut engine = PlatformEngine::new(mock, AppRegistry::default(), PathBuf::from("."));

        // 没有加载 app 时也能正常工作
        let response = engine.process_message("hello").await.unwrap();
        assert!(!response.is_empty());
    }

    #[test]
    fn test_apply_choice_result_records_context_and_advances_milestone() {
        let mock = MockHarness::new();
        let registry = registry_with_plan_app("fine");
        let mut engine = PlatformEngine::new(mock, registry, PathBuf::from("."));

        engine.load_app("计划敲定").unwrap();
        let result = engine.apply_choice_result(
            "choice-1",
            &[
                UserChoiceAnswer {
                    id: "duration".into(),
                    label: "半天".into(),
                    value: "半天".into(),
                },
                UserChoiceAnswer {
                    id: "transport".into(),
                    label: "自驾".into(),
                    value: "自驾".into(),
                },
            ],
            false,
        );

        let cs = engine.conv_state.as_ref().unwrap();
        assert_eq!(cs.context.get("duration").map(String::as_str), Some("半天"));
        assert_eq!(
            cs.context.get("transport").map(String::as_str),
            Some("自驾")
        );
        assert_eq!(result.completed_milestone_id.as_deref(), Some("goal"));
        assert_eq!(result.next_milestone_id.as_deref(), Some("constraints"));
        assert_eq!(
            cs.active_milestone().map(|m| m.id.as_str()),
            Some("constraints")
        );
        assert!(engine.messages.iter().any(|m| {
            m.role == "tool" && m.content.contains("choice-1") && m.content.contains("duration")
        }));
    }

    #[test]
    fn test_consume_continue_advances_only_in_fine_granularity() {
        let mock = MockHarness::new();
        let registry = registry_with_plan_app("fine");
        let mut engine = PlatformEngine::new(mock, registry, PathBuf::from("."));

        engine.load_app("计划敲定").unwrap();
        let result = engine.consume_continue_command("继续").unwrap();

        let cs = engine.conv_state.as_ref().unwrap();
        assert_eq!(result.completed_milestone_id.as_deref(), Some("goal"));
        assert_eq!(result.next_milestone_id.as_deref(), Some("constraints"));
        assert_eq!(
            cs.active_milestone().map(|m| m.id.as_str()),
            Some("constraints")
        );

        let mock = MockHarness::new();
        let registry = registry_with_plan_app("medium");
        let mut engine = PlatformEngine::new(mock, registry, PathBuf::from("."));
        engine.load_app("计划敲定").unwrap();

        assert!(engine.consume_continue_command("继续").is_none());
        assert_eq!(
            engine
                .conv_state
                .as_ref()
                .and_then(|cs| cs.active_milestone())
                .map(|m| m.id.as_str()),
            Some("goal")
        );
    }

    #[test]
    fn test_continue_after_choice_starts_next_milestone_without_skipping_it() {
        let mock = MockHarness::new();
        let registry = registry_with_plan_app("fine");
        let mut engine = PlatformEngine::new(mock, registry, PathBuf::from("."));

        engine.load_app("计划敲定").unwrap();
        engine.apply_choice_result(
            "choice-1",
            &[UserChoiceAnswer {
                id: "duration".into(),
                label: "半天".into(),
                value: "半天".into(),
            }],
            false,
        );

        assert!(engine.consume_continue_command("继续").is_none());
        assert_eq!(
            engine
                .conv_state
                .as_ref()
                .and_then(|cs| cs.active_milestone())
                .map(|m| m.id.as_str()),
            Some("constraints")
        );
    }

    #[tokio::test]
    async fn test_ensure_plan_initialized_uses_static_fallback_when_dynamic_parse_fails() {
        let mut mock = MockHarness::new();
        mock.responses = vec!["not json".into()];
        let registry = registry_with_plan_app("fine");
        let mut engine = PlatformEngine::new(mock, registry, PathBuf::from("."));
        engine.load_app("计划敲定").unwrap();

        engine
            .ensure_plan_initialized("我周末去水库徒步")
            .await
            .unwrap();

        let cs = engine.conv_state.as_ref().unwrap();
        assert!(cs.plan_initialized);
        assert_eq!(cs.milestones[0].0.id, "goal");
    }

    #[tokio::test]
    async fn test_ensure_plan_initialized_creates_missing_conversation_state() {
        let mock = MockHarness::new();
        let registry = registry_with_plan_app("fine");
        let mut engine = PlatformEngine::new(mock, registry, PathBuf::from("."));
        engine.load_app("计划敲定").unwrap();
        engine.conv_state = None;

        engine
            .ensure_plan_initialized("我周末去水库徒步")
            .await
            .unwrap();

        let cs = engine.conv_state.as_ref().unwrap();
        assert!(cs.plan_initialized);
        assert_eq!(cs.milestones[0].0.id, "goal");
    }

    #[tokio::test]
    async fn test_next_contract_prompt_uses_runtime_directive() {
        let mock = MockHarness::new();
        let registry = registry_with_plan_app("fine");
        let mut engine = PlatformEngine::new(mock, registry, PathBuf::from("."));
        engine.load_app("计划敲定").unwrap();

        let prompt = engine.build_next_contract_prompt("继续").unwrap();
        assert!(prompt.system.contains("当前契约要求"));
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
        let mut engine = PlatformEngine::new(mock, AppRegistry::default(), PathBuf::from("."));
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
        let mut engine = PlatformEngine::new(mock, AppRegistry::default(), PathBuf::from("."));
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
        let mut engine = PlatformEngine::new(mock, AppRegistry::default(), PathBuf::from("."));
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
        let mut engine = PlatformEngine::new(mock, AppRegistry::default(), PathBuf::from("."));
        engine.set_agent_registry(agents_registry());

        let _ = engine.ensure_combined_plan("first").await.unwrap();
        let second = engine.ensure_combined_plan("second").await.unwrap();
        assert_eq!(second, CombinedPlanOutcome::AlreadyPlanned);
    }

    #[tokio::test]
    async fn ensure_combined_plan_without_registry_errors() {
        let mock = MockHarness::with_responses(vec![]);
        let mut engine = PlatformEngine::new(mock, AppRegistry::default(), PathBuf::from("."));
        // 没有调用 set_agent_registry
        let err = engine.ensure_combined_plan("hi").await.unwrap_err();
        assert!(err.to_string().contains("AgentRegistry"));
    }

    #[tokio::test]
    async fn agent_system_prompt_returns_body_for_current_agent() {
        let mock = MockHarness::with_responses(vec![
            r#"{"agent": "qa", "milestones": []}"#.into(),
        ]);
        let mut engine = PlatformEngine::new(mock, AppRegistry::default(), PathBuf::from("."));
        engine.set_agent_registry(agents_registry());

        engine.ensure_combined_plan("hi").await.unwrap();
        let body = engine.agent_system_prompt().unwrap();
        assert_eq!(body, "Q&A body");
    }

    fn registry_with_plan_app(granularity: &str) -> AppRegistry {
        let seq = REGISTRY_ROOT_SEQ.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "pinvou-choice-test-{}-{}-{}",
            std::process::id(),
            granularity,
            seq
        ));
        let app_dir = root.join("计划敲定");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("prompt.md"), "测试 prompt").unwrap();
        std::fs::write(
            app_dir.join("app.toml"),
            format!(
                r#"[app]
name = "计划敲定"
description = "测试计划应用"
icon = "*"
model_preference = "large"
granularity = "{granularity}"
prompt_file = "prompt.md"
tools = ["request_user_input"]

[[milestones]]
id = "goal"
label = "明确目标"
prompt_hint = "确认目标"

[[milestones]]
id = "constraints"
label = "梳理约束"
prompt_hint = "确认约束"
"#
            ),
        )
        .unwrap();
        AppRegistry::load(&root).unwrap()
    }
}
