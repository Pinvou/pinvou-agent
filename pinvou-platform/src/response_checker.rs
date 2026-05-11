//! **LEGACY**：[OK] / [MORE] / [BLOCKED] 信号路由。
//!
//! 新设计用 `contract_runtime` + `contract_validator` 取代「LLM 自评推进」语义。
//! 仍保留是因为 web 主路径在 LLM 响应后调用 ResponseChecker 做软推进判断；
//! 待 ContractValidator 完全替代后删除（P1）。
//!
//! ResponseChecker — 信号解析 + 越界检测 + 路由决策。
//!
//! 纯函数模块，不做 LLM 调用，只做规则-based 文本解析与决策。

use regex::Regex;

use crate::app::{AppConfig, Milestone};

/// LLM 响应中的完成信号
#[derive(Debug, Clone, PartialEq)]
pub enum CompletionSignal {
    /// 明确完成标记
    Done,
    /// 需要更多轮次
    More { reason: String },
    /// 生成被阻塞
    Blocked { reason: String },
}

/// 下一步动作
#[derive(Debug, Clone, PartialEq)]
pub enum NextAction {
    /// 自动推进到下一里程碑
    Advance,
    /// 等待用户确认
    WaitForUser,
    /// 继续当前里程碑（需要更多内容）
    Continue { reason: String },
    /// 阻塞，需要用户介入
    Block { reason: String },
}

/// 检查结果
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// 响应是否越界（超出当前阶段预期）
    pub out_of_scope: bool,
    /// 安全截取的内容（越界时使用）
    pub safe_content: Option<String>,
    /// 检测到的完成信号
    pub signal: Option<CompletionSignal>,
    /// 路由决策
    pub next_action: NextAction,
}

/// 响应检查器（无状态）
pub struct ResponseChecker;

impl ResponseChecker {
    /// 综合检查入口：解析信号 → 越界检测 → 路由决策
    pub fn check(
        response: &str,
        current_milestone: &Milestone,
        app_config: &AppConfig,
    ) -> CheckResult {
        let signal = Self::parse_signal(response);
        let (out_of_scope, safe_content) = Self::check_out_of_scope(response, current_milestone);
        let next_action = Self::decide(&signal, current_milestone, app_config, out_of_scope);

        CheckResult {
            out_of_scope,
            safe_content,
            signal,
            next_action,
        }
    }

    /// 解析 LLM 响应末尾 500 字符中的完成信号
    ///
    /// 优先级: BLOCKED > MORE > OK
    fn parse_signal(response: &str) -> Option<CompletionSignal> {
        let tail = tail_chars(response, 500);

        // --- BLOCKED (highest priority) ---
        let blocked_patterns = [
            r"(?i)\[BLOCKED\]",
            r"\[阻塞\]",
            r"卡住了[：:]",
            r"无法继续[：:]",
        ];
        for pat in &blocked_patterns {
            let re = Regex::new(&format!(r"{}(?:[:\s]*(?P<reason>.{{1,200}}))?", pat)).unwrap();
            if let Some(caps) = re.captures(&tail) {
                let reason = caps
                    .name("reason")
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_default();
                return Some(CompletionSignal::Blocked { reason });
            }
        }

        // --- MORE ---
        let more_patterns = [r"(?i)\[MORE\]", r"\[继续\]", r"还需要[：:]", r"还没完[：:]"];
        for pat in &more_patterns {
            let re = Regex::new(&format!(r"{}(?:[:\s]*(?P<reason>.{{1,200}}))?", pat)).unwrap();
            if let Some(caps) = re.captures(&tail) {
                let reason = caps
                    .name("reason")
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_default();
                return Some(CompletionSignal::More { reason });
            }
        }

        // --- OK (lowest priority) ---
        // Check explicit markers first
        let ok_markers = [r"(?i)\[OK\]", r"\[完成\]", r"\[✓\]"];
        for pat in &ok_markers {
            if Regex::new(pat).unwrap().is_match(&tail) {
                return Some(CompletionSignal::Done);
            }
        }
        // Check "完成。" ending
        if tail.ends_with("完成。") {
            return Some(CompletionSignal::Done);
        }

        None
    }

