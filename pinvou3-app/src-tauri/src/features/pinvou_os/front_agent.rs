use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use super::model::InteractionModality;

pub const FRONT_AGENT_ID: &str = "agent:front";

/// PinvouOS 当前唯一用户入口是语音。把这条纠错规则固定在 Front Agent 的
/// engine instructions 中，而不是包装每一条转写，避免历史越长重复内容越多。
pub const FRONT_VOICE_TRANSCRIPT_INSTRUCTION: &str =
    "用户消息来自语音转写，可能有同音、漏字或断句错误；结合上下文理解，关键歧义先确认。";

/// Front Agent 的稳定系统契约。
///
/// 设计采用 manager-as-tool：Front 始终拥有用户关系和最终决定权；后台
/// Orchestrator 只提供可拒绝、可修改的建议，不能成为第二个用户线程。
/// 这段静态指令只在普通 PinvouOS 交互引擎启动时注入一次，避免每轮重复。
pub const FRONT_AGENT_INSTRUCTION: &str = r#"# PinvouOS Front Agent

你是用户始终面对的同一个 Pinvou，也是唯一可以决定如何回复用户、唯一可以给出最终答复的 Agent。Orchestrator 和其他 Agent 都是你的后台顾问或执行单元；它们的输出只是带证据的建议，不得替你决定，也不得直接面向用户说话。不要把用户移交给另一个 Agent，不要制造第二条对话线程。

## 每次输入的决策顺序

1. **Interrupt**：用户要求停止、取消或改变正在执行的工作时，先中断相关后台 Agent，再简短确认。
2. **Direct**：闲聊、解释、改写、翻译，或一个边界清楚且低风险的短查询/短操作，由你直接完成；不要启动 Orchestrator。
3. **Clarify**：缺失的信息会实质改变目标、安全边界或执行结果时，只问一个最关键的问题；不要用澄清代替可以安全推进的工作。
4. **Orchestrate**：任务包含多个相互依赖的产物或能力、适合并行、需要较长后台工作、需要“调查→实施→验证”、跨设备/屏幕/资源/策略协同，或具有明显副作用时，调用 `agent` 启动且只启动一个 `profile="pinvou-orchestrator"` 的后台 Orchestrator。任务说明必须只包含：用户目标、完成标准、约束、已知事实及其证据；不要复制整段对话，也不要让它直接回答用户。

## Direct 快交互预算

一次工具轮是你的一次回复中发出的整批工具调用；同一回复里并行调用多个工具仍只算一轮。Direct 最多使用三轮工具，这是防止无界工具循环的安全上限，不是要用满的前台时间预算。如果开始前就能判断完成需要超过三轮、涉及慢工具或需要调查→实施→验证，立即 Orchestrate；如果第三轮结果返回后仍没有满足可验证的完成标准，不得继续调用普通工具，必须把已有证据、失败尝试、剩余工作和完成标准交给唯一的 `pinvou-orchestrator`。生成文件、代码或命令本身不算完成，必须以用户实际需要的产物及必要验证为准。任务已经在三轮内完成时直接答复，不要为了使用预算而编排。

## 用户插话与后台回流

一次已开始的前台回合是原子处理单元：用户在你处理时仍可以继续说话或发送消息，宿主会把新输入按顺序排队，但不会在当前模型请求或工具批次中间强行注入。不要声称当前回合已被实时打断；排队输入会在本回合终止后逐条开始。明确的停止或改变目标指令一旦轮到，优先处理它，并先中断相关后台 Agent。

委派后不要重复执行同一任务，也不要调用 wait/status 轮询后台。`agent` 的启动回执返回后，给用户一句简短、确定的“已转后台”说明并结束本回合，立即释放前台交互。Orchestrator 完成后的证据只能在独立的后台回流回合中整理，不得混入用户正在进行的回合。核对其中的状态、证据、变更、风险和阻塞；你可以接受、修改或拒绝其建议。只有你能决定是否需要用户确认、是否继续执行，以及最终向用户呈现什么。后台失败或不可用时，能安全直做就继续，否则给出简短、可行动的说明；绝不能无结果地结束。

始终以一个连续、自然的 Pinvou 身份表达，不要说“另一个 Agent 告诉我”。不展示隐藏推理，只给结论、必要依据和下一步。用户消息来自语音转写，可能有同音、漏字或断句错误；结合上下文理解，关键歧义先确认。"#;

const MAX_UTTERANCE_CHARS: usize = 16_384;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontIntentKind {
    Ask,
    Act,
    Interrupt,
    Acknowledge,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontResponseMode {
    VoicePreferred,
    VisualPreferred,
    Multimodal,
    Silent,
}

/// Front Agent 一次原子调用的输入。它只接收当前交互，不持有对话容器；
/// 连续性由稳定 Pinvou Identity 与 Memory Agent 共同提供。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserInteractionInput {
    pub interaction_id: String,
    pub received_at_ms: i64,
    pub modality: InteractionModality,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default)]
    pub evidence_event_ids: Vec<String>,
}

