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

/// Exact user-turn count over raw JSON messages. Headless callers (the CLI)
/// do not link the foundation crate and only hold the JSON message array
/// before deserialization; the CLI previously approximated the count from
/// the role + tool_result shape, which over-counts messages the engine does
/// not treat as prompts (image-only turns) and could wedge the
/// `checkpoints rewind` precheck. Each message is deserialized and run
/// through the exact same predicate as `count_user_turns`, so the two sides
/// can no longer drift.
pub fn count_user_turns_in_json(messages: &[serde_json::Value]) -> Result<u32, serde_json::Error> {
    let mut total = 0u32;
    for value in messages {
        let message: Message = serde_json::from_value(value.clone())?;
        total += count_user_turns(std::slice::from_ref(&message));
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseek_tui::models::ContentBlock;

    fn text_message(role: &str, text: &str) -> Message {
        Message {
            role: role.into(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn tool_result_message() -> Message {
        Message {
            role: deepseek_tui::models::Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "tool output".to_string(),
                is_error: None,
                content_blocks: None,
            }],
        }
    }

    fn image_only_user_message() -> Message {
        Message {
            role: deepseek_tui::models::Role::User,
            content: vec![ContentBlock::ImageUrl {
                image_url: deepseek_tui::models::ImageUrlContent {
                    url: "file:///staged/attachments/shot.png".to_string(),
                },
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

    /// The JSON entry point and the in-memory slice must agree: after a
    /// serde round-trip, the same messages give `count_user_turns_in_json`
    /// the same value as `count_user_turns`. The "CLI and engine no longer
    /// drift" promise lands exactly on this deserialization step.
    #[test]
    fn count_user_turns_in_json_matches_in_memory_counting() {
        let messages = vec![
            text_message("user", "第一轮"),
            text_message("assistant", "调工具"),
            tool_result_message(),
            text_message("user", "第二轮"),
            // The motivating drift case: an image-only user turn is not a
            // prompt turn — the CLI's role-based approximation over-counted
            // exactly this shape and wedged the `checkpoints rewind` precheck.
            image_only_user_message(),
        ];
        let json: Vec<serde_json::Value> = messages
            .iter()
            .map(|message| serde_json::to_value(message).unwrap())
            .collect();
        assert_eq!(
            count_user_turns_in_json(&json).unwrap(),
            count_user_turns(&messages)
        );
        assert_eq!(
            count_user_turns_in_json(&json).unwrap(),
            2,
            "image-only turns must not count as user turns"
        );

        // Non-message payloads must error instead of silently counting 0,
        // aligning with the engine's "session load failed" error model (the
        // doc promises Err on deserialization failure).
        let invalid = vec![serde_json::json!({ "role": 42 })];
        assert!(count_user_turns_in_json(&invalid).is_err());
    }

    /// 工作车道附件-only 发送的真实落盘形态必须计为用户 turn（评审 M1 对齐 pin）。
    /// 两个附件构造器（`prepare_native_user_message_in_dir` /
    /// `build_message_with_attachments_in_dir`）对空草稿文本也总是生成非空包装段
    /// （"---\n用户附上了以下文件:\n\n…"），引擎 `user_content_blocks` 把该完整
    /// 文本作为第一个 Text 块落盘、ImageUrl 与 turn_meta 随后追加。因此工作车道
    /// 的附件-only 轮次在 `classify_user_turn_prompt` 下是 Editable——后端计数
    /// **包含**它们，与前端 `rewindEntriesByTurnId` 逐 userItem 计数对齐（若任
    /// 一侧改变——比如包装文本被剥离、该形态退化成上面的裸 ImageUrl fixture——
    /// 本 pin 与 image-only fixture 的计数差即暴露错位）。
    #[test]
    fn attachment_wrapped_image_turn_counts_as_user_turn() {
        let wrapper = "---\n用户附上了以下文件:\n\n### shot.png (image, 12345 bytes)\n\
                       [Attached image: /ws/attachments/shot.png]\n\n---\n";
        let wrapped = Message {
            role: deepseek_tui::models::Role::User,
            content: vec![
                ContentBlock::Text {
                    text: wrapper.to_string(),
                    cache_control: None,
                },
                ContentBlock::ImageUrl {
                    image_url: deepseek_tui::models::ImageUrlContent {
                        url: "file:///ws/attachments/shot.png".to_string(),
                    },
                },
            ],
        };
        assert!(is_user_turn_prompt(&wrapped));
        assert_eq!(count_user_turns(std::slice::from_ref(&wrapped)), 1);

        // 对照组：同一张图去掉包装文本（CLI 车道的裸形态）不计——证明两个
        // fixture 之间隔着的正是构造器的包装文本，而非图片块本身。
        let bare = Message {
            role: deepseek_tui::models::Role::User,
            content: vec![ContentBlock::ImageUrl {
                image_url: deepseek_tui::models::ImageUrlContent {
                    url: "file:///ws/attachments/shot.png".to_string(),
                },
            }],
        };
        assert!(!is_user_turn_prompt(&bare));
        assert_eq!(count_user_turns(std::slice::from_ref(&bare)), 0);
    }
}
