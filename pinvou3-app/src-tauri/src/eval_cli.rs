//! 无窗口产品模式评测的 Tauri composition root。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use tauri::Manager;

use crate::features::assistant::engine_pool::{EnginePool, EngineToolFactory, ToolPolicy};
use crate::features::assistant::eval::analysis::judge::analyze_with_product_judge;
use crate::features::assistant::eval::analysis::rules::analyze_rules;
use crate::features::assistant::eval::analysis::{
    merge_findings, score_product_run, summarize_product_problems, JudgeReport,
    JudgeStatus as AnalysisJudgeStatus, ProductScore,
};
use crate::features::assistant::eval::markdown_report::{
    write_markdown_report, EvalMarkdownReport,
};
use crate::features::assistant::eval::report::{EvalReportWriter, EvalRunMetadata};
#[cfg(test)]
use crate::features::assistant::eval::EvalCase;
use crate::features::assistant::eval::{
    cases, run_eval_suite_with_model_factory, EvalMode, EvalSuiteResult,
};
use crate::features::assistant::product_runtime::EnginePoolRuntime;
use crate::features::{knowledge, sessions::SessionStore};

#[derive(Debug, Clone, Default)]
pub struct EvalSmokeOptions {
    pub judge_model_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JudgeStatus {
    Completed,
    NotConfigured,
    SkippedSameModel,
    Failed,
}

#[derive(Debug)]
pub struct EvalSmokeOutcome {
    pub all_succeeded: bool,
    pub jsonl_report_path: PathBuf,
    pub markdown_report_path: PathBuf,
    pub markdown: String,
    pub judge_status: JudgeStatus,
    pub product_score: Option<u8>,
    pub product_score_version: Option<String>,
}

/// 在主线程运行零窗口 Tauri 事件循环，并通过真实 EnginePool 执行 PLEP smoke。
pub fn run_product_eval_smoke(options: EvalSmokeOptions) -> Result<EvalSmokeOutcome> {
    super::install_rustls_provider();
    super::ensure_release_env();

    let completed = Arc::new(AtomicBool::new(false));
    let completed_for_setup = completed.clone();
    let completed_for_loop = completed.clone();
    let (outcome_tx, outcome_rx) = tokio::sync::oneshot::channel();

    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default()
        .setup(move |app| {
            if let Ok(resource_dir) = app.path().resource_dir() {
                crate::platform::paths::set_runtime_resource_dir(resource_dir);
            }

            let store = SessionStore::boot().context("boot eval session store")?;
            store.load_skill_bindings();
            store.load_session_models();
            store.load_pinned_sessions();
            store.load_hidden_sessions();
            app.manage(store.clone());

            let pool = build_eval_pool(app.handle().clone(), store)?;
            app.manage(pool.clone());
            let handle = app.handle().clone();
            let completed = completed_for_setup.clone();
            tauri::async_runtime::spawn(async move {
                let outcome = execute_product_smoke(pool, options).await;
                let exit_code = match &outcome {
                    Ok(value) if value.all_succeeded => 0,
                    _ => 1,
                };
                completed.store(true, Ordering::Release);
                let _ = outcome_tx.send(outcome);
                handle.exit(exit_code);
            });
            Ok(())
        })
        .build(context)
        .context("build windowless eval application")?;

    let exit_code = app.run_return(move |_handle, event| {
        if let tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            if !completed_for_loop.load(Ordering::Acquire) {
                api.prevent_exit();
            }
        }
    });

    let outcome = outcome_rx
        .blocking_recv()
        .context("eval event loop exited before producing an outcome")??;
    debug_assert_eq!(exit_code == 0, outcome.all_succeeded);
    Ok(outcome)
}

fn build_eval_pool(app: tauri::AppHandle, store: SessionStore) -> Result<EnginePool> {
    let tool_factory: EngineToolFactory = Arc::new(|app, session_id| {
        vec![
            Arc::new(knowledge::KbSearchTool::new(
                app.clone(),
                session_id.to_string(),
            )),
            Arc::new(knowledge::KbOpenSourceTool::new(
                app.clone(),
                session_id.to_string(),
            )),
        ]
    });
    let tool_policy: ToolPolicy = Arc::new(|app| {
        let mut tools = crate::features::marketplace::disabled_tool_names();
        let kb_usable = app
            .try_state::<knowledge::KnowledgeService>()
            .map(|service| service.has_indexed_content() && service.semantic_ready())
            .unwrap_or(false);
        if !kb_usable {
            tools.push("kb_search".to_string());
            tools.push("kb_open_source".to_string());
        }
        tools
    });
    EnginePool::new_with_dependencies(app, store, tool_factory, tool_policy)
}

