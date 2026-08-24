use std::time::Duration;

use adapter_smoke::{
    JudgeDimensionScore, JudgeStatus, JudgeWireResponse, ProductScoreConfidence,
    ProductScoreDimension, SmokeAnalysisMaterial, SmokeRecord, SmokeToolEvent, SmokeUsage,
    ToolExpectation, analyze_rules, calculate_product_score, is_low_cache_hit_ratio,
    latency_exceeds_twice_median, parse_judge_response, render_smoke_markdown, smoke_cases,
};
use agent_backend_api::PrivateInputResolver;
use benchmark_core::{
    BenchmarkAdapter, BenchmarkError, ExecutionRequest, PredictionRetention, TaskSelection,
};
use benchmark_core::{TaskOutcome, TaskStatus};

#[test]
fn product_score_saturates_deductions_for_large_record_sets() {
    let records = (0..3_000)
        .map(|index| {
            SmokeRecord::new(
                TaskOutcome::new(
                    format!("stress-{index}"),
                    TaskStatus::Failed,
                    None,
                    vec![],
                    1,
                ),
                SmokeAnalysisMaterial::default(),
            )
        })
        .collect::<Vec<_>>();

    let score = calculate_product_score(&records, &[]).expect("large score remains valid");
    assert_eq!(score.dimensions().task_completion(), 0);
}

#[test]
fn score_and_markdown_reject_forged_sensitive_finding_text() {
    let forged: adapter_smoke::SmokeFinding = serde_json::from_value(serde_json::json!({
        "id": "case_failed",
        "severity": "p0",
        "case_id": "plep_smoke_hi",
        "title": "authorization: Bearer secret-value",
        "recommendation": "do not render this secret"
    }))
    .expect("wire shape");
    let records = vec![SmokeRecord::new(
        TaskOutcome::new("plep_smoke_hi", TaskStatus::Failed, None, vec![], 1),
        SmokeAnalysisMaterial::default(),
    )];
    let analysis: adapter_smoke::RuleAnalysis = serde_json::from_value(serde_json::json!({
        "findings": [forged.clone()],
        "limitations": []
    }))
    .expect("wire shape");

    assert!(calculate_product_score(&records, &[forged]).is_err());
    let safe_score = calculate_product_score(&records, &[]).expect("safe score");
    assert!(
        render_smoke_markdown(
            &records,
            &analysis,
            &safe_score,
            &adapter_smoke::not_configured_judge(),
        )
        .is_err()
    );
}

#[test]
fn score_rejects_sensitive_record_identifiers_before_deductions() {
    let records = vec![SmokeRecord::new(
        TaskOutcome::new("authorization-secret", TaskStatus::Failed, None, vec![], 1),
        SmokeAnalysisMaterial::default(),
    )];
    assert!(calculate_product_score(&records, &[]).is_err());
}

#[test]
fn product_diagnosis_aggregates_fixed_guidance_and_trusted_case_ids() {
    let findings = vec![
        serde_json::from_value(serde_json::json!({
            "id": "required_tool_missing", "severity": "p1",
            "case_id": "plep_smoke_weather", "title": "untrusted title",
            "recommendation": "untrusted recommendation"
        }))
        .expect("wire shape"),
        serde_json::from_value(serde_json::json!({
            "id": "tool_event_failed", "severity": "p0",
            "case_id": "not-in-suite", "title": "untrusted secondary title",
            "recommendation": "untrusted secondary recommendation"
        }))
        .expect("wire shape"),
    ];
    let records = vec![SmokeRecord::new(
        TaskOutcome::new("plep_smoke_weather", TaskStatus::Completed, None, vec![], 1),
        SmokeAnalysisMaterial::default(),
    )];

    let score = adapter_smoke::calculate_product_score_with_trusted_cases(
        &records,
        &findings,
        &["plep_smoke_weather"],
    )
    .expect("diagnosis uses policy text, not finding text");
    let diagnosis = &score.diagnoses()[0];
    assert_eq!(
        diagnosis.area(),
        adapter_smoke::ProductProblemArea::Toolchain
    );
    assert_eq!(diagnosis.severity(), adapter_smoke::FindingSeverity::P0);
    assert_eq!(diagnosis.source(), adapter_smoke::FindingSource::Rule);
    assert_eq!(diagnosis.affected_case_ids(), &["plep_smoke_weather"]);
    assert_eq!(diagnosis.affected_case_count(), 1);
    assert!(diagnosis.acceptance().contains("连续 3 次"));
    let rendered = format!(
        "{} {} {} {}",
        diagnosis.conclusion(),
        diagnosis.evidence(),
        diagnosis.action(),
        diagnosis.acceptance()
    );
    assert!(!rendered.contains("authorization"));
    assert!(!rendered.contains("untrusted"));

    let analysis: adapter_smoke::RuleAnalysis = serde_json::from_value(serde_json::json!({
        "findings": findings,
        "limitations": []
    }))
    .expect("wire shape");
    let markdown = render_smoke_markdown(
        &records,
        &analysis,
        &score,
        &adapter_smoke::not_configured_judge(),
    )
    .expect("validated markdown");
    assert!(!markdown.contains("untrusted"));
    assert!(markdown.contains("工具链调用可靠性不足"));
}

