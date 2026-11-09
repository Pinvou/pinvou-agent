//! 原生 code 会话 turn 计数口径：底座 era 形 `user_turn_prompt_indices`。
//!
//! tool_result 与运行时内部信封同样以 `role = "user"` 落盘，按 role 计数会把
//! 它们误算成用户 turn（feat 分支 `32b5fdf9e` 的 turn 号错位 bug 即源于此）。
//! 底座文档（`CodeWhale/crates/tui/src/runtime_handoff.rs`
//! `user_turn_prompt_indices` 契约）明确：host turn 编号、回退切点与 turn
//! 计数都应统一读 era 形列表——单消息 `is_user_turn_prompt` 逐条分类会漏计
//! post-provenance 历史里粘贴的压缩摘要头 turn。sessions 侧 rewind/fork
//! （`features::sessions`）同样读这一个列表，checkpoint 序号必须与之同口径，
//! 否则「第 N 轮」在 checkpoint 卡片与回退/分叉入口之间整体错位。计数使用方：
//! 底座 `Op::EditLastTurn`、store 落盘兜底 `persist_admitted_chat_display`、
//! 本模块 turn 计数（设计文档 `docs/code-mode-改动随对话回退-设计.md` §3
//! 「移植时的修正」）。
//!
//! 唯一在 era 列表之上追加的剔除：步数预算通知（ExternalUser 落盘、turn_meta
//! 无 non-authoritative 标记，era 口径会误计——见
//! `is_step_budget_notification`）。计数与截断边界统一走
//! `user_turn_prompt_cut_indices`。

use deepseek_tui::models::{ContentBlock, Message, Role};
use deepseek_tui::user_turn_prompt_indices;

/// 步数预算通知判定（评审 r10 M3）：底座把 soft landing / exhaustion 两条通知
/// 以 ExternalUser provenance 落盘（engine.rs 硬编码，上游待修）。ExternalUser
/// 可授权工作（`ops.rs can_authorize_work`），其 turn_meta 不带
/// non-authoritative 标记，era 口径因此仍把它们计成 Editable turn——识别
/// 前缀与前端双 bridge / code 车道同款镜像，统一在此剔除。
pub fn is_step_budget_notification(message: &Message) -> bool {
    if message.role != Role::User {
        return false;
    }
    let Some(ContentBlock::Text { text, .. }) = message.content.first() else {
        return false;
    };
    text.starts_with("Step budget soft landing:")
        || text.starts_with("Your model-step budget was exhausted")
}

/// 统一口径（快照序号、回退预检、调和门、截断边界与 undo 计数）：底座 era 形
/// `user_turn_prompt_indices` 再剔除步数预算通知。era 口径比单消息
/// `is_user_turn_prompt` 多计入 post-provenance 历史里粘贴的压缩摘要头
/// turn——底座文档明确 host turn 编号应统一读这一个列表（fork 分叉同一
/// 口径）。era 由传入的完整消息列表推导；对被截段的 `removed_messages`
/// 切片，era 取切片自身（通常不含 provenance 载体，退回宽松口径）。
pub(crate) fn user_turn_prompt_cut_indices(messages: &[Message]) -> Vec<usize> {
    user_turn_prompt_indices(messages)
        .into_iter()
        .filter(|&index| !is_step_budget_notification(&messages[index]))
        .collect()
}

/// 会话日志中的真实用户 turn 数。checkpoint 的 turn 序号 = 本计数 + 1
/// （当前 turn 尚未落盘时调用）。
pub(crate) fn count_user_turns(messages: &[Message]) -> u32 {
    user_turn_prompt_cut_indices(messages).len() as u32
}
}