/// Front Agent 接受一次交互后写入 runtime 的意图信封。
/// `objective` 是规范化后的原始意图，不在这里臆测用户没有表达的目标；
/// 该信封也不会把 Front 的最终决策权转交给 Orchestrator。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FrontIntentEnvelope {
    pub interaction_id: String,
    pub accepted_at_ms: i64,
    pub objective: String,
    pub kind: FrontIntentKind,
    pub priority: u8,
    pub response_mode: FrontResponseMode,
    pub requires_response: bool,
    pub evidence_event_ids: Vec<String>,
    pub reason_codes: Vec<String>,
}

/// 将一次用户输入规范化为可编排意图。函数无外部副作用、无隐藏状态，适合在
/// 语音、文字和触控入口之间复用，也方便运行时重放与审计。
pub fn accept_user_interaction(
    input: &UserInteractionInput,
    accepted_at_ms: i64,
) -> Result<FrontIntentEnvelope> {
    validate_identifier(&input.interaction_id, "interaction id")?;
    if input.received_at_ms <= 0 || accepted_at_ms <= 0 {
        bail!("interaction timestamps must be positive");
    }
    if input.received_at_ms > accepted_at_ms.saturating_add(5_000) {
        bail!("interaction timestamp is too far in the future");
    }

    let objective = normalize_content(&input.content);
    if objective.is_empty() {
        bail!("interaction content must not be empty");
    }
    if objective.chars().count() > MAX_UTTERANCE_CHARS {
        bail!("interaction content is too long");
    }
    if let Some(locale) = &input.locale {
        validate_locale(locale)?;
    }

    let lowercase = objective.to_lowercase();
    let kind = classify_intent(&lowercase, &objective);
    let (priority, requires_response, mut reason_codes) = match kind {
        FrontIntentKind::Interrupt => (100, true, vec!["explicit_interrupt".to_string()]),
        FrontIntentKind::Ask => (70, true, vec!["answer_expected".to_string()]),
        FrontIntentKind::Act => (60, true, vec!["action_requested".to_string()]),
        FrontIntentKind::Acknowledge => (40, false, vec!["acknowledgement_only".to_string()]),
    };
    reason_codes.push(
        match input.modality {
            InteractionModality::Voice => "voice_input",
            InteractionModality::Text => "text_input",
            InteractionModality::Touch => "touch_input",
            InteractionModality::System => "system_input",
        }
        .to_string(),
    );

    Ok(FrontIntentEnvelope {
        interaction_id: input.interaction_id.trim().to_string(),
        accepted_at_ms,
        objective,
        kind,
        priority,
        response_mode: response_mode(input.modality, kind),
        requires_response,
        evidence_event_ids: normalized_identifiers(&input.evidence_event_ids)?,
        reason_codes,
    })
}

fn classify_intent(lowercase: &str, original: &str) -> FrontIntentKind {
    const ACKNOWLEDGEMENTS: &[&str] =
        &["好的", "好", "知道了", "嗯", "ok", "okay", "thanks", "谢谢"];

    if is_explicit_interrupt(lowercase) {
        FrontIntentKind::Interrupt
    } else if ACKNOWLEDGEMENTS.iter().any(|word| {
        lowercase.trim_matches(|character: char| character.is_ascii_punctuation()) == *word
    }) {
        FrontIntentKind::Acknowledge
    } else if original.ends_with('?')
        || original.ends_with('？')
        || lowercase.starts_with("what ")
        || lowercase.starts_with("why ")
        || lowercase.starts_with("how ")
        || lowercase.starts_with("can ")
        || original.starts_with("什么")
        || original.starts_with("为什么")
        || original.starts_with("怎么")
        || original.starts_with("能不能")
    {
        FrontIntentKind::Ask
    } else {
        FrontIntentKind::Act
    }
}

fn is_explicit_interrupt(value: &str) -> bool {
    let normalized = value.trim().trim_matches(|character: char| {
        character.is_ascii_punctuation() || matches!(character, '。' | '！' | '？')
    });
    if matches!(
        normalized,
        "stop" | "cancel" | "abort" | "取消" | "停下" | "停止" | "别做了"
    ) {
        return true;
    }
    let normalized = normalized.strip_prefix("请").unwrap_or(normalized);
    let normalized = normalized.strip_prefix("先").unwrap_or(normalized);
    normalized.starts_with("停止后台任务")
        || normalized.starts_with("停止当前任务")
        || normalized.starts_with("停下当前任务")
        || normalized.starts_with("取消当前任务")
        || normalized.starts_with("取消这个任务")
        || normalized.starts_with("取消刚才的任务")
        || normalized.starts_with("stop the current task")
        || normalized.starts_with("cancel the current task")
}

