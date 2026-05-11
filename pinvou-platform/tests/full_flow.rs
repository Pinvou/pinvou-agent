//! 端到端集成测试：用 MockHarness 模拟 LLM 拆解 JSON，跑完整 Engine 流程，
//! 验证 ensure_combined_plan → contract execution → choice result → 状态机
//! → slash 命令的事件序列与状态变化。
//!
//! 这些测试**不**走 DeepSeekHarness 的工具自动执行循环（那个需要真实 LlmClient
//! mock）；它们覆盖 PlatformEngine + ConversationState + RollbackManager 这一层。

use std::path::PathBuf;
use std::sync::Arc;

use futures_util::StreamExt;
use pinvou_platform::agent_registry::{AgentDefinition, AgentRegistry};
use pinvou_platform::contract::MilestoneMode;
use pinvou_platform::engine::{CombinedPlanOutcome, PlatformEngine, UserChoiceAnswer, mock::MockHarness};
use pinvou_platform::harness::{AgentHarness, ChatRequest, StreamEvent};
use pinvou_platform::rollback::{self, SlashCommand};
use pinvou_platform::workflow::{GlobalMode, MilestoneStatus};

// === Test fixtures ===

fn agents() -> Arc<AgentRegistry> {
    let mut reg = AgentRegistry::default();
    for (id, name, body) in [
        ("qa", "简单问答", "你是 Q&A 助手"),
        ("doc_generation", "文档生成", "你是文档生成助手"),
        ("data_analysis", "数据分析", "你是数据分析助手"),
        ("planning", "计划制定", "你是计划制定助手"),
        ("generic", "通用任务", "你是通用助手"),
    ] {
        reg.register(AgentDefinition {
            id: id.into(),
            name: name.into(),
            description: format!("{name} 的描述"),
            emoji: None,
            body: body.into(),
        });
    }
    Arc::new(reg)
}

fn doc_generation_plan_json() -> String {
    r#"{
        "agent": "doc_generation",
        "milestones": [
            {"label": "明确需求", "mode": "collect", "tools": ["request_user_input"]},
            {"label": "生成草稿", "mode": "freeform", "tools": []},
            {"label": "定稿", "mode": "final_output", "tools": ["file_write"]}
        ]
    }"#
    .to_string()
}

fn engine_with_plan(plan_json: &str) -> PlatformEngine<MockHarness> {
    let mock = MockHarness::with_responses(vec![plan_json.to_string()]);
    let mut engine = PlatformEngine::new(mock, PathBuf::from("."));
    engine.set_agent_registry(agents());
    engine
}

// === Q&A 路径 ===