/// Exact user-turn count over raw JSON messages. Headless callers (the CLI)
/// do not link the foundation crate and only hold the JSON message array
/// before deserialization; the CLI previously approximated the count from
/// the role + tool_result shape, which over-counts messages the engine does
/// not treat as prompts (image-only turns) and could wedge the
/// `checkpoints rewind` precheck. Every message is deserialized and the
/// whole array run through the exact same era-form predicate as
/// `count_user_turns`, so the two sides can no longer drift.
pub fn count_user_turns_in_json(messages: &[serde_json::Value]) -> Result<u32, serde_json::Error> {
    let mut parsed = Vec::with_capacity(messages.len());
    for value in messages {
        parsed.push(serde_json::from_value::<Message>(value.clone())?);
    }
    Ok(count_user_turns(&parsed))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// era 口径：pre-provenance 历史（无 provenance 载体）里粘贴的压缩摘要头
    /// turn 按宽松口径读作已保存摘要，不计入 turn 数——与 sessions 侧
    /// rewind/fork 同为 `user_turn_prompt_indices`，三处口径一致。
    #[test]
    fn count_user_turns_skips_pasted_summary_header_in_pre_provenance_history() {
        // 底座 COMPACTION_SUMMARY_MARKER（history_recognition.rs:20，该模块
        // 私有，测试以字面量钉住契约）。
        const COMPACTION_SUMMARY_MARKER: &str =
            "Another language model started to solve this problem";
        let messages = vec![
            text_message("user", "第一轮"),
            text_message("assistant", "答"),
            text_message("user", COMPACTION_SUMMARY_MARKER),
            text_message("user", "第二轮"),
        ];
        assert_eq!(
            count_user_turns(&messages),
            2,
            "pasted summary-header turn must not count in a pre-provenance history"
        );
        assert_eq!(count_user_turns(&messages[..1]), 1);
    }

    /// post-provenance 方向的本侧钉子（底座
    /// `forkguard_user_turn_prompt_indices_are_era_aware` 的 app 层镜像）：
    /// 含 stamped 载体的历史里，粘贴的摘要头 turn **计入** turn 数。
    /// `count_user_turns` 是 `user_turn_prompt_indices` 的一行委托，底座已钉
    /// 该契约；这里本地复钉，防止未来退回单消息宽松口径（会漏计粘贴头，
    /// fork/rewind 序号与底座 host 编号整体错位）。载体按底座
    /// `compaction_checkpoint_message` 的形状以字面量钉住（provenance 块为
    /// 底座 crate 私有，未导出）。
    #[test]
    fn count_user_turns_counts_pasted_header_in_post_provenance_history() {
        const COMPACTION_CHECKPOINT_PROVENANCE: &str =
            "<!-- codewhale.compaction-checkpoint.v1 -->";
        const COMPACTION_SUMMARY_MARKER: &str =
            "Another language model started to solve this problem";
        let stamped_carrier = Message {
            role: "user".into(),
            content: vec![
                ContentBlock::Text {
                    text: format!("{COMPACTION_SUMMARY_MARKER}\nhandoff"),
                    cache_control: None,
                },
                ContentBlock::Text {
                    text: COMPACTION_CHECKPOINT_PROVENANCE.to_string(),
                    cache_control: None,
                },
            ],
        };
        let messages = vec![
            text_message("user", "第一轮"),
            stamped_carrier,
            text_message("user", COMPACTION_SUMMARY_MARKER),
            text_message("user", "第二轮"),
        ];
        assert_eq!(
            count_user_turns(&messages),
            3,
            "post-provenance history counts the pasted header turn (base era contract)"
        );
    }

    /// 序号计数器的存在前提（r7 minor 10）：底座 era 分类把 canonical
    /// `<context_update>` 漂移增量记为 Editable turn 边界——前端计数器、
    /// fork/rewind 序号与 `count_context_updates` 全部建立在这一前提上。
    /// 底座若未来把它改判 runtime-owned（provenance/`<turn_meta>` 族），
    /// 本钉先红，防止整个机制反向静默漂移。消息形状按底座
    /// `context_update_message` 的合成（prefix_cache.rs）以字面量钉住。
    #[test]
    fn count_user_turns_counts_canonical_context_update_as_era_boundary() {
        const CONTEXT_UPDATE_PREFIX: &str = "<context_update>\nSession context changed since it was pinned; the pinned system prompt is unchanged. Delta (+ added, - removed):\n";
        let delta = Message {
            role: "user".into(),
            content: vec![ContentBlock::Text {
                text: format!("{CONTEXT_UPDATE_PREFIX}\n- removed line\n</context_update>"),
                cache_control: None,
            }],
        };
        let messages = vec![
            text_message("user", "第一轮"),
            delta,
            text_message("user", "第二轮"),
        ];
        assert_eq!(
            count_user_turns(&messages),
            3,
            "a canonical <context_update> drift delta is an Editable era boundary (base contract the ordinal counters are built on)"
        );
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
