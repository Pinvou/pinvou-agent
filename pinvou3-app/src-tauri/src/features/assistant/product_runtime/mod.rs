//! ProductChatRuntime: 产品等价的聊天 runtime seam。
//!
//! GUI 和评测 Runner 共同调用此 trait，确保评测走真实产品链路。
//! 当前实现薄包装 EnginePool——逻辑逐步从 EnginePool/AppEngine/Bridge
//! 迁入此处，但首期只建立 seam 不搬逻辑。
//!
//! 设计原则（低耦合）：
//! - trait 类型（SessionSpec/TurnInput/TurnHandle）不引用 EnginePool/Op/EngineConfig
//! - trait 窄接口：prepare/submit/is_active/cancel/close 共 5 个方法
//! - 实现是唯一知道 EnginePool 的地方；换实现不影响 trait 消费方
//! - 不引入循环依赖：product_runtime -> engine_pool（单向）

use anyhow::Result;
use deepseek_tui::models::{ContentBlock, Message};
use deepseek_tui::tui::app::AppMode;
use std::collections::HashMap;
use std::sync::Arc;

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::assistant::eval::{EvalModelSelection, EvalSuiteModelSnapshot};
use crate::features::assistant::timing;
use std::time::Duration;

pub(crate) mod eval_tool_policy;
#[cfg(feature = "benchmark-hooks")]
pub mod headless_bridge;

/// 会话创建规格
pub struct SessionSpec {
    pub session_id: String,
    pub model_selection: Option<EvalModelSelection>,
}

/// 单轮提交输入（产品级，不暴露 Op/EngineConfig）
pub struct TurnInput {
    pub session_id: String,
    pub content: String,
    pub mode: AppMode,
    pub restrict_tools: bool,
    pub eval_tool_policy: Option<eval_tool_policy::EvalToolPolicy>,
}

/// 单轮句柄（后续扩展为事件订阅入口）
pub struct TurnHandle {
    pub session_id: String,
    pub turn_id: String,
}

/// 单轮完成结果
pub struct TurnResult {
    pub turn_id: String,
    pub status: String,
    pub error: Option<String>,
    pub usage: Option<timing::TurnUsage>,
    pub milestones: Vec<timing::TimelineEvent>,
    pub assistant_text: String,
    pub tool_events: Vec<RuntimeToolEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeToolEvent {
    pub name: String,
    pub failed: bool,
}

fn extract_turn_analysis(messages: &[Message]) -> (String, Vec<RuntimeToolEvent>) {
    let assistant_text = messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .unwrap_or_default();

    let failures = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                is_error,
                ..
            } => Some((tool_use_id.as_str(), is_error.unwrap_or(false))),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let tool_events = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, .. } => Some(RuntimeToolEvent {
                name: name.clone(),
                failed: failures.get(id.as_str()).copied().unwrap_or(false),
            }),
            _ => None,
        })
        .collect();

    (assistant_text, tool_events)
}

/// 产品聊天 runtime seam。
///
/// GUI 和 headless 评测宿主都通过此 trait 驱动会话，
/// 确保评测与生产走同一条构造路径。
pub trait ProductChatRuntime: Send + Sync {
    /// 确保会话已创建并就绪
    async fn prepare(&self, spec: &SessionSpec) -> Result<()>;

    /// 提交用户消息，返回轮次句柄
    async fn submit(&self, input: &TurnInput) -> Result<TurnHandle>;

    /// 当前是否有活跃轮次
    fn is_turn_active(&self, session_id: &str) -> bool;

    /// 等待轮次完成并返回结果（轮询实现，后续可升级为事件流）
    async fn wait_for_completion(&self, handle: &TurnHandle) -> Result<TurnResult>;

    /// 取消当前轮次
    async fn cancel(&self, session_id: &str);

    /// 删除本次评测的临时会话（释放引擎资源且不污染用户历史）
    async fn close(&self, session_id: &str);
}

/// EnginePool 薄包装实现。
///
/// 首期只是委托调用，不添加逻辑。后续产品语义（路由策略、
/// 事件归一化、工具策略等）逐步从 EnginePool/AppEngine/Bridge
/// 迁入此处，trait 消费方不受影响。
#[derive(Clone)]
pub struct EnginePoolRuntime {
    pool: Arc<EnginePool>,
}

pub(crate) struct EvalSuiteModelGuard {
    runtime: EnginePoolRuntime,
    snapshot: EvalSuiteModelSnapshot,
}

impl EvalSuiteModelGuard {
    pub(crate) fn identity(&self) -> &crate::features::assistant::eval::ModelIdentity {
        self.snapshot.identity()
    }

    pub(crate) fn derive_case_selection(&self) -> Result<EvalModelSelection> {
        self.runtime
            .derive_eval_suite_case_selection(&self.snapshot)
    }
}

impl Drop for EvalSuiteModelGuard {
    fn drop(&mut self) {
        self.runtime.discard_eval_suite_model(&self.snapshot);
    }
}

impl EnginePoolRuntime {
    pub fn new(pool: Arc<EnginePool>) -> Self {
        Self { pool }
    }

    pub(crate) fn tested_eval_identity(&self) -> crate::features::assistant::eval::ModelIdentity {
        self.pool.tested_eval_identity()
    }

    pub(crate) fn pin_active_eval_suite_model(&self) -> Result<EvalSuiteModelSnapshot> {
        self.pool.pin_active_eval_suite_model()
    }

    pub(crate) fn capture_eval_suite_model(&self) -> Result<EvalSuiteModelGuard> {
        Ok(EvalSuiteModelGuard {
            runtime: self.clone(),
            snapshot: self.pin_active_eval_suite_model()?,
        })
    }