async fn execute_product_smoke(
    pool: EnginePool,
    options: EvalSmokeOptions,
) -> Result<EvalSmokeOutcome> {
    let started_at = Utc::now();
    let run_id = format!(
        "product-{}-{}",
        started_at.format("%Y%m%dT%H%M%S%3fZ"),
        std::process::id()
    );
    let runtime = EnginePoolRuntime::new(Arc::new(pool));
    // Snapshot the complete tested model once. Each case receives a fresh single-consume
    // selection derived from this private snapshot, so preference changes cannot mix models.
    let suite_model = runtime.capture_eval_suite_model()?;
    let tested_identity = suite_model.identity().clone();
    let metadata = EvalRunMetadata {
        schema_version: 1,
        run_id,
        mode: EvalMode::Product,
        case_set: "plep_smoke".to_string(),
        case_set_version: "1".to_string(),
        pinvou_version: env!("CARGO_PKG_VERSION").to_string(),
        provider: tested_identity.provider.clone(),
        model: tested_identity.model.clone(),
        started_at: started_at.to_rfc3339(),
    };
    let mut report = EvalReportWriter::create(metadata.clone())?;
    let smoke_cases = cases::smoke_cases();
    let suite = run_eval_suite_with_model_factory(
        runtime.clone(),
        &smoke_cases,
        |_| suite_model.derive_case_selection().map(Some),
        |case, record| report.append(&case.case_id, record),
    )
    .await?;
    drop(suite_model);
    // Rules are intentionally computed before any Judge request.
    let rules = analyze_rules(&smoke_cases, &suite.records);
    let trusted_case_ids = smoke_cases
        .iter()
        .map(|case| case.case_id.clone())
        .collect::<Vec<_>>();
    let product_score = score_product_run(&suite.records, &rules.findings, &trusted_case_ids);
    let judge = analyze_with_product_judge(
        runtime,
        tested_identity,
        options.judge_model_id.as_deref(),
        &suite.records,
    )
    .await;
    finalize_eval_outputs(
        report,
        &metadata,
        suite,
        rules,
        product_score,
        judge,
        &trusted_case_ids,
    )
}

fn finalize_eval_outputs(
    report: EvalReportWriter,
    metadata: &EvalRunMetadata,
    suite: EvalSuiteResult,
    rules: crate::features::assistant::eval::analysis::RuleAnalysis,
    product_score: ProductScore,
    judge: JudgeReport,
    trusted_case_ids: &[String],
) -> Result<EvalSmokeOutcome> {
    let all_succeeded = suite.all_succeeded();
    let limitations = rules.limitations.clone();
    let product_diagnoses = summarize_product_problems(&rules.findings, &judge, trusted_case_ids);
    let findings = merge_findings(rules, judge.findings.clone());
    let status = analysis_status_label(&judge.status);
    // JSONL is finalized first by contract. A later Markdown error is returned while this
    // durable fact report remains available for diagnosis.
    let product_score_version = product_score.total.map(|_| product_score.version.as_str());
    let jsonl_report_path = absolute_path(report.finish(
        all_succeeded,
        Some(status),
        product_score.total,
        product_score_version,
        None,
    )?)?;
    let markdown_outcome = write_markdown_report(
        &jsonl_report_path,
        &EvalMarkdownReport {
            metadata,
            records: &suite.records,
            findings: &findings,
            judge: &judge,
            product_score: &product_score,
            product_diagnoses: &product_diagnoses,
            limitations: &limitations,
        },
    )?;
    let markdown_report_path = absolute_path(markdown_outcome.path)?;
    Ok(EvalSmokeOutcome {
        all_succeeded,
        jsonl_report_path,
        markdown_report_path,
        markdown: markdown_outcome.markdown,
        judge_status: JudgeStatus::from(&judge.status),
        product_score: product_score.total,
        product_score_version: product_score_version.map(str::to_string),
    })
}

fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()
            .context("resolve current directory for eval report")?
            .join(path))
    }
}

impl From<&AnalysisJudgeStatus> for JudgeStatus {
    fn from(status: &AnalysisJudgeStatus) -> Self {
        match status {
            AnalysisJudgeStatus::Completed => Self::Completed,
            AnalysisJudgeStatus::NotConfigured => Self::NotConfigured,
            AnalysisJudgeStatus::SkippedSameModel { .. } => Self::SkippedSameModel,
            AnalysisJudgeStatus::Failed { .. } => Self::Failed,
        }
    }
}

pub fn judge_status_label(status: &JudgeStatus) -> &'static str {
    match status {
        JudgeStatus::Completed => "completed",
        JudgeStatus::NotConfigured => "not_configured",
        JudgeStatus::SkippedSameModel => "skipped_same_model",
        JudgeStatus::Failed => "failed",
    }
}