    /// 检测响应是否「越界」—— 内容超出当前阶段预期范围
    ///
    /// 两个机械规则（按顺序检查）：
    /// 1. 询问阶段（需求/确认/收集）却生成了大段内容 (>300 字符, 无问号)
    /// 2. 非生成阶段却产出了完整文档（3+ 个空行分隔的段落）
    fn check_out_of_scope(response: &str, milestone: &Milestone) -> (bool, Option<String>) {
        let label = &milestone.label;
        let hint = milestone.prompt_hint.as_deref().unwrap_or("");

        // --- Rule 1: Asking phase but LLM generated content ---
        let is_asking_phase = is_asking_label(label) && is_asking_hint(hint);

        if is_asking_phase {
            let has_question = response.contains('?') || response.contains('？');
            if response.chars().count() > 300 && !has_question {
                let safe: String = response.chars().take(150).collect();
                return (true, Some(safe));
            }
        }

        // --- Rule 2: Generated complete doc but not in generation phase ---
        let is_gen_phase = is_gen_label(label) || is_gen_hint(hint);

        if !is_gen_phase {
            let paragraphs: Vec<&str> = response.split("\n\n").collect();
            if paragraphs.len() >= 3 {
                let safe = paragraphs[..2].join("\n\n");
                return (true, Some(safe));
            }
        }

        (false, None)
    }

