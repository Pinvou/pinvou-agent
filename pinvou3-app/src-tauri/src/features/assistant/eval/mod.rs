//! PinvouChatRunner: 评测 Runner，驱动 ProductChatRuntime 执行 EvalCase。
//!
//! 设计原则（低耦合）：
//! - 泛型 <R: ProductChatRuntime>，不 hardcode EnginePoolRuntime
//! - EvalCase/EvalRecord 是产品级类型，不引用内部引擎类型
//! - Runner 只拥有 case 隔离、超时、事件采集、失败分类，不拥有模型/工具/评分
//! - mock runtime 可替换 EnginePoolRuntime 用于 CI 确定性测试

pub(crate) mod analysis;
pub(crate) mod cases;
pub(crate) mod markdown_report;
pub(crate) mod mock;
pub(crate) mod report;

use anyhow::Result;
use deepseek_tui::tui::app::AppMode;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use self::analysis::EvalModelSelection;
use crate::features::assistant::product_runtime::{
    ProductChatRuntime, RuntimeToolEvent, SessionSpec, TurnInput, TurnResult,
};
use crate::features::assistant::timing::{TimelineEvent, TurnUsage};

/// 评测任务（benchmark adapter 产出，Runner 消费）
pub struct EvalCase {
    pub case_id: String,
    pub user_message: String,
    pub mode: AppMode,
    pub restrict_tools: bool,
    pub timeout_ms: u64,
    pub tool_expectation: ToolExpectation,
}

