//! 评测 smoke 命令：在 app 内直接跑 PLEP smoke case 并返回 Markdown 报告。
//!
//! 用法（dev console）：
//!   await window.__TAURI__.core.invoke('run_eval_smoke')

use std::sync::Arc;

use chrono::Utc;
use tauri::State;

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::assistant::eval::analysis::rules::analyze_rules;
use crate::features::assistant::eval::analysis::{
    merge_findings, score_product_run, summarize_product_problems, JudgeReport, JudgeStatus,
};
use crate::features::assistant::eval::markdown_report::{
    write_markdown_report, EvalMarkdownReport,
};
use crate::features::assistant::eval::report::{EvalReportWriter, EvalRunMetadata};
use crate::features::assistant::eval::{cases, run_eval_suite_with_model_factory, EvalMode};
use crate::features::assistant::product_runtime::{EnginePoolRuntime, ProductChatRuntime};

/// 运行 PLEP smoke 任务集，返回 Markdown 报告。
#[tauri::command]
pub async fn run_eval_smoke(pool: State<'_, EnginePool>) -> Result<String, String> {
    let started_at = Utc::now();
    let runtime = EnginePoolRuntime::new(Arc::new(pool.inner().clone()));
    let suite_model = runtime
        .capture_eval_suite_model()
        .map_err(|error| error.to_string())?;
    let identity = suite_model.identity().clone();
    let metadata = EvalRunMetadata {
        schema_version: 1,
        run_id: format!(
            "product-gui-{}-{}",
            started_at.format("%Y%m%dT%H%M%S%3fZ"),
            std::process::id()
        ),
        mode: EvalMode::Product,
        case_set: "plep_smoke".to_string(),
        case_set_version: "1".to_string(),
        pinvou_version: env!("CARGO_PKG_VERSION").to_string(),
        provider: identity.provider,
        model: identity.model,
        started_at: started_at.to_rfc3339(),
    };
    let result = run_smoke_with_runtime(runtime, metadata, |_| {
        suite_model.derive_case_selection().map(Some)
    })
    .await
    .map_err(|error| error.to_string());
    drop(suite_model);
    result
}

async fn run_smoke_with_runtime<R, M>(
    runtime: R,
    metadata: EvalRunMetadata,
    mut model_for_case: M,
) -> anyhow::Result<String>
where
    R: ProductChatRuntime,
    M: FnMut(
        &crate::features::assistant::eval::EvalCase,
    ) -> anyhow::Result<
        Option<crate::features::assistant::eval::analysis::EvalModelSelection>,
    >,
{
    let smoke_cases = cases::smoke_cases();
    let mut report = EvalReportWriter::create(metadata.clone())?;
    let suite = run_eval_suite_with_model_factory(
        runtime,
        &smoke_cases,
        |case| model_for_case(case),
        |case, record| report.append(&case.case_id, record),
    )
    .await?;
    let all_succeeded = suite.all_succeeded();
    let rules = analyze_rules(&smoke_cases, &suite.records);
    let limitations = rules.limitations.clone();
    let trusted_case_ids = smoke_cases
        .iter()
        .map(|case| case.case_id.clone())
        .collect::<Vec<_>>();
    let product_score = score_product_run(&suite.records, &rules.findings, &trusted_case_ids);
    let judge = JudgeReport {
        status: JudgeStatus::NotConfigured,
        dimensions: Vec::new(),
        findings: Vec::new(),
    };
    let product_diagnoses = summarize_product_problems(&rules.findings, &judge, &trusted_case_ids);
    let findings = merge_findings(rules, Vec::new());
    let product_score_version = product_score.total.map(|_| product_score.version.as_str());
    let jsonl_path = report.finish(
        all_succeeded,
        Some("not_configured"),
        product_score.total,
        product_score_version,
        None,
    )?;
    let markdown = write_markdown_report(
        &jsonl_path,
        &EvalMarkdownReport {
            metadata: &metadata,
            records: &suite.records,
            findings: &findings,
            judge: &judge,
            product_score: &product_score,
            product_diagnoses: &product_diagnoses,
            limitations: &limitations,
        },
    )?;
    Ok(markdown.markdown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::assistant::eval::mock::MockRuntime;

    #[tokio::test]
    async fn command_eval_uses_shared_suite_and_writes_rule_only_reports() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os("PINVOU3_HOME");
        let home = std::env::temp_dir().join(format!(
            "pinvou-gui-eval-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::env::set_var("PINVOU3_HOME", &home);

        let report =
            run_smoke_with_runtime(MockRuntime::immediate(), test_metadata(), |_| Ok(None))
                .await
                .expect("run shared smoke suite");

        for case in cases::smoke_cases() {
            assert!(report.contains(&case.case_id));
            let session_id = format!("eval_{}", &case.case_id[..case.case_id.len().min(16)]);
            let _ =
                std::fs::remove_file(crate::platform::paths::session_timing_events(&session_id));
        }
        assert!(report.contains("## 产品健康评分"));
        assert!(report.contains("100/100"));
        assert!(report.contains("## 产品问题与改进方向"));
        assert!(report.contains("## 独立 Judge 质量评分"));
        assert!(report.contains("状态：未配置"));
        let outputs = std::fs::read_dir(crate::platform::paths::eval_reports_dir())
            .expect("read eval reports")
            .map(|entry| entry.expect("report entry").path())
            .collect::<Vec<_>>();
        assert!(outputs
            .iter()
            .any(|path| path.extension().is_some_and(|ext| ext == "jsonl")));
        assert!(outputs
            .iter()
            .any(|path| path.extension().is_some_and(|ext| ext == "md")));
        let jsonl_path = outputs
            .iter()
            .find(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .expect("JSONL report");
        let complete = std::fs::read_to_string(jsonl_path)
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

        let _ = std::fs::remove_dir_all(&home);
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }

    fn test_metadata() -> crate::features::assistant::eval::report::EvalRunMetadata {
        crate::features::assistant::eval::report::EvalRunMetadata {
            schema_version: 1,
            run_id: "gui-test".to_string(),
            mode: crate::features::assistant::eval::EvalMode::Product,
            case_set: "plep_smoke".to_string(),
            case_set_version: "1".to_string(),
            pinvou_version: "test".to_string(),
            provider: "mock".to_string(),
            model: "mock-model".to_string(),
            started_at: "2026-08-12T00:00:00Z".to_string(),
        }
    }
}