#[tokio::test]
async fn qa_flow_creates_qa_state_with_no_milestones() {
    let mut engine = engine_with_plan(r#"{"agent": "qa", "milestones": []}"#);

    let outcome = engine
        .ensure_combined_plan("K-means 是什么")
        .await
        .expect("ensure_combined_plan ok");

    match outcome {
        CombinedPlanOutcome::QaMode { agent_id } => assert_eq!(agent_id, "qa"),
        other => panic!("expected QaMode, got {other:?}"),
    }

    let cs = engine.conv_state.as_ref().unwrap();
    assert_eq!(cs.global_mode, GlobalMode::QnA);
    assert_eq!(cs.agent_id.as_deref(), Some("qa"));
    assert!(cs.milestones.is_empty());
    assert!(cs.plan_initialized);
}

#[tokio::test]
async fn second_message_in_qa_mode_skips_replanning() {
    let mut engine = engine_with_plan(r#"{"agent": "qa", "milestones": []}"#);
    engine.ensure_combined_plan("K-means 是什么").await.unwrap();

    // 第二次调用：因为 plan_initialized=true，应该返回 AlreadyPlanned，
    // 不会触发新的 LLM 调用（mock 也只准备了 1 个响应）
    let outcome = engine.ensure_combined_plan("K-medoids 呢").await.unwrap();
    assert_eq!(outcome, CombinedPlanOutcome::AlreadyPlanned);
}

// === 场景路径 ===

#[tokio::test]
async fn scenario_flow_builds_milestones_from_plan() {
    let mut engine = engine_with_plan(&doc_generation_plan_json());

    let outcome = engine.ensure_combined_plan("帮我写周报").await.unwrap();

    match outcome {
        CombinedPlanOutcome::ScenarioMode {
            agent_id,
            milestone_count,
            used_fallback,
        } => {
            assert_eq!(agent_id, "doc_generation");
            assert_eq!(milestone_count, 3);
            assert!(!used_fallback);
        }
        other => panic!("expected ScenarioMode, got {other:?}"),
    }

    let cs = engine.conv_state.as_ref().unwrap();
    assert_eq!(cs.global_mode, GlobalMode::Executing);
    assert_eq!(cs.agent_id.as_deref(), Some("doc_generation"));
    assert_eq!(cs.milestones.len(), 3);
    assert_eq!(cs.milestones[0].1, MilestoneStatus::Active);
    assert_eq!(cs.milestones[1].1, MilestoneStatus::Pending);
    assert_eq!(cs.milestones[2].0.contract.mode, MilestoneMode::FinalOutput);
}

#[tokio::test]
async fn invalid_plan_json_falls_back_to_generic() {
    let mut engine = engine_with_plan("definitely not json");

    let outcome = engine.ensure_combined_plan("不知道做什么").await.unwrap();

    match outcome {
        CombinedPlanOutcome::ScenarioMode {
            agent_id,
            used_fallback,
            ..
        } => {
            assert_eq!(agent_id, "generic");
            assert!(used_fallback);
        }
        other => panic!("expected fallback to generic, got {other:?}"),
    }
    let cs = engine.conv_state.as_ref().unwrap();
    assert_eq!(cs.milestones.last().unwrap().0.contract.mode, MilestoneMode::FinalOutput);
}

// === Choice + 状态推进 ===

#[tokio::test]
async fn choice_result_advances_to_next_milestone_and_records_context() {
    let mut engine = engine_with_plan(&doc_generation_plan_json());
    engine.ensure_combined_plan("帮我写周报").await.unwrap();

    let result = engine.apply_choice_result(
        "call-1",
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

    assert_eq!(result.completed_milestone_id.as_deref(), Some("ms_0"));
    assert_eq!(result.next_milestone_id.as_deref(), Some("ms_1"));

    let cs = engine.conv_state.as_ref().unwrap();
    assert_eq!(cs.milestones[0].1, MilestoneStatus::Done);
    assert_eq!(cs.milestones[1].1, MilestoneStatus::Active);
    // context 携带选择 + 归属到 ms_0
    assert_eq!(cs.context.get("doc_type").map(String::as_str), Some("周报"));
    assert_eq!(cs.context.get("audience").map(String::as_str), Some("团队"));
}

#[tokio::test]
async fn skip_choice_still_advances_milestone() {
    let mut engine = engine_with_plan(&doc_generation_plan_json());
    engine.ensure_combined_plan("帮我写周报").await.unwrap();

    let result = engine.apply_choice_result("call-1", &[], true);

    // skip=true 仍然推进状态机
    assert_eq!(result.completed_milestone_id.as_deref(), Some("ms_0"));
    assert_eq!(result.next_milestone_id.as_deref(), Some("ms_1"));

    let cs = engine.conv_state.as_ref().unwrap();
    assert_eq!(cs.milestones[0].1, MilestoneStatus::Done);
    assert_eq!(cs.milestones[1].1, MilestoneStatus::Active);
}

// === Slash 命令回退 ===

#[tokio::test]
async fn slash_back_rewinds_to_previous_milestone() {
    let mut engine = engine_with_plan(&doc_generation_plan_json());
    engine.ensure_combined_plan("帮我写周报").await.unwrap();

    // 走过 ms_0 和 ms_1
    engine.apply_choice_result(
        "c1",
        &[UserChoiceAnswer {
            id: "doc_type".into(),
            label: "周报".into(),
            value: "周报".into(),
        }],
        false,
    );
    // 模拟 ms_1 也完成（在真实流程中由 contract validator 推进）
    {
        let cs = engine.conv_state.as_mut().unwrap();
        let active_id = cs.active_milestone().unwrap().id.clone();
        cs.mark_done(&active_id);
    }
    assert_eq!(
        engine.conv_state.as_ref().unwrap().active_milestone().unwrap().id,
        "ms_2"
    );

    // /back 应该回到 ms_1（最后一个 Done 的）
    let cs = engine.conv_state.as_mut().unwrap();
    let outcome = rollback::execute(SlashCommand::Back, cs);
    assert!(outcome.state_changed);
    assert_eq!(cs.milestones[1].1, MilestoneStatus::Active);
    assert_eq!(cs.milestones[2].1, MilestoneStatus::Pending);
}

#[tokio::test]
async fn slash_replan_clears_milestones_and_flags_replan() {
    let mut engine = engine_with_plan(&doc_generation_plan_json());
    engine.ensure_combined_plan("帮我写周报").await.unwrap();
    // 收集点 context 模拟历史
    engine.apply_choice_result(
        "c1",
        &[UserChoiceAnswer {
            id: "k".into(),
            label: "v".into(),
            value: "v".into(),
        }],
        false,
    );

    let cs = engine.conv_state.as_mut().unwrap();
    let outcome = rollback::execute(SlashCommand::Replan, cs);

    assert!(outcome.state_changed);
    assert!(outcome.trigger_replan);
    assert_eq!(cs.global_mode, GlobalMode::Replan);
    assert!(cs.milestones.is_empty());
    assert!(!cs.plan_initialized);
    // context 保留（让新计划自己决定哪些有用）
    assert!(cs.context.contains_key("k"));
}

#[tokio::test]
async fn slash_use_switches_agent_in_qa_mode() {
    let mut engine = engine_with_plan(r#"{"agent": "qa", "milestones": []}"#);
    engine.ensure_combined_plan("hello").await.unwrap();

    let cs = engine.conv_state.as_mut().unwrap();
    let outcome = rollback::execute(SlashCommand::Use("planning".into()), cs);

    assert!(outcome.state_changed);
    assert_eq!(outcome.switch_agent.as_deref(), Some("planning"));
    assert_eq!(cs.agent_id.as_deref(), Some("planning"));
}

#[tokio::test]
async fn slash_use_blocked_when_in_executing_mode() {
    let mut engine = engine_with_plan(&doc_generation_plan_json());
    engine.ensure_combined_plan("帮我写周报").await.unwrap();

    let cs = engine.conv_state.as_mut().unwrap();
    let outcome = rollback::execute(SlashCommand::Use("planning".into()), cs);

    assert!(!outcome.state_changed);
    assert!(outcome.message.contains("/replan"));
    // agent_id 不变
    assert_eq!(cs.agent_id.as_deref(), Some("doc_generation"));
}

// === Harness streaming（验证基础流式行为）===

#[tokio::test]
async fn mock_harness_chat_stream_emits_text_and_done() {
    let mock = MockHarness::with_responses(vec!["Hello, world!".into()]);
    let req = ChatRequest {
        user_message: "hi".into(),
        platform_system_prompt: None,
        context: Default::default(),
        tools: vec![],
        model: None,
        session_id: None,
        previous_messages: vec![],
    };

    let mut stream = mock.chat_stream(req).await.unwrap();
    let mut text = String::new();
    let mut got_done = false;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            StreamEvent::TextDelta { content } => text.push_str(&content),
            StreamEvent::Done => got_done = true,
            _ => {}
        }
    }
    assert_eq!(text, "Hello, world!");
    assert!(got_done);
}