#[derive(Debug, Clone, Default)]
pub struct EvalAnalysisMaterial {
    pub user_message: String,
    pub assistant_text: String,
    pub tool_events: Vec<EvalToolEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalToolEvent {
    pub name: String,
    pub failed: bool,
}

impl From<RuntimeToolEvent> for EvalToolEvent {
    fn from(event: RuntimeToolEvent) -> Self {
        Self {
            name: event.name,
            failed: event.failed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExpectation {
    Forbidden,
    Optional,
    Required,
}

/// 评测执行口径。
///
/// `Product` 运行完整 Pinvou 产品链路；`OfficialCompatible` 为后续 BFCL adapter
/// 预留，当前入口不得把 Product 结果当作公开榜单成绩。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalMode {
    Product,
    OfficialCompatible,
}

impl EvalCase {
    /// 创建一个简单的 smoke case
    pub fn smoke(case_id: &str, message: &str) -> Self {
        Self {
            case_id: case_id.to_string(),
            user_message: message.to_string(),
            mode: AppMode::Yolo,
            restrict_tools: false,
            timeout_ms: 60_000,
            tool_expectation: ToolExpectation::Optional,
        }
    }
}

/// 评测记录（Runner 产出，scorer 消费）
#[derive(Debug, Serialize, Deserialize)]
pub struct EvalRecord {
    pub case_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub status: String,
    #[serde(skip)]
    pub error: Option<String>,
    pub usage: Option<TurnUsage>,
    pub milestones: Vec<EvalMilestone>,
    pub elapsed_ms: u64,
    #[serde(skip)]
    pub analysis: EvalAnalysisMaterial,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EvalMilestone {
    pub event: String,
    pub timestamp: i64,
    pub ts: String,
    pub tool_name: Option<String>,
    pub tool_id: Option<String>,
}

impl From<TimelineEvent> for EvalMilestone {
    fn from(event: TimelineEvent) -> Self {
        Self {
            event: event.event,
            timestamp: event.timestamp,
            ts: event.ts,
            tool_name: event.tool_name,
            tool_id: event.tool_id,
        }
    }
}

/// 一次批量评测的完整结果，保持与输入 case 相同的顺序。
pub struct EvalSuiteResult {
    pub records: Vec<Result<EvalRecord>>,
}

impl EvalSuiteResult {
    /// 只有每条 case 都产生 Completed 记录时才视为整批成功。
    pub fn all_succeeded(&self) -> bool {
        !self.records.is_empty()
            && self.records.iter().all(|record| {
                record
                    .as_ref()
                    .is_ok_and(|record| record.status.eq_ignore_ascii_case("completed"))
            })
    }
}

/// 评测 Runner：驱动 ProductChatRuntime 执行 EvalCase 并收集 EvalRecord。
///
/// 泛型参数 R 允许替换为 mock runtime 用于 CI 确定性测试。
pub struct PinvouChatRunner<R: ProductChatRuntime> {
    runtime: R,
}

impl<R: ProductChatRuntime> PinvouChatRunner<R> {
    pub fn new(runtime: R) -> Self {
        Self { runtime }
    }

    /// 执行单个 EvalCase，返回 EvalRecord。
    ///
    /// 流程：prepare → submit → wait_for_completion（带超时）→ close
    /// 超时触发 cancel，仍返回带 timeout 状态的 EvalRecord。
    pub async fn run_case(&self, case: &EvalCase) -> Result<EvalRecord> {
        self.run_case_with_selection(case, None).await
    }

    async fn run_case_with_selection(
        &self,
        case: &EvalCase,
        model_selection: Option<EvalModelSelection>,
    ) -> Result<EvalRecord> {
        let session_id = unique_eval_session_id(&case.case_id);

        if let Err(error) = self
            .runtime
            .prepare(&SessionSpec {
                session_id: session_id.clone(),
                model_selection,
            })
            .await
        {
            self.runtime.close(&session_id).await;
            return Err(error);
        }

        let start = Instant::now();
        let handle = match self
            .runtime
            .submit(&TurnInput {
                session_id: session_id.clone(),
                content: case.user_message.clone(),
                mode: case.mode,
                restrict_tools: case.restrict_tools,
                eval_tool_policy: None,
            })
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                self.runtime.close(&session_id).await;
                return Err(error);
            }
        };

        let result = tokio::time::timeout(
            Duration::from_millis(case.timeout_ms),
            self.runtime.wait_for_completion(&handle),
        )
        .await;

        let elapsed_ms = start.elapsed().as_millis() as u64;

        let record = match result {
            Ok(Ok(turn_result)) => Ok(to_record(case, &session_id, turn_result, elapsed_ms)),
            Ok(Err(e)) => Ok(EvalRecord {
                case_id: case.case_id.clone(),
                session_id: session_id.clone(),
                turn_id: handle.turn_id,
                status: "runner_error".to_string(),
                error: Some(e.to_string()),
                usage: None,
                milestones: Vec::new(),
                elapsed_ms,
                analysis: EvalAnalysisMaterial {
                    user_message: case.user_message.clone(),
                    ..Default::default()
                },
            }),
            Err(_) => {
                self.runtime.cancel(&session_id).await;
                Ok(EvalRecord {
                    case_id: case.case_id.clone(),
                    session_id: session_id.clone(),
                    turn_id: handle.turn_id,
                    status: "timeout".to_string(),
                    error: Some(format!("timeout after {}ms", case.timeout_ms)),
                    usage: None,
                    milestones: Vec::new(),
                    elapsed_ms,
                    analysis: EvalAnalysisMaterial {
                        user_message: case.user_message.clone(),
                        ..Default::default()
                    },
                })
            }
        };
        self.runtime.close(&session_id).await;
        record
    }

    /// 批量执行，返回所有记录（失败不中断，继续下一个 case）
    pub async fn run_cases(&self, cases: &[EvalCase]) -> Vec<Result<EvalRecord>> {
        let mut results = Vec::with_capacity(cases.len());
        for case in cases {
            results.push(self.run_case(case).await);
        }
        results
    }
}

fn unique_eval_session_id(case_id: &str) -> String {
    static NEXT_SESSION: AtomicU64 = AtomicU64::new(0);
    let case_component = case_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .take(16)
        .collect::<String>();
    format!(
        "eval_{}_{}_{}",
        case_component,
        std::process::id(),
        NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
    )
}

/// 顺序执行一组评测 case，并在每条记录产生后立即通知调用方。
///
/// callback 可用于增量写报告；callback 失败属于批次级错误，会立即终止，避免继续
/// 运行却丢失记录。单条 case 的 provider/runner 错误仍作为记录保留并继续下一条。
pub async fn run_eval_suite<R, F>(
    runtime: R,
    cases: &[EvalCase],
    on_record: F,
) -> Result<EvalSuiteResult>
where
    R: ProductChatRuntime,
    F: FnMut(&EvalCase, &Result<EvalRecord>) -> Result<()>,
{
    run_eval_suite_with_model_factory(runtime, cases, |_| Ok(None), on_record).await
}

pub async fn run_eval_suite_with_model_factory<R, M, F>(
    runtime: R,
    cases: &[EvalCase],
    mut model_for_case: M,
    mut on_record: F,
) -> Result<EvalSuiteResult>
where
    R: ProductChatRuntime,
    M: FnMut(&EvalCase) -> Result<Option<EvalModelSelection>>,
    F: FnMut(&EvalCase, &Result<EvalRecord>) -> Result<()>,
{
    let runner = PinvouChatRunner::new(runtime);
    let mut records = Vec::with_capacity(cases.len());
    for case in cases {
        let selection = model_for_case(case)?;
        let record = runner.run_case_with_selection(case, selection).await;
        on_record(case, &record)?;
        records.push(record);
    }
    Ok(EvalSuiteResult { records })
}

fn to_record(case: &EvalCase, session_id: &str, turn: TurnResult, elapsed_ms: u64) -> EvalRecord {
    let analysis = EvalAnalysisMaterial {
        user_message: case.user_message.clone(),
        assistant_text: turn.assistant_text,
        tool_events: turn
            .tool_events
            .into_iter()
            .map(EvalToolEvent::from)
            .collect(),
    };
    EvalRecord {
        case_id: case.case_id.clone(),
        session_id: session_id.to_string(),
        turn_id: turn.turn_id,
        status: turn.status,
        error: turn.error,
        usage: turn.usage,
        milestones: turn
            .milestones
            .into_iter()
            .map(EvalMilestone::from)
            .collect(),
        elapsed_ms,
        analysis,
    }
}

#[cfg(test)]
mod tests;