#[test]
fn smoke_cases_preserve_the_legacy_golden_contract() {
    let cases = smoke_cases();
    let actual = cases
        .iter()
        .map(|case| {
            (
                case.id(),
                case.prompt(),
                case.timeout(),
                case.tool_expectation(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        actual,
        vec![
            (
                "plep_smoke_hi",
                "hi",
                Duration::from_secs(30),
                ToolExpectation::Forbidden
            ),
            (
                "plep_smoke_weather",
                "广州今天天气怎么样",
                Duration::from_secs(60),
                ToolExpectation::Required
            ),
            (
                "plep_smoke_math",
                "1+1等于几",
                Duration::from_secs(30),
                ToolExpectation::Forbidden
            ),
            (
                "plep_smoke_poem",
                "帮我写一首关于春天的诗",
                Duration::from_secs(60),
                ToolExpectation::Forbidden
            ),
            (
                "plep_smoke_date",
                "今天星期几",
                Duration::from_secs(30),
                ToolExpectation::Optional
            ),
        ]
    );
}

#[test]
fn rules_and_health_score_use_only_safe_adapter_material() {
    let cases = smoke_cases();
    let records = vec![
        SmokeRecord::new(
            TaskOutcome::new("plep_smoke_hi", TaskStatus::Completed, None, vec![], 10),
            SmokeAnalysisMaterial::new(vec!["web_search".into()]),
        ),
        SmokeRecord::new(
            TaskOutcome::new(
                "plep_smoke_weather",
                TaskStatus::Completed,
                None,
                vec![],
                20,
            ),
            SmokeAnalysisMaterial::new(vec![]),
        ),
    ];

    let analysis = analyze_rules(&cases, &records);
    assert_eq!(analysis.findings().len(), 2);
    assert_eq!(analysis.findings()[0].id(), "unexpected_tool_use");
    assert_eq!(analysis.findings()[1].id(), "required_tool_missing");

    let score = calculate_product_score(&records, analysis.findings()).expect("safe findings");
    assert_eq!(score.version(), "pinvou-product-score/v1");
    assert!(score.total().is_some_and(|score| score < 100));
    assert!(!score.is_official_score());

    let debug = format!("{records:?}");
    assert!(!debug.contains("prompt"));
    assert!(!debug.contains("answer"));
    assert!(!debug.contains("provider error"));
}

#[test]
fn judge_requires_exactly_the_six_distinct_dimensions() {
    let dimensions = [
        "task_completion",
        "correctness",
        "tool_choice",
        "efficiency",
        "safety_boundaries",
        "overall_quality",
    ]
    .into_iter()
    .map(|dimension| JudgeDimensionScore::new(dimension, 80, 0.9, "safe evidence"))
    .collect();

    let report = parse_judge_response(JudgeWireResponse::new(dimensions, vec![]))
        .expect("valid strict judge report");
    assert_eq!(report.status(), &JudgeStatus::Completed);

    let duplicate = vec![JudgeDimensionScore::new("task_completion", 80, 0.9, "safe"); 6];
    assert!(parse_judge_response(JudgeWireResponse::new(duplicate, vec![])).is_err());
    assert_eq!(
        adapter_smoke::not_configured_judge().status(),
        &JudgeStatus::NotConfigured
    );
}

#[test]
fn markdown_calls_the_number_a_smoke_health_score_not_an_official_score() {
    let records = vec![SmokeRecord::new(
        TaskOutcome::new("plep_smoke_hi", TaskStatus::Completed, None, vec![], 10),
        SmokeAnalysisMaterial::default(),
    )];
    let analysis = analyze_rules(&smoke_cases(), &records);
    let score = calculate_product_score(&records, analysis.findings()).expect("safe findings");
    let markdown = render_smoke_markdown(
        &records,
        &analysis,
        &score,
        &adapter_smoke::not_configured_judge(),
    )
    .expect("safe markdown");

    assert!(markdown.contains("Smoke Health Score"));
    assert!(markdown.contains("不是官方 benchmark 分数"));
    assert!(!markdown.contains("Official Score"));
}

#[test]
fn adapter_plans_smoke_tasks_but_refuses_to_publish_an_official_score() {
    let adapter = adapter_smoke::SmokeAdapter::new();
    assert_eq!(adapter.descriptor().id().as_str(), "smoke");
    assert_eq!(
        adapter.private_output_retention(),
        PredictionRetention::Ephemeral
    );
    let dataset = adapter
        .verify_dataset(std::path::Path::new("."))
        .expect("smoke has no external dataset");
    let plan = adapter.plan(&dataset, &TaskSelection::all()).expect("plan");
    assert_eq!(plan.tasks().len(), 5);

    let error = adapter
        .score(&benchmark_core::CompletedRun::new("smoke-run", vec![]))
        .expect_err("Smoke Health must never masquerade as an official score");
    assert!(matches!(error, BenchmarkError::Contract(_)));
}

#[test]
fn private_input_store_keeps_prompts_behind_opaque_ids() {
    fn assert_resolver<T: PrivateInputResolver>() {}
    assert_resolver::<adapter_smoke::SmokePrivateInputs>();

    let inputs = adapter_smoke::SmokePrivateInputs::new();
    let case = &smoke_cases()[0];
    let task = case.to_benchmark_task();
    let ExecutionRequest::NativeTurn { prompt_handle, .. } = task.execution() else {
        panic!("Smoke task must be native");
    };

    assert_ne!(prompt_handle.expose_to_backend(), case.prompt());
    let resolved = inputs
        .resolve_handle(prompt_handle)
        .expect("known prompt id");
    assert_eq!(resolved.prompt().expose_to_backend(), case.prompt());
    assert!(!format!("{inputs:?}").contains(case.prompt()));
}

#[test]
fn deterministic_rules_match_legacy_failure_tool_and_performance_boundaries() {
    let cases = smoke_cases();
    let records = vec![
        SmokeRecord::new(
            TaskOutcome::new("plep_smoke_hi", TaskStatus::Timeout, None, vec![], 30_000),
            SmokeAnalysisMaterial::default(),
        ),
        SmokeRecord::new(
            TaskOutcome::new(
                "plep_smoke_weather",
                TaskStatus::Completed,
                None,
                vec![],
                10_000,
            ),
            SmokeAnalysisMaterial::default(),
        ),
        SmokeRecord::new(
            TaskOutcome::new(
                "plep_smoke_math",
                TaskStatus::Completed,
                None,
                vec![],
                30_000,
            ),
            SmokeAnalysisMaterial::with_details(
                vec![
                    SmokeToolEvent::new("web_search", true),
                    SmokeToolEvent::new("web_search", false),
                    SmokeToolEvent::new("web_search", false),
                ],
                Some(SmokeUsage::new(40_000, 24, 76)),
            ),
        ),
        SmokeRecord::new(
            TaskOutcome::new(
                "plep_smoke_poem",
                TaskStatus::Completed,
                None,
                vec![],
                10_000,
            ),
            SmokeAnalysisMaterial::default(),
        ),
    ];

    let analysis = analyze_rules(&cases, &records);
    let ids = analysis
        .findings()
        .iter()
        .map(|finding| finding.id())
        .collect::<Vec<_>>();
    for expected in [
        "case_failed",
        "tool_event_failed",
        "required_tool_missing",
        "unexpected_tool_use",
        "repeated_tool_use",
        "slow_high_token",
        "low_cache_hit_ratio",
        "latency_outlier",
    ] {
        assert!(ids.contains(&expected), "missing {expected}: {ids:?}");
    }
    assert_eq!(
        analysis
            .findings()
            .iter()
            .find(|finding| finding.id() == "tool_event_failed")
            .unwrap()
            .severity(),
        adapter_smoke::FindingSeverity::P0
    );
    assert_eq!(
        analysis
            .findings()
            .iter()
            .find(|finding| finding.id() == "repeated_tool_use")
            .unwrap()
            .severity(),
        adapter_smoke::FindingSeverity::P2
    );
}

#[test]
fn latency_and_cache_helpers_preserve_exact_overflow_safe_boundaries() {
    assert!(!latency_exceeds_twice_median(12_000, &[]));
    assert!(latency_exceeds_twice_median(12_000, &[4_000]));
    assert!(!latency_exceeds_twice_median(10_001, &[5_000, 5_001]));
    assert!(latency_exceeds_twice_median(10_002, &[5_000, 5_001]));
    assert!(!latency_exceeds_twice_median(u64::MAX, &[u64::MAX]));

    assert!(!is_low_cache_hit_ratio(0, 0));
    assert!(!is_low_cache_hit_ratio(25, 75));
    assert!(is_low_cache_hit_ratio(24, 76));
    assert!(is_low_cache_hit_ratio(u64::MAX / 4 + 1, u64::MAX - 1));
}

#[test]
fn health_score_and_markdown_include_every_deterministic_rule_policy() {
    let cases = smoke_cases();
    let records = vec![SmokeRecord::new(
        TaskOutcome::new(
            "plep_smoke_math",
            TaskStatus::Completed,
            None,
            vec![],
            30_001,
        ),
        SmokeAnalysisMaterial::with_details(
            vec![SmokeToolEvent::new("web_search", true); 3],
            Some(SmokeUsage::new(40_000, 0, 100)),
        ),
    )];
    let analysis = analyze_rules(&cases, &records);
    let score = calculate_product_score(&records, analysis.findings()).expect("safe findings");
    assert!(score.total().is_some_and(|total| total < 100));
    let markdown = render_smoke_markdown(
        &records,
        &analysis,
        &score,
        &adapter_smoke::not_configured_judge(),
    )
    .expect("safe markdown");
    assert!(markdown.contains("工具"));
    assert!(markdown.contains("优化"));
    assert!(markdown.contains("不是官方 benchmark 分数"));
}

#[test]
fn score_always_deducts_non_completed_records_once_even_with_matching_rule_finding() {
    let records = vec![SmokeRecord::new(
        TaskOutcome::new("plep_smoke_hi", TaskStatus::Timeout, None, vec![], 30_000),
        SmokeAnalysisMaterial::default(),
    )];
    let analysis = analyze_rules(&smoke_cases(), &records);
    let score = calculate_product_score(&records, analysis.findings()).expect("safe findings");

    assert_eq!(score.confidence(), ProductScoreConfidence::LowSample);
    assert_eq!(score.dimensions().task_completion(), 65);
    assert_eq!(score.total(), Some(88));
    assert_eq!(score.deductions().len(), 1);
    assert_eq!(score.deductions()[0].finding_id(), "case_failed");
    assert_eq!(
        score.deductions()[0].dimension(),
        ProductScoreDimension::TaskCompletion
    );
}

#[test]
fn markdown_restores_versioned_dimensions_deductions_and_confidence_contract() {
    let records = vec![SmokeRecord::new(
        TaskOutcome::new("plep_smoke_hi", TaskStatus::Failed, None, vec![], 1),
        SmokeAnalysisMaterial::default(),
    )];
    let analysis = analyze_rules(&smoke_cases(), &records);
    let score = calculate_product_score(&records, analysis.findings()).expect("safe findings");
    let markdown = render_smoke_markdown(
        &records,
        &analysis,
        &score,
        &adapter_smoke::not_configured_judge(),
    )
    .expect("safe markdown");

    assert!(markdown.contains("pinvou-product-score/v1"));
    assert!(markdown.contains("LowSample"));
    assert!(markdown.contains("Task Completion"));
    assert!(markdown.contains("case_failed"));
    assert!(markdown.contains("-35"));
}

#[test]
fn judge_rejects_sensitive_or_oversized_dynamic_text_fail_closed() {
    fn dimensions(evidence: &str) -> Vec<JudgeDimensionScore> {
        [
            "task_completion",
            "correctness",
            "tool_choice",
            "efficiency",
            "safety_boundaries",
            "overall_quality",
        ]
        .into_iter()
        .map(|dimension| JudgeDimensionScore::new(dimension, 80, 0.9, evidence))
        .collect()
    }

    assert!(
        parse_judge_response(JudgeWireResponse::new(
            dimensions("authorization: Bearer secret-value"),
            vec![],
        ))
        .is_err()
    );
    assert!(
        parse_judge_response(JudgeWireResponse::new(dimensions(&"x".repeat(501)), vec![],))
            .is_err()
    );

    let valid = parse_judge_response(JudgeWireResponse::new(
        dimensions("fixed safe evidence"),
        vec![],
    ))
    .expect("safe bounded judge text");
    assert_eq!(valid.dimensions().len(), 6);
}
