//! `strict_mode` 严格错误校验回归。
//!
//! 这三个测试原属 `l1_dialog_harness.rs`，是确定性纯函数校验：只依赖
//! `deepseek_tui` 的 `Event` / `ErrorEnvelope` / `TurnOutcomeStatus` / `Usage`，
//! 不触碰 `AppEngine` / `Pinvou3Bridge`。作为 `pinvou3_lib` 的单测模块随现有
//! `cargo test --lib` 一起链接和执行，避免再启动一个 integration test 目标，
//! 重复编译并链接 fastembed / tauri / aws-lc-sys 全树。
//!
//! 真 vLLM 端到端 scenario 仍留在 `l1_dialog_harness.rs`（`#[ignore]`，需 vLLM
//! 在线、按需手跑）；本模块只跑不依赖运行时与外部模型的确定性校验，CI 可常跑。
//!
//! 下面的纯辅助函数与 `l1_dialog_harness.rs` 中同名函数行为一致（那份同时被真模型
//! scenario 使用，无法整体迁出）；此处保留副本，改动需两边同步。

#![allow(dead_code)] // TurnSummary 部分字段由 summarize 填充、本目标测试不全部读取

use std::collections::HashMap;
use std::time::Duration;

use deepseek_tui::core::events::{Event, TurnOutcomeStatus};
use deepseek_tui::error_taxonomy::ErrorEnvelope;

/// scenario 跑完后聚合结果（与 `l1_dialog_harness.rs` 同名结构保持一致）。
struct TurnSummary {
    /// 所有 MessageDelta.content 串起来 (LLM 的纯文本输出)
    full_text: String,
    /// 工具名 → 成功完成的调用次数
    tool_call_counts: HashMap<String, usize>,
    /// Engine 在 turn 内发出的错误；严格真模型验收中任意一条都必须失败。
    engine_errors: Vec<String>,
    /// TurnComplete 报告的权威终态；None 表示未收到完整终态。
    terminal_status: Option<TurnOutcomeStatus>,
    /// TurnComplete 携带的终态错误。
    terminal_error: Option<String>,
    elapsed: Duration,
    timed_out: bool,
}

fn summarize(timeline: &[(f64, Event)], elapsed: Duration, timed_out: bool) -> TurnSummary {
    let mut full_text = String::new();
    let mut tool_call_counts = HashMap::<String, usize>::new();
    let mut engine_errors = Vec::new();
    let mut terminal_status = None;
    let mut terminal_error = None;
    for (_t, e) in timeline {
        match e {
            Event::MessageDelta { content, .. } => full_text.push_str(content),
            Event::ToolCallComplete { name, result, .. } => {
                if result.is_ok() {
                    *tool_call_counts.entry(name.clone()).or_insert(0) += 1;
                }
            }
            Event::Error { envelope, .. } => {
                engine_errors.push(format!("{}: {}", envelope.code, envelope.message));
            }
            Event::TurnComplete { status, error, .. } => {
                terminal_status = Some(*status);
                terminal_error = error.clone();
            }
            _ => {}
        }
    }
    TurnSummary {
        full_text,
        tool_call_counts,
        engine_errors,
        terminal_status,
        terminal_error,
        elapsed,
        timed_out,
    }
}

fn validate_engine_errors(engine_errors: &[String], strict: bool) -> Result<(), String> {
    if strict && !engine_errors.is_empty() {
        return Err(format!(
            "turn 收到 {} 个 Engine Error: {}",
            engine_errors.len(),
            engine_errors.join(" | ")
        ));
    }
    Ok(())
}

fn validate_terminal_outcome(summary: &TurnSummary, strict: bool) -> Result<(), String> {
    if !strict {
        return Ok(());
    }

    match summary.terminal_status {
        Some(TurnOutcomeStatus::Completed) if summary.terminal_error.is_none() => Ok(()),
        Some(status) => Err(format!(
            "turn 终态不是无错误的 Completed: status={status:?}, error={:?}",
            summary.terminal_error
        )),
        None => Err("turn 未收到 TurnComplete 权威终态".to_string()),
    }
}

fn is_authoritative_turn_complete(event: &Event) -> bool {
    matches!(event, Event::TurnComplete { .. })
}

#[test]
fn strict_mode_rejects_runtime_engine_error() {
    let timeline = vec![(
        0.1,
        Event::error(ErrorEnvelope::classify(
            "stream read error: response body decode failed".to_string(),
            true,
        )),
    )];
    let summary = summarize(&timeline, Duration::from_millis(100), false);

    assert_eq!(summary.engine_errors.len(), 1);
    assert!(summary.engine_errors[0].contains("stream read error"));
    assert!(validate_engine_errors(&summary.engine_errors, true).is_err());
    assert!(validate_engine_errors(&summary.engine_errors, false).is_ok());
    assert!(validate_engine_errors(&[], true).is_ok());
}

#[test]
fn strict_mode_rejects_failed_turn_without_error_event() {
    use deepseek_tui::models::Usage;

    let timeline = vec![(
        0.1,
        Event::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Failed,
            error: Some("engine task panicked".to_string()),
            tool_catalog: None,
            base_url: None,
        },
    )];
    let summary = summarize(&timeline, Duration::from_millis(100), false);

    assert!(summary.engine_errors.is_empty());
    assert_eq!(summary.terminal_status, Some(TurnOutcomeStatus::Failed));
    assert_eq!(
        summary.terminal_error.as_deref(),
        Some("engine task panicked")
    );
    assert!(validate_terminal_outcome(&summary, true).is_err());
    assert!(validate_terminal_outcome(&summary, false).is_ok());
}

#[test]
fn strict_mode_waits_for_turn_complete_after_error() {
    use deepseek_tui::models::Usage;

    let error = Event::error(ErrorEnvelope::classify(
        "temporary stream error".to_string(),
        true,
    ));
    let complete = Event::TurnComplete {
        usage: Usage::default(),
        status: TurnOutcomeStatus::Failed,
        error: Some("stream recovery failed".to_string()),
        tool_catalog: None,
        base_url: None,
    };

    assert!(!is_authoritative_turn_complete(&error));
    assert!(is_authoritative_turn_complete(&complete));
}
