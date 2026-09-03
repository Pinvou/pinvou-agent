//! 原生 code 会话 turn 计数口径：直接复用底座 `is_user_turn_prompt`。
//!
//! tool_result 与运行时内部信封同样以 `role = "user"` 落盘，按 role 计数会把
//! 它们误算成用户 turn（feat 分支 `32b5fdf9e` 的 turn 号错位 bug 即源于此）。
//! 正确口径由 fork 提交 `8cc61b609` 在 `CodeWhale/crates/tui/src/runtime_handoff.rs`
//! 提供并根级重导出，本分支已合入该 gitlink（merge pr-320），故不再 vendored
//! 副本。同口径使用方：底座 `Op::EditLastTurn`、store 落盘兜底
//! `persist_admitted_chat_display`、本模块 turn 计数（设计文档
//! `docs/code-mode-改动随对话回退-设计.md` §3「移植时的修正」）。

use deepseek_tui::is_user_turn_prompt;
use deepseek_tui::models::Message;

/// 会话日志中的真实用户 turn 数（不含 tool_result 与运行时内部信封）。
/// checkpoint 的 turn 序号 = 本计数 + 1（当前 turn 尚未落盘时调用）。
pub(crate) fn count_user_turns(messages: &[Message]) -> u32 {
    messages
        .iter()
        .filter(|message| is_user_turn_prompt(message))
        .count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseek_tui::models::ContentBlock;

    fn text_message(role: &str, text: &str) -> Message {
        Message {
            role: role.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn tool_result_message() -> Message {
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "tool output".to_string(),
                is_error: None,
                content_blocks: None,
            }],
        }
    }

    /// turn 计数口径（feat 分支 bug）：一轮带工具往返的对话只算 1 个 turn。
    #[test]
    fn count_user_turns_ignores_tool_results() {
        let messages = vec![
            text_message("user", "第一轮"),
            text_message("assistant", "调工具"),
            tool_result_message(),
            tool_result_message(),
            text_message("assistant", "答一"),
            text_message("user", "第二轮"),
            tool_result_message(),
        ];
        assert_eq!(count_user_turns(&messages), 2);
    }
}