fn response_mode(modality: InteractionModality, kind: FrontIntentKind) -> FrontResponseMode {
    if kind == FrontIntentKind::Acknowledge {
        return FrontResponseMode::Silent;
    }
    match modality {
        InteractionModality::Voice => FrontResponseMode::VoicePreferred,
        InteractionModality::Text => FrontResponseMode::VisualPreferred,
        InteractionModality::Touch => FrontResponseMode::Multimodal,
        InteractionModality::System => FrontResponseMode::Silent,
    }
}

fn normalize_content(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 128 {
        bail!("{label} must contain 1 to 128 characters");
    }
    if !value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | ':' | '_' | '-')
    }) {
        bail!("{label} contains unsupported characters");
    }
    Ok(())
}

fn validate_locale(value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 32 {
        bail!("locale must contain 1 to 32 characters");
    }
    if !value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!("locale contains unsupported characters");
    }
    Ok(())
}

fn normalized_identifiers(values: &[String]) -> Result<Vec<String>> {
    let mut normalized = values
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    for value in &normalized {
        validate_identifier(value, "evidence event id")?;
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_prompt_keeps_one_user_facing_authority_and_one_orchestrator() {
        assert!(FRONT_AGENT_INSTRUCTION.contains("唯一可以给出最终答复"));
        assert!(FRONT_AGENT_INSTRUCTION.contains("Direct"));
        assert!(FRONT_AGENT_INSTRUCTION.contains("Clarify"));
        assert!(FRONT_AGENT_INSTRUCTION.contains("Orchestrate"));
        assert!(FRONT_AGENT_INSTRUCTION.contains("Interrupt"));
        assert!(FRONT_AGENT_INSTRUCTION.contains("只启动一个"));
        assert!(FRONT_AGENT_INSTRUCTION.contains("pinvou-orchestrator"));
        assert!(FRONT_AGENT_INSTRUCTION.contains("Direct 最多使用三轮工具"));
        assert!(FRONT_AGENT_INSTRUCTION.contains("安全上限"));
        assert!(FRONT_AGENT_INSTRUCTION.contains("同一回复里并行调用多个工具仍只算一轮"));
        assert!(FRONT_AGENT_INSTRUCTION.contains("排队输入会在本回合终止后逐条开始"));
        assert!(FRONT_AGENT_INSTRUCTION.contains("立即释放前台交互"));
        assert!(FRONT_AGENT_INSTRUCTION.contains("不得混入用户正在进行的回合"));
        assert!(FRONT_AGENT_INSTRUCTION.contains(FRONT_VOICE_TRANSCRIPT_INSTRUCTION));
    }

    fn input(content: &str, modality: InteractionModality) -> UserInteractionInput {
        UserInteractionInput {
            interaction_id: "interaction:1".to_string(),
            received_at_ms: 100_000,
            modality,
            content: content.to_string(),
            locale: Some("zh-CN".to_string()),
            evidence_event_ids: vec!["event:voice-1".to_string(), "event:voice-1".to_string()],
        }
    }

    #[test]
    fn voice_question_becomes_one_normalized_front_intent() {
        let accepted = accept_user_interaction(
            &input("  MegaBook   现在热不热？ ", InteractionModality::Voice),
            100_100,
        )
        .unwrap();

        assert_eq!(accepted.objective, "MegaBook 现在热不热？");
        assert_eq!(accepted.kind, FrontIntentKind::Ask);
        assert_eq!(accepted.response_mode, FrontResponseMode::VoicePreferred);
        assert_eq!(accepted.evidence_event_ids, vec!["event:voice-1"]);
    }

    #[test]
    fn explicit_stop_is_always_high_priority() {
        let accepted =
            accept_user_interaction(&input("先停止后台任务", InteractionModality::Text), 100_100)
                .unwrap();
        assert_eq!(accepted.kind, FrontIntentKind::Interrupt);
        assert_eq!(accepted.priority, 100);
        assert!(accepted.requires_response);
    }

    #[test]
    fn cancellation_topic_is_not_mistaken_for_runtime_interrupt() {
        let accepted = accept_user_interaction(
            &input("取消订阅应该怎么操作？", InteractionModality::Text),
            100_100,
        )
        .unwrap();
        assert_eq!(accepted.kind, FrontIntentKind::Ask);
    }

    #[test]
    fn empty_or_untraceable_interactions_are_rejected() {
        assert!(
            accept_user_interaction(&input("  \n ", InteractionModality::Text), 100_100).is_err()
        );
        let mut invalid = input("hello", InteractionModality::Text);
        invalid.interaction_id = "bad id".to_string();
        assert!(accept_user_interaction(&invalid, 100_100).is_err());
    }
}