    /// 路由决策矩阵
    ///
    /// | signal       | condition                    | next_action  |
    /// |-------------|------------------------------|--------------|
    /// | out_of_scope | (any)                       | Continue     |
    /// | [BLOCKED]    | (any)                       | Block        |
    /// | [MORE]       | (any)                       | Continue     |
    /// | [OK]         | model_preference == "small" | WaitForUser  |
    /// | [OK]         | otherwise                   | Advance      |
    /// | None         | model_preference == "small" | WaitForUser  |
    /// | None         | otherwise                   | Advance      |
    fn decide(
        signal: &Option<CompletionSignal>,
        _milestone: &Milestone,
        app_config: &AppConfig,
        out_of_scope: bool,
    ) -> NextAction {
        // out_of_scope always overrides
        if out_of_scope {
            return NextAction::Continue {
                reason: "out of scope, using safe content".to_string(),
            };
        }

        match signal {
            Some(CompletionSignal::Blocked { reason }) => NextAction::Block {
                reason: if reason.is_empty() {
                    "response blocked".to_string()
                } else {
                    reason.clone()
                },
            },
            Some(CompletionSignal::More { reason }) => NextAction::Continue {
                reason: if reason.is_empty() {
                    "more content expected".to_string()
                } else {
                    reason.clone()
                },
            },
            // OK or no signal: routing depends on model_preference
            Some(CompletionSignal::Done) | None => {
                // "small" model → more cautious, wait for user
                if app_config.model_preference == "small" {
                    NextAction::WaitForUser
                } else {
                    NextAction::Advance
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// 取字符串末尾 `n` 个字符
fn tail_chars(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        s.to_string()
    } else {
        s.chars().skip(count - n).collect()
    }
}

/// 标签包含「询问/需求收集」关键词
fn is_asking_label(label: &str) -> bool {
    label.contains("需求") || label.contains("确认") || label.contains("收集")
}

/// hint 包含「询问」关键词
fn is_asking_hint(hint: &str) -> bool {
    hint.contains("问") || hint.contains("确认")
}

/// 标签包含「生成/撰写」关键词
fn is_gen_label(label: &str) -> bool {
    label.contains("生成") || label.contains("草稿") || label.contains("撰写")
}

/// hint 包含「生成/撰写」关键词
fn is_gen_hint(hint: &str) -> bool {
    hint.contains("生成") || hint.contains("草稿") || hint.contains("撰写")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- helpers to build test fixtures ---

    fn milestone_asking() -> Milestone {
        Milestone {
            id: "ask".into(),
            label: "需求收集".into(),
            prompt_hint: Some("询问用户需求".into()),
            icon: None,
            ..Default::default()
        }
    }

    fn milestone_writing() -> Milestone {
        Milestone {
            id: "gen".into(),
            label: "文档生成".into(),
            prompt_hint: Some("草稿撰写".into()),
            icon: None,
            ..Default::default()
        }
    }

    fn milestone_neutral() -> Milestone {
        Milestone {
            id: "review".into(),
            label: "审核".into(),
            prompt_hint: None,
            icon: None,
            ..Default::default()
        }
    }

    fn app_small() -> AppConfig {
        AppConfig {
            id: "test".into(),
            name: "Test".into(),
            description: "".into(),
            icon: "".into(),
            prompt_file: None,
            prompt: None,
            model_preference: "small".into(),
            tools: vec![],
            milestones: vec![],
            granularity: None,
            confirm_at: None,
            ban_list: vec![],
            meta: Default::default(),
            ..Default::default()
        }
    }

    fn app_medium() -> AppConfig {
        AppConfig {
            model_preference: "medium".into(),
            ..app_small()
        }
    }

    fn app_large() -> AppConfig {
        AppConfig {
            model_preference: "large".into(),
            ..app_small()
        }
    }

    // ===================================================================
    // Signal parsing tests
    // ===================================================================

    #[test]
    fn signal_ok_bracket() {
        let resp = "报告已生成完毕 [OK]";
        assert_eq!(
            ResponseChecker::parse_signal(resp),
            Some(CompletionSignal::Done)
        );
    }

    #[test]
    fn signal_wan_cheng_bracket() {
        let resp = "所有步骤均已完成 [完成]";
        assert_eq!(
            ResponseChecker::parse_signal(resp),
            Some(CompletionSignal::Done)
        );
    }

    #[test]
    fn signal_check_bracket() {
        let resp = "没问题了 [✓]";
        assert_eq!(
            ResponseChecker::parse_signal(resp),
            Some(CompletionSignal::Done)
        );
    }

    #[test]
    fn signal_done_period() {
        let resp = "所有内容已生成完毕。任务完成。";
        assert_eq!(
            ResponseChecker::parse_signal(resp),
            Some(CompletionSignal::Done)
        );
    }

    #[test]
    fn signal_more_bracket() {
        let resp = "第一部分已经写好了 [MORE] 还有第二部分需要继续";
        let signal = ResponseChecker::parse_signal(resp);
        match signal {
            Some(CompletionSignal::More { reason }) => {
                assert!(reason.contains("还有第二部分需要继续") || !reason.is_empty());
            }
            _ => panic!("expected More signal, got {:?}", signal),
        }
    }

    #[test]
    fn signal_more_cn_bracket() {
        let resp = "还没写完 [继续] 请稍候";
        let signal = ResponseChecker::parse_signal(resp);
        match signal {
            Some(CompletionSignal::More { .. }) => {}
            _ => panic!("expected More signal, got {:?}", signal),
        }
    }

    #[test]
    fn signal_more_hai_xu_yao() {
        let resp = "还需要：用户提供更多信息";
        let signal = ResponseChecker::parse_signal(resp);
        match signal {
            Some(CompletionSignal::More { reason }) => {
                assert!(reason.contains("用户提供更多信息"));
            }
            _ => panic!("expected More signal, got {:?}", signal),
        }
    }

    #[test]
    fn signal_more_hai_mei_wan() {
        let resp = "还没完：还有最后一段总结";
        let signal = ResponseChecker::parse_signal(resp);
        match signal {
            Some(CompletionSignal::More { reason }) => {
                assert!(reason.contains("还有最后一段总结"));
            }
            _ => panic!("expected More signal, got {:?}", signal),
        }
    }

    #[test]
    fn signal_blocked_bracket() {
        let resp = "无法继续处理 [BLOCKED] 缺少必要参数";
        let signal = ResponseChecker::parse_signal(resp);
        match signal {
            Some(CompletionSignal::Blocked { reason }) => {
                assert!(reason.contains("缺少必要参数"));
            }
            _ => panic!("expected Blocked signal, got {:?}", signal),
        }
    }

    #[test]
    fn signal_blocked_cn_bracket() {
        let resp = "这个问题我无法回答 [阻塞] 需要更多上下文";
        let signal = ResponseChecker::parse_signal(resp);
        match signal {
            Some(CompletionSignal::Blocked { reason }) => {
                assert!(reason.contains("需要更多上下文"));
            }
            _ => panic!("expected Blocked signal, got {:?}", signal),
        }
    }

    #[test]
    fn signal_blocked_ka_zhu() {
        let resp = "卡住了：模型不知道下一步做什么";
        let signal = ResponseChecker::parse_signal(resp);
        match signal {
            Some(CompletionSignal::Blocked { reason }) => {
                assert!(reason.contains("模型不知道下一步做什么"));
            }
            _ => panic!("expected Blocked signal, got {:?}", signal),
        }
    }

    #[test]
    fn signal_blocked_wu_fa_ji_xu() {
        let resp = "无法继续：超出 token 限制";
        let signal = ResponseChecker::parse_signal(resp);
        match signal {
            Some(CompletionSignal::Blocked { .. }) => {}
            _ => panic!("expected Blocked signal, got {:?}", signal),
        }
    }

    #[test]
    fn signal_no_signal() {
        let resp = "这是普通的一句回复，没有任何标记。";
        assert_eq!(ResponseChecker::parse_signal(resp), None);
    }

    #[test]
    fn signal_blocked_priority_over_ok() {
        // [BLOCKED] should win over [OK] when both present
        let resp = "Something went wrong [BLOCKED] but also [OK]";
        let signal = ResponseChecker::parse_signal(resp);
        match signal {
            Some(CompletionSignal::Blocked { .. }) => {}
            _ => panic!(
                "expected Blocked to take priority over OK, got {:?}",
                signal
            ),
        }
    }

    #[test]
    fn signal_blocked_priority_over_more() {
        let resp = "[BLOCKED] stuck [MORE]";
        let signal = ResponseChecker::parse_signal(resp);
        match signal {
            Some(CompletionSignal::Blocked { .. }) => {}
            _ => panic!(
                "expected Blocked to take priority over MORE, got {:?}",
                signal
            ),
        }
    }

    #[test]
    fn signal_more_priority_over_ok() {
        let resp = "[MORE] need more info [OK]";
        let signal = ResponseChecker::parse_signal(resp);
        match signal {
            Some(CompletionSignal::More { .. }) => {}
            _ => panic!("expected MORE to take priority over OK, got {:?}", signal),
        }
    }

    #[test]
    fn signal_only_in_tail_500_chars() {
        // Signal is deep in the response, but should still be found in last 500 chars
        let prefix = "a".repeat(1000);
        let resp = format!("{} [OK]", prefix);
        assert_eq!(
            ResponseChecker::parse_signal(&resp),
            Some(CompletionSignal::Done)
        );
    }

    // ===================================================================
    // Out-of-scope tests
    // ===================================================================

    #[test]
    fn out_of_scope_asking_generates_content() {
        // Asking phase (需求收集 + 询问) but response >300 chars and no question mark
        let response = "根据您的需求，我建议采用以下方案：\n".repeat(20); // >300 chars, no question
        let (out, safe) = ResponseChecker::check_out_of_scope(&response, &milestone_asking());
        assert!(
            out,
            "asking phase with >300 chars and no ? should be out of scope"
        );
        assert!(safe.is_some());
        // safe content should be truncated to 150 chars
        assert!(safe.unwrap().chars().count() <= 150);
    }

    #[test]
    fn out_of_scope_asking_with_question_is_safe() {
        // Asking phase with a question mark → still a proper question
        let mut response = "根据您的需求，我建议采用以下方案：".repeat(20);
        response.push('？'); // has question mark
        let (out, safe) = ResponseChecker::check_out_of_scope(&response, &milestone_asking());
        assert!(!out, "asking with question mark should not be out of scope");
        assert!(safe.is_none());
    }

    #[test]
    fn out_of_scope_asking_with_eng_question_is_safe() {
        let mut response = "Can you tell me more about your requirements?".to_string();
        // pad to >300 chars
        while response.chars().count() <= 300 {
            response.push_str(" More padding text here.");
        }
        response.push('?');
        let (out, _safe) = ResponseChecker::check_out_of_scope(&response, &milestone_asking());
        assert!(!out, "asking with ? should not be out of scope");
    }

    #[test]
    fn out_of_scope_asking_short_is_safe() {
        // Short response (<300 chars) even without question mark is fine
        let response = "好的，我来确认一下需求。";
        let (out, _safe) = ResponseChecker::check_out_of_scope(&response, &milestone_asking());
        assert!(!out, "short asking response should be safe");
    }

    #[test]
    fn out_of_scope_gen_phase_is_safe() {
        // In generation phase (文档生成 + 草稿撰写), large output is expected
        let response = "第一章\n\n第二章\n\n第三章\n\n第四章";
        let (out, _safe) = ResponseChecker::check_out_of_scope(&response, &milestone_writing());
        assert!(
            !out,
            "generation phase should be safe even with many paragraphs"
        );
    }

    #[test]
    fn out_of_scope_neutral_multi_para() {
        // Neutral phase (审核) with 3+ paragraphs → out of scope
        let response = "段落一\n\n段落二\n\n段落三";
        let (out, safe) = ResponseChecker::check_out_of_scope(&response, &milestone_neutral());
        assert!(
            out,
            "neutral phase with 3+ paragraphs should be out of scope"
        );
        assert!(safe.is_some());
        let safe = safe.unwrap();
        // Should keep first 2 paragraphs
        assert!(safe.contains("段落一"));
        assert!(safe.contains("段落二"));
        assert!(!safe.contains("段落三"));
    }

    #[test]
    fn out_of_scope_neutral_two_para_is_safe() {
        // Neutral phase with only 2 paragraphs → safe
        let response = "段落一\n\n段落二";
        let (out, _safe) = ResponseChecker::check_out_of_scope(&response, &milestone_neutral());
        assert!(!out, "2 paragraphs should be safe");
    }

    #[test]
    fn out_of_scope_gen_label_only() {
        // Label says 生成 but hint is empty → still gen phase
        let ms = Milestone {
            id: "gen2".into(),
            label: "内容生成".into(),
            prompt_hint: None,
            icon: None,
            ..Default::default()
        };
        let response = "A\n\nB\n\nC";
        let (out, _safe) = ResponseChecker::check_out_of_scope(&response, &ms);
        assert!(!out, "label containing 生成 should mark gen phase");
    }

    // ===================================================================
    // Routing decision tests
    // ===================================================================

    #[test]
    fn routing_ok_small_waits() {
        let signal = Some(CompletionSignal::Done);
        let action = ResponseChecker::decide(&signal, &milestone_neutral(), &app_small(), false);
        assert_eq!(action, NextAction::WaitForUser);
    }

    #[test]
    fn routing_ok_medium_advances() {
        let signal = Some(CompletionSignal::Done);
        let action = ResponseChecker::decide(&signal, &milestone_neutral(), &app_medium(), false);
        assert_eq!(action, NextAction::Advance);
    }

    #[test]
    fn routing_ok_large_advances() {
        let signal = Some(CompletionSignal::Done);
        let action = ResponseChecker::decide(&signal, &milestone_neutral(), &app_large(), false);
        assert_eq!(action, NextAction::Advance);
    }

    #[test]
    fn routing_none_small_waits() {
        let action = ResponseChecker::decide(&None, &milestone_neutral(), &app_small(), false);
        assert_eq!(action, NextAction::WaitForUser);
    }

    #[test]
    fn routing_none_medium_advances() {
        let action = ResponseChecker::decide(&None, &milestone_neutral(), &app_medium(), false);
        assert_eq!(action, NextAction::Advance);
    }

    #[test]
    fn routing_blocked_blocks() {
        let signal = Some(CompletionSignal::Blocked {
            reason: "token limit exceeded".into(),
        });
        let action = ResponseChecker::decide(&signal, &milestone_neutral(), &app_medium(), false);
        assert_eq!(
            action,
            NextAction::Block {
                reason: "token limit exceeded".into()
            }
        );
    }

    #[test]
    fn routing_blocked_default_reason() {
        let signal = Some(CompletionSignal::Blocked {
            reason: String::new(),
        });
        let action = ResponseChecker::decide(&signal, &milestone_neutral(), &app_medium(), false);
        match action {
            NextAction::Block { reason } => {
                assert!(!reason.is_empty(), "should have default reason");
            }
            _ => panic!("expected Block action"),
        }
    }

    #[test]
    fn routing_more_continues() {
        let signal = Some(CompletionSignal::More {
            reason: "需要用户输入".into(),
        });
        let action = ResponseChecker::decide(&signal, &milestone_neutral(), &app_medium(), false);
        assert_eq!(
            action,
            NextAction::Continue {
                reason: "需要用户输入".into()
            }
        );
    }

    #[test]
    fn routing_more_default_reason() {
        let signal = Some(CompletionSignal::More {
            reason: String::new(),
        });
        let action = ResponseChecker::decide(&signal, &milestone_neutral(), &app_medium(), false);
        match action {
            NextAction::Continue { reason } => {
                assert!(!reason.is_empty(), "should have default reason");
            }
            _ => panic!("expected Continue action"),
        }
    }

    #[test]
    fn routing_out_of_scope_overrides() {
        // Even with [OK] and medium (which normally → Advance), out_of_scope → Continue
        let signal = Some(CompletionSignal::Done);
        let action = ResponseChecker::decide(&signal, &milestone_neutral(), &app_medium(), true);
        match action {
            NextAction::Continue { reason } => {
                assert!(reason.contains("out of scope"));
            }
            _ => panic!("out_of_scope should override to Continue, got {:?}", action),
        }
    }

    #[test]
    fn routing_out_of_scope_overrides_blocked() {
        let signal = Some(CompletionSignal::Blocked {
            reason: "blocked".into(),
        });
        let action = ResponseChecker::decide(&signal, &milestone_neutral(), &app_medium(), true);
        match action {
            NextAction::Continue { .. } => {}
            _ => panic!("out_of_scope should override Block"),
        }
    }

    // ===================================================================
    // Integration-style tests (check() method)
    // ===================================================================

    #[test]
    fn check_done_medium_advances() {
        let result = ResponseChecker::check("[OK] 任务完成。", &milestone_neutral(), &app_medium());
        assert!(!result.out_of_scope);
        assert_eq!(result.signal, Some(CompletionSignal::Done));
        assert_eq!(result.next_action, NextAction::Advance);
    }

    #[test]
    fn check_done_small_waits() {
        let result = ResponseChecker::check("完成。", &milestone_neutral(), &app_small());
        assert!(!result.out_of_scope);
        assert_eq!(result.signal, Some(CompletionSignal::Done));
        assert_eq!(result.next_action, NextAction::WaitForUser);
    }

    #[test]
    fn check_blocked_with_small() {
        let result = ResponseChecker::check(
            "无法继续：超出上下文窗口",
            &milestone_neutral(),
            &app_small(),
        );
        match result.next_action {
            NextAction::Block { .. } => {}
            _ => panic!("expected Block action"),
        }
    }

    #[test]
    fn check_asking_phase_out_of_scope() {
        let long_response = "这是很长的回复".repeat(50); // 7*50=350 chars, >300, no question
        let result = ResponseChecker::check(&long_response, &milestone_asking(), &app_medium());
        assert!(result.out_of_scope);
        assert!(result.safe_content.is_some());
        match result.next_action {
            NextAction::Continue { .. } => {}
            _ => panic!("expected Continue for out of scope"),
        }
    }

    #[test]
    fn check_more_with_reason() {
        let result = ResponseChecker::check(
            "还需要：用户的详细需求描述",
            &milestone_neutral(),
            &app_medium(),
        );
        match result.signal {
            Some(CompletionSignal::More { ref reason }) => {
                assert!(reason.contains("用户的详细需求描述"));
            }
            _ => panic!("expected More signal"),
        }
        match result.next_action {
            NextAction::Continue { .. } => {}
            _ => panic!("expected Continue action for MORE"),
        }
    }
}