fn analysis_status_label(status: &AnalysisJudgeStatus) -> &'static str {
    judge_status_label(&JudgeStatus::from(status))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::assistant::eval::{EvalAnalysisMaterial, EvalRecord};

    fn completed_record(case_id: &str) -> EvalRecord {
        EvalRecord {
            case_id: case_id.to_string(),
            session_id: format!("eval_{case_id}"),
            turn_id: format!("turn_{case_id}"),
            status: "Completed".to_string(),
            error: None,
            usage: None,
            milestones: Vec::new(),
            elapsed_ms: 1,
            analysis: EvalAnalysisMaterial::default(),
        }
    }

    fn metadata(run_id: &str) -> EvalRunMetadata {
        EvalRunMetadata {
            schema_version: 1,
            run_id: run_id.to_string(),
            mode: EvalMode::Product,
            case_set: "test".to_string(),
            case_set_version: "1".to_string(),
            pinvou_version: "test".to_string(),
            provider: "tested-provider".to_string(),
            model: "tested-model".to_string(),
            started_at: "2026-08-12T00:00:00Z".to_string(),
        }
    }

    fn failed_judge() -> JudgeReport {
        JudgeReport {
            status: AnalysisJudgeStatus::Failed {
                reason: "provider detail must stay hidden".to_string(),
            },
            dimensions: Vec::new(),
            findings: Vec::new(),
        }
    }

    fn with_isolated_home<T>(test: impl FnOnce() -> T) -> T {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os("PINVOU3_HOME");
        let home = std::env::temp_dir().join(format!(
            "pinvou-eval-cli-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::env::set_var("PINVOU3_HOME", &home);
        let result = test();
        let _ = std::fs::remove_dir_all(&home);
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        result
    }

    #[test]
    fn failing_judge_still_writes_markdown_and_preserves_suite_success() {
        with_isolated_home(|| {
            let metadata = metadata("failed-judge");
            let report = EvalReportWriter::create(metadata.clone()).expect("create JSONL");
            let cases = vec![EvalCase::smoke("healthy", "private prompt")];
            let suite = EvalSuiteResult {
                records: vec![Ok(completed_record("healthy"))],
            };

            let rules = analyze_rules(&cases, &suite.records);
            let trusted_case_ids = vec!["healthy".to_string()];
            let product_score =
                score_product_run(&suite.records, &rules.findings, &trusted_case_ids);
            let outcome = finalize_eval_outputs(
                report,
                &metadata,
                suite,
                rules,
                product_score,
                failed_judge(),
                &trusted_case_ids,
            )
            .expect("Judge failure degrades to a report");

            assert!(outcome.all_succeeded);
            assert_eq!(outcome.judge_status, JudgeStatus::Failed);
            assert_eq!(outcome.product_score, Some(100));
            assert_eq!(
                outcome.product_score_version.as_deref(),
                Some(crate::features::assistant::eval::analysis::PRODUCT_SCORE_VERSION)
            );
            assert!(outcome.jsonl_report_path.is_absolute());
            assert!(outcome.markdown_report_path.is_absolute());
            assert!(outcome.markdown_report_path.is_file());
            assert!(outcome.markdown.contains("## 产品问题与改进方向"));
            assert!(outcome.markdown.contains("## 产品健康评分"));
            let complete = std::fs::read_to_string(&outcome.jsonl_report_path)
                .expect("read JSONL")
                .lines()
                .last()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("valid JSONL"))
                .expect("complete line");
            assert_eq!(complete["product_score"], 100);
            assert_eq!(
                complete["product_score_version"],
                crate::features::assistant::eval::analysis::PRODUCT_SCORE_VERSION
            );
        });
    }

    #[test]
    fn markdown_write_failure_returns_error_after_jsonl_is_finalized() {
        with_isolated_home(|| {
            let metadata = metadata("markdown-failure");
            let report = EvalReportWriter::create(metadata.clone()).expect("create JSONL");
            let jsonl_path = report.final_path().to_path_buf();
            let markdown_path = jsonl_path.with_extension("md");
            std::fs::write(&markdown_path, "occupied").expect("occupy Markdown path");
            let cases = vec![EvalCase::smoke("healthy", "private prompt")];
            let suite = EvalSuiteResult {
                records: vec![Ok(completed_record("healthy"))],
            };

            let rules = analyze_rules(&cases, &suite.records);
            let trusted_case_ids = vec!["healthy".to_string()];
            let product_score =
                score_product_run(&suite.records, &rules.findings, &trusted_case_ids);
            let error = finalize_eval_outputs(
                report,
                &metadata,
                suite,
                rules,
                product_score,
                failed_judge(),
                &trusted_case_ids,
            )
            .expect_err("Markdown no-overwrite must fail the operation");

            assert!(error.to_string().contains("already exists"));
            assert!(jsonl_path.is_file());
            assert_eq!(
                std::fs::read_to_string(markdown_path).expect("read occupied report"),
                "occupied"
            );
        });
    }
}
