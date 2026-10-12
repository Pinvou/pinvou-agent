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
//! The pure helpers below originate from same-named functions in
//! `l1_dialog_harness.rs` (that copy is also used by the real-model scenario
//! and cannot be moved out wholesale); this module's `TurnSummary` /
//! `summarize` keep only the fields these three tests read, so the two
//! same-named structures are no longer field-for-field identical — evaluate
//! both sides when changing either.

use deepseek_tui::core::events::{Event, TurnOutcomeStatus};
use deepseek_tui::error_taxonomy::ErrorEnvelope;

/// Aggregate a finished scenario run (this copy keeps only the fields the
/// three tests in this module read).
struct TurnSummary {
    /// Engine 在 turn 内发出的错误；严格真模型验收中任意一条都必须失败。
    engine_errors: Vec<String>,
    /// TurnComplete 报告的权威终态；None 表示未收到完整终态。
    terminal_status: Option<TurnOutcomeStatus>,
    /// TurnComplete 携带的终态错误。
    terminal_error: Option<String>,
}

fn summarize(timeline: &[(f64, Event)]) -> TurnSummary {
    let mut engine_errors = Vec::new();
    let mut terminal_status = None;
    let mut terminal_error = None;
    for (_t, e) in timeline {
        match e {
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
        engine_errors,
        terminal_status,
        terminal_error,
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

#[test]
fn strict_mode_rejects_runtime_engine_error() {
    let timeline = vec![(
        0.1,
        Event::error(ErrorEnvelope::classify(
            "stream read error: response body decode failed".to_string(),
            true,
        )),
    )];
    let summary = summarize(&timeline);

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
    let summary = summarize(&timeline);

    assert!(summary.engine_errors.is_empty());
    assert_eq!(summary.terminal_status, Some(TurnOutcomeStatus::Failed));
    assert_eq!(
        summary.terminal_error.as_deref(),
        Some("engine task panicked")
    );
    assert!(validate_terminal_outcome(&summary, true).is_err());
    assert!(validate_terminal_outcome(&summary, false).is_ok());
}