    pub(crate) fn derive_eval_suite_case_selection(
        &self,
        suite: &EvalSuiteModelSnapshot,
    ) -> Result<EvalModelSelection> {
        self.pool.derive_eval_suite_case_selection(suite)
    }

    pub(crate) fn discard_eval_suite_model(&self, suite: &EvalSuiteModelSnapshot) {
        self.pool.discard_eval_suite_model(suite);
    }

    pub(crate) fn pin_eval_model_selection(&self, model_id: &str) -> Result<EvalModelSelection> {
        self.pool.pin_eval_model_selection(model_id)
    }

    pub(crate) fn discard_eval_model_selection(&self, selection: &EvalModelSelection) {
        self.pool.discard_eval_model_selection(selection);
    }

    pub(crate) async fn close_eval_session_result(&self, session_id: &str) -> Result<()> {
        self.pool.delete_eval_session(session_id).await
    }

    pub(crate) fn eval_session_execution_root(
        &self,
        session_id: &str,
    ) -> Result<std::path::PathBuf> {
        self.pool.eval_session_execution_root(session_id)
    }

    pub(crate) fn schedule_eval_cleanup(&self, session_id: &str) {
        self.pool.schedule_eval_cleanup(session_id);
    }
}

impl ProductChatRuntime for EnginePoolRuntime {
    async fn prepare(&self, spec: &SessionSpec) -> Result<()> {
        self.pool
            .prepare_eval_session(&spec.session_id, spec.model_selection.as_ref())
            .await
    }

    async fn submit(&self, input: &TurnInput) -> Result<TurnHandle> {
        let turn_id = timing::start_turn(&input.session_id);
        if let Some(policy_id) = input.eval_tool_policy {
            let policy = match policy_id {
                eval_tool_policy::EvalToolPolicy::ProductV1 => {
                    eval_tool_policy::resolve_eval_policy("pinvou-product/v1")?
                }
                eval_tool_policy::EvalToolPolicy::GaiaPublicWebV1 => {
                    eval_tool_policy::resolve_eval_policy("pinvou-gaia-public-web/v1")?
                }
                eval_tool_policy::EvalToolPolicy::GaiaOfflineV1 => {
                    eval_tool_policy::resolve_eval_policy("pinvou-gaia-offline/v1")?
                }
            };
            self.pool
                .send_eval_user_message(&input.session_id, input.content.clone(), policy)
                .await?;
        } else {
            self.pool
                .send_user_message(
                    &input.session_id,
                    input.content.clone(),
                    input.mode,
                    input.restrict_tools,
                )
                .await?;
        }
        Ok(TurnHandle {
            session_id: input.session_id.clone(),
            turn_id,
        })
    }

    fn is_turn_active(&self, session_id: &str) -> bool {
        self.pool.is_turn_active(session_id)
    }

    async fn wait_for_completion(&self, handle: &TurnHandle) -> Result<TurnResult> {
        let session_id = &handle.session_id;
        while self.pool.is_turn_active(session_id) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let timeline =
            timing::read_timeline(session_id).map_err(|e| anyhow::anyhow!("read timeline: {e}"))?;
        let entry = timeline
            .iter()
            .rev()
            .find(|e| e.turn_id == handle.turn_id && e.event == "assistant_done");
        let milestones = timeline
            .iter()
            .filter(|event| {
                event.turn_id == handle.turn_id
                    && !matches!(event.event.as_str(), "user_start" | "assistant_done")
            })
            .cloned()
            .collect();
        let transcript = self.pool.load_eval_transcript(session_id)?;
        let (assistant_text, tool_events) = extract_turn_analysis(&transcript);
        Ok(TurnResult {
            turn_id: handle.turn_id.clone(),
            status: entry
                .and_then(|e| e.status.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            error: entry.and_then(|e| e.error.clone()),
            usage: entry.and_then(|e| e.usage),
            milestones,
            assistant_text,
            tool_events,
        })
    }

    async fn cancel(&self, session_id: &str) {
        // 评测/headless 的取消即停止语义：不保留未注入 steer（StopDropInbox）。
        self.pool.cancel(session_id, false).await;
    }

    async fn close(&self, session_id: &str) {
        if let Err(error) = self.pool.delete_chat_session(session_id).await {
            eprintln!("[eval] failed to delete temporary session {session_id}: {error:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::extract_turn_analysis;
    use deepseek_tui::models::{ContentBlock, Message};
    use serde_json::json;

    #[test]
    fn transcript_analysis_extracts_only_last_assistant_text_and_tool_metadata() {
        let messages = vec![
            Message {
                role: "assistant".into(),
                content: vec![ContentBlock::Text {
                    text: "older answer".into(),
                    cache_control: None,
                }],
            },
            Message {
                role: "assistant".into(),
                content: vec![ContentBlock::ToolUse {
                    id: "tool-1".into(),
                    name: "weather".into(),
                    input: json!({"query": "secret tool input"}),
                    caller: None,
                }],
            },
            Message {
                role: "user".into(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool-1".into(),
                    content: "secret tool output".into(),
                    is_error: Some(true),
                    content_blocks: None,
                }],
            },
            Message {
                role: "assistant".into(),
                content: vec![
                    ContentBlock::Text {
                        text: "final ".into(),
                        cache_control: None,
                    },
                    ContentBlock::Text {
                        text: "answer".into(),
                        cache_control: None,
                    },
                ],
            },
        ];

        let (assistant_text, tool_events) = extract_turn_analysis(&messages);

        assert_eq!(assistant_text, "final answer");
        assert_eq!(tool_events.len(), 1);
        assert_eq!(tool_events[0].name, "weather");
        assert!(tool_events[0].failed);
        let debug = format!("{tool_events:?}");
        assert!(!debug.contains("secret tool input"));
        assert!(!debug.contains("secret tool output"));
    }
}
