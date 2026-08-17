//! PLEP (Pinvou Local Evaluation Pack) 任务定义。
//!
//! 首批 5 条 smoke case，覆盖闲聊/实时查询/创作/算术/日期，
//! 用于验证评测管道端到端 + 建立性能基线。
//! 后续扩充到 60 条（PLEP full）。

use deepseek_tui::tui::app::AppMode;

use super::{EvalCase, ToolExpectation};

/// 返回首批 5 条 PLEP smoke case。
pub fn smoke_cases() -> Vec<EvalCase> {
    vec![
        EvalCase {
            case_id: "plep_smoke_hi".to_string(),
            user_message: "hi".to_string(),
            mode: AppMode::Yolo,
            restrict_tools: false,
            timeout_ms: 30_000,
            tool_expectation: ToolExpectation::Forbidden,
        },
        EvalCase {
            case_id: "plep_smoke_weather".to_string(),
            user_message: "广州今天天气怎么样".to_string(),
            mode: AppMode::Yolo,
            restrict_tools: false,
            timeout_ms: 60_000,
            tool_expectation: ToolExpectation::Required,
        },
        EvalCase {
            case_id: "plep_smoke_math".to_string(),
            user_message: "1+1等于几".to_string(),
            mode: AppMode::Yolo,
            restrict_tools: false,
            timeout_ms: 30_000,
            tool_expectation: ToolExpectation::Forbidden,
        },
        EvalCase {
            case_id: "plep_smoke_poem".to_string(),
            user_message: "帮我写一首关于春天的诗".to_string(),
            mode: AppMode::Yolo,
            restrict_tools: false,
            timeout_ms: 60_000,
            tool_expectation: ToolExpectation::Forbidden,
        },
        EvalCase {
            case_id: "plep_smoke_date".to_string(),
            user_message: "今天星期几".to_string(),
            mode: AppMode::Yolo,
            restrict_tools: false,
            timeout_ms: 30_000,
            tool_expectation: ToolExpectation::Optional,
        },
    ]
}
