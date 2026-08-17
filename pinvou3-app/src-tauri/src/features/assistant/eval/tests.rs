//! MockRuntime 端到端 smoke test。

#[cfg(test)]
mod tests {
    use crate::features::assistant::eval::analysis::rules::{
        analyze_rules, canonical_tool_label, classify_error, has_http_status,
        is_low_cache_hit_ratio, latency_exceeds_twice_median,
    };
    use crate::features::assistant::eval::analysis::{
        calculate_product_score as calculate_product_score_with_trusted, enforce_finding_safety,
        judge_report_is_usable, merge_findings, score_product_run, sort_findings,
        summarize_product_problems, EvalFinding, FindingSeverity, FindingSource,
        JudgeDimensionScore, JudgeReport, JudgeStatus, ProductDiagnosis, ProductGrade,
        ProductProblemArea, ProductScore, ProductScoreConfidence, ProductScoreDimension,
        RuleAnalysis, PRODUCT_SCORE_VERSION,
    };
    use crate::features::assistant::eval::markdown_report::{
        write_markdown_report, EvalMarkdownReport,
    };
    use crate::features::assistant::eval::mock::{MockConfig, MockRuntime};
    use crate::features::assistant::eval::report::{EvalReportWriter, EvalRunMetadata};
    use crate::features::assistant::eval::{
        run_eval_suite, run_eval_suite_with_model_factory, EvalAnalysisMaterial, EvalCase,
        EvalMode, EvalRecord, EvalToolEvent, PinvouChatRunner, ToolExpectation,
    };
    use crate::features::assistant::product_runtime::{
        ProductChatRuntime, RuntimeToolEvent, SessionSpec,
    };
    use crate::features::assistant::timing::TurnUsage;
    use deepseek_tui::tui::app::AppMode;
    use serde_json::Value;

    fn clean(sid: &str) {
        let _ = std::fs::remove_file(crate::platform::paths::session_timing_events(sid));
    }

    #[tokio::test]
    async fn runner_prepares_normal_sessions_without_a_model_override() {
        let mock = MockRuntime::immediate();
        let observer = mock.clone();
        let runner = PinvouChatRunner::new(mock);

        runner
            .run_case(&EvalCase::smoke("normal_model", "hello"))
            .await
            .expect("run case");

        assert_eq!(observer.prepared_model_ids(), vec![None]);
        clean("eval_normal_model");
    }

    #[tokio::test]
    async fn mock_records_an_explicit_judge_model_override() {
        let mock = MockRuntime::immediate();
        mock.prepare(&SessionSpec {
            session_id: "judge-session".to_string(),
            model_selection: Some(
                crate::features::assistant::eval::analysis::EvalModelSelection::new(
                    "mock-selection-token".to_string(),
                    Some("judge-model-id".to_string()),
                    crate::features::assistant::eval::analysis::ModelIdentity::new(
                        "judge-provider",
                        "judge-wire-model",
                    ),
                ),
            ),
        })
        .await
        .expect("prepare judge session");

        assert_eq!(
            mock.prepared_model_ids(),
            vec![Some("judge-model-id".to_string())]
        );
    }

    fn report_metadata(run_id: &str) -> EvalRunMetadata {
        EvalRunMetadata {
            schema_version: 1,
            run_id: run_id.to_string(),
            mode: EvalMode::Product,
            case_set: "plep_smoke".to_string(),
            case_set_version: "1".to_string(),
            pinvou_version: "test".to_string(),
            provider: "mock".to_string(),
            model: "mock-model".to_string(),
            started_at: "2026-08-12T00:00:00Z".to_string(),
        }
    }

    fn completed_record(case_id: &str) -> EvalRecord {
        EvalRecord {
            case_id: case_id.to_string(),
            session_id: format!("eval_{case_id}"),
            turn_id: format!("turn_{case_id}"),
            status: "Completed".to_string(),
            error: None,
            usage: Some(TurnUsage::default()),
            milestones: Vec::new(),
            elapsed_ms: 42,
            analysis: EvalAnalysisMaterial::default(),
        }
    }

    fn rule_case(case_id: &str, expectation: ToolExpectation) -> EvalCase {
        EvalCase {
            case_id: case_id.to_string(),
            user_message: "private prompt sentinel".to_string(),
            mode: AppMode::Yolo,
            restrict_tools: false,
            timeout_ms: 60_000,
            tool_expectation: expectation,
        }
    }

    fn finding(
        id: &str,
        source: FindingSource,
        severity: FindingSeverity,
        case_id: Option<&str>,
        category: &str,
        title: &str,
        evidence: &str,
    ) -> EvalFinding {
        EvalFinding {
            id: id.to_string(),
            source,
            severity,
            case_id: case_id.map(str::to_string),
            category: category.to_string(),
            title: title.to_string(),
            evidence: evidence.to_string(),
            impact: "impact".to_string(),
            recommendation: "recommendation".to_string(),
            confidence: None,
        }
    }

    fn markdown_jsonl_path(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-markdown-report-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(&directory).expect("create markdown report test directory");
        directory.join(format!("{name}.jsonl"))
    }

    fn empty_judge(status: JudgeStatus) -> JudgeReport {
        JudgeReport {
            status,
            dimensions: Vec::new(),
            findings: Vec::new(),
        }
    }

    fn completed_judge() -> JudgeReport {
        JudgeReport {
            status: JudgeStatus::Completed,
            dimensions: [
                "task_completion",
                "correctness",
                "tool_choice",
                "efficiency",
                "safety_boundaries",
                "overall_quality",
            ]
            .map(|dimension| JudgeDimensionScore {
                dimension: dimension.to_string(),
                score: 80,
                confidence: 0.8,
                evidence: "safe evidence".to_string(),
            })
            .to_vec(),
            findings: Vec::new(),
        }
    }

    fn product_score_finding(
        id: &str,
        source: FindingSource,
        case_id: Option<&str>,
        evidence: &str,
    ) -> EvalFinding {
        finding(
            id,
            source,
            FindingSeverity::P1,
            case_id,
            "score-test",
            "score test",
            evidence,
        )
    }

    fn calculate_product_score(
        records: &[anyhow::Result<EvalRecord>],
        findings: &[EvalFinding],
    ) -> ProductScore {
        let trusted_case_ids = records
            .iter()
            .filter_map(|record| record.as_ref().ok().map(|record| record.case_id.clone()))
            .chain(
                findings
                    .iter()
                    .filter_map(|finding| finding.case_id.clone()),
            )
            .collect::<Vec<_>>();
        calculate_product_score_with_trusted(records, findings, &trusted_case_ids)
    }

    fn diagnose(
        rule_findings: &[EvalFinding],
        judge: &JudgeReport,
        allowed_case_ids: &[&str],
    ) -> Vec<ProductDiagnosis> {
        let allowed_case_ids = allowed_case_ids
            .iter()
            .map(|case_id| (*case_id).to_string())
            .collect::<Vec<_>>();
        summarize_product_problems(rule_findings, judge, &allowed_case_ids)
    }

    fn empty_product_score() -> ProductScore {
        calculate_product_score(&[], &[])
    }

    #[test]
    fn product_score_clean_non_empty_run_is_excellent() {
        let records = vec![Ok(completed_record("clean"))];
        let score = calculate_product_score(&records, &[]);

        assert_eq!(score.total, Some(100));
        assert_eq!(score.grade, Some(ProductGrade::Excellent));
        assert_eq!(score.dimensions.task_completion, 100);
        assert_eq!(score.dimensions.tool_reliability, 100);
        assert_eq!(score.dimensions.constraint_adherence, 100);
        assert_eq!(score.dimensions.performance_efficiency, 100);
        assert_eq!(score.dimensions.runtime_stability, 100);
        assert_eq!(score.confidence, ProductScoreConfidence::LowSample);
        assert!(score.deductions.is_empty());
    }

    #[test]
    fn product_score_filters_sensitive_case_ids_at_domain_boundary() {
        let records = vec![Ok(completed_record("normal-case"))];
        let case_ids = [
            "glpat-private",
            "xoxb-private",
            "AKIA1234567890ABCDEF",
            "Bearer-private",
            "normal-case",
        ];
        let findings = case_ids
            .iter()
            .map(|case_id| {
                product_score_finding(
                    "repeated_tool_use",
                    FindingSource::Rule,
                    Some(case_id),
                    "safe evidence",
                )
            })
            .collect::<Vec<_>>();
        let trusted_case_ids = case_ids
            .iter()
            .map(|case_id| (*case_id).to_string())
            .collect::<Vec<_>>();

        let score = calculate_product_score_with_trusted(&records, &findings, &trusted_case_ids);

        assert_eq!(
            score
                .deductions
                .iter()
                .filter_map(|deduction| deduction.case_id.as_deref())
                .collect::<Vec<_>>(),
            vec!["normal-case"]
        );
        let serialized = serde_json::to_string(&score).expect("serialize safe score");
        for sensitive in &case_ids[..4] {
            assert!(!serialized.contains(sensitive));
        }

        let path = markdown_jsonl_path("product-score-safe-case-ids");
        let metadata = report_metadata("product-score-safe-case-ids");
        let judge = empty_judge(JudgeStatus::NotConfigured);
        let report = EvalMarkdownReport {
            metadata: &metadata,
            records: &records,
            findings: &[],
            judge: &judge,
            product_score: &score,
            product_diagnoses: &[],
            limitations: &[],
        };
        let markdown = write_markdown_report(&path, &report)
            .expect("safe score report")
            .markdown;
        assert!(markdown.contains("normal\\-case"));
        for sensitive in &case_ids[..4] {
            assert!(!markdown.contains(sensitive));
        }
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn eval_product_score_wiring_shared_score_helper_is_identical_for_same_inputs() {
        let records = vec![Ok(completed_record("shared-score"))];
        let findings = vec![product_score_finding(
            "latency_outlier",
            FindingSource::Rule,
            Some("shared-score"),
            "safe evidence",
        )];

        let trusted_case_ids = vec!["shared-score".to_string()];
        let cli_score = score_product_run(&records, &findings, &trusted_case_ids);
        let gui_score = score_product_run(&records, &findings, &trusted_case_ids);

        assert_eq!(cli_score, gui_score);
        assert_eq!(cli_score.total, Some(98));
    }

    #[test]
    fn product_score_empty_run_is_unavailable() {
        let score = calculate_product_score(&[], &[]);

        assert_eq!(score.total, None);
        assert_eq!(score.grade, None);
        assert_eq!(score.confidence, ProductScoreConfidence::Unavailable);
    }

    #[test]
    fn product_score_grade_boundaries_follow_four_band_contract() {
        let cases: [(u8, ProductGrade, &[(&str, &str, Option<&str>)]); 6] = [
            (
                90,
                ProductGrade::Excellent,
                &[
                    ("tool_event_failed", "tool-failure", Some("case")),
                    ("repeated_tool_use", "repeat", Some("case")),
                ],
            ),
            (
                89,
                ProductGrade::Good,
                &[
                    ("required_tool_missing", "required", Some("case")),
                    ("unexpected_tool_use", "unexpected", Some("case")),
                    ("low_cache_hit_ratio", "cache", Some("case")),
                ],
            ),
            (
                75,
                ProductGrade::Good,
                &[
                    ("case_failed", "failure", Some("failed-a")),
                    ("tool_event_failed", "tool-failure", Some("case")),
                    ("unexpected_tool_use", "unexpected", Some("case")),
                    ("latency_outlier", "latency", Some("case")),
                ],
            ),
            (
                74,
                ProductGrade::Fair,
                &[
                    ("case_failed", "failure", Some("failed-a")),
                    ("tool_event_failed", "tool-failure", Some("case")),
                    ("required_tool_missing", "required", Some("case")),
                ],
            ),
            (
                60,
                ProductGrade::Fair,
                &[
                    ("case_failed", "failure-a", Some("failed-a")),
                    ("case_failed", "failure-b", Some("failed-b")),
                    ("tool_event_failed", "tool-failure", Some("case")),
                    ("unexpected_tool_use", "unexpected", Some("case")),
                    ("slow_high_token", "slow", Some("case")),
                    ("low_cache_hit_ratio", "cache", Some("case")),
                ],
            ),
            (
                59,
                ProductGrade::HighRisk,
                &[
                    ("case_failed", "failure-a", Some("failed-a")),
                    ("case_failed", "failure-b", Some("failed-b")),
                    ("required_tool_missing", "required", Some("case")),
                    ("unexpected_tool_use", "unexpected", Some("case")),
                    ("slow_high_token", "slow", Some("case")),
                    ("latency_outlier", "latency", Some("case")),
                    ("low_cache_hit_ratio", "cache", Some("case")),
                ],
            ),
        ];
        let records = vec![Ok(completed_record("case"))];

        for (expected_total, expected_grade, deductions) in cases {
            let findings = deductions
                .iter()
                .map(|(id, evidence, case_id)| {
                    product_score_finding(id, FindingSource::Rule, *case_id, evidence)
                })
                .collect::<Vec<_>>();
            let score = calculate_product_score(&records, &findings);
            assert_eq!(score.total, Some(expected_total));
            assert_eq!(score.grade, Some(expected_grade));
        }
    }

    #[test]
    fn product_score_record_error_deducts_completion_without_exposing_error() {
        let records = vec![Err(anyhow::anyhow!("PRIVATE_ERROR_SENTINEL"))];

        let score = calculate_product_score(&records, &[]);

        assert_eq!(score.dimensions.task_completion, 65);
        assert_eq!(score.deductions.len(), 1);
        assert!(!score.deductions[0]
            .evidence
            .contains("PRIVATE_ERROR_SENTINEL"));
    }

    #[test]
    fn product_score_only_trimmed_case_insensitive_completed_status_succeeds() {
        for status in ["timeout", "runner_error", "error", "unknown", ""] {
            let mut record = completed_record("case");
            record.status = status.to_string();
            let score = calculate_product_score(&[Ok(record)], &[]);
            assert_eq!(score.dimensions.task_completion, 65, "status {status:?}");
        }

        for status in ["Completed", " completed ", "cOmPlEtEd"] {
            let mut record = completed_record("case");
            record.status = status.to_string();
            let score = calculate_product_score(&[Ok(record)], &[]);
            assert_eq!(score.dimensions.task_completion, 100, "status {status:?}");
        }
    }

    #[test]
    fn product_score_status_and_same_case_rule_failure_deduct_once() {
        let mut record = completed_record("case");
        record.status = " timeout ".to_string();
        let findings = vec![product_score_finding(
            "case_failed",
            FindingSource::Rule,
            Some("case"),
            "status failure",
        )];

        let score = calculate_product_score(&[Ok(record)], &findings);

        assert_eq!(score.dimensions.task_completion, 65);
        assert_eq!(score.deductions.len(), 1);
    }

    #[test]
    fn product_score_record_error_and_rule_failure_deduct_once() {
        let records = vec![Err(anyhow::anyhow!("runner failed"))];
        let findings = vec![product_score_finding(
            "case_failed",
            FindingSource::Rule,
            Some("case"),
            "rule failure",
        )];

        let score = calculate_product_score(&records, &findings);

        assert_eq!(score.dimensions.task_completion, 65);
        assert_eq!(score.deductions.len(), 1);
    }

    #[test]
    fn product_score_deduction_evidence_uses_fixed_safe_text() {
        let records = vec![Ok(completed_record("case"))];
        let findings = vec![product_score_finding(
            "tool_event_failed",
            FindingSource::Rule,
            Some("case"),
            "PRIVATE_FINDING_EVIDENCE_SENTINEL",
        )];

        let score = calculate_product_score(&records, &findings);

        assert_eq!(score.deductions.len(), 1);
        assert_eq!(score.deductions[0].evidence, "A tool event failed.");
        assert!(!score.deductions[0]
            .evidence
            .contains("PRIVATE_FINDING_EVIDENCE_SENTINEL"));
    }

    #[test]
    fn product_score_known_findings_only_deduct_their_dimensions() {
        let records = vec![Ok(completed_record("case"))];

        let tool_failure = calculate_product_score(
            &records,
            &[product_score_finding(
                "tool_event_failed",
                FindingSource::Rule,
                Some("case"),
                "tool failed",
            )],
        );
        assert_eq!(tool_failure.dimensions.tool_reliability, 70);
        assert_eq!(tool_failure.dimensions.task_completion, 100);
        assert_eq!(tool_failure.total, Some(93));

        let unexpected_tool = calculate_product_score(
            &records,
            &[product_score_finding(
                "unexpected_tool_use",
                FindingSource::Rule,
                Some("case"),
                "tool forbidden",
            )],
        );
        assert_eq!(unexpected_tool.dimensions.constraint_adherence, 75);
        assert_eq!(unexpected_tool.dimensions.tool_reliability, 100);
        assert_eq!(unexpected_tool.total, Some(96));

        let latency = calculate_product_score(
            &records,
            &[product_score_finding(
                "latency_outlier",
                FindingSource::Rule,
                Some("case"),
                "slow",
            )],
        );
        assert_eq!(latency.dimensions.performance_efficiency, 88);
        assert_eq!(latency.total, Some(98));
    }

    #[test]
    fn product_score_deduplicates_rule_findings_and_ignores_unknown_or_judge_findings() {
        let records = vec![Ok(completed_record("case"))];
        let duplicate = product_score_finding(
            "tool_event_failed",
            FindingSource::Rule,
            Some("case"),
            "same evidence",
        );
        let findings = vec![
            duplicate.clone(),
            duplicate,
            product_score_finding("unknown_rule", FindingSource::Rule, Some("case"), "unknown"),
            product_score_finding(
                "tool_event_failed",
                FindingSource::Judge,
                Some("case"),
                "judge evidence",
            ),
        ];

        let score = calculate_product_score(&records, &findings);

        assert_eq!(score.dimensions.tool_reliability, 70);
        assert_eq!(score.deductions.len(), 1);
    }

    #[test]
    fn product_score_uses_versioned_exact_deduction_allowlist() {
        let records = vec![Ok(completed_record("case"))];
        let expected = [
            ("case_failed", ProductScoreDimension::TaskCompletion, 35),
            (
                "tool_event_failed",
                ProductScoreDimension::ToolReliability,
                30,
            ),
            (
                "required_tool_missing",
                ProductScoreDimension::ToolReliability,
                25,
            ),
            (
                "repeated_tool_use",
                ProductScoreDimension::ToolReliability,
                10,
            ),
            (
                "unexpected_tool_use",
                ProductScoreDimension::ConstraintAdherence,
                25,
            ),
            (
                "slow_high_token",
                ProductScoreDimension::PerformanceEfficiency,
                20,
            ),
            (
                "latency_outlier",
                ProductScoreDimension::PerformanceEfficiency,
                12,
            ),
            (
                "low_cache_hit_ratio",
                ProductScoreDimension::RuntimeStability,
                15,
            ),
        ];

        assert_eq!(PRODUCT_SCORE_VERSION, "pinvou-product-score/v1");
        for (id, dimension, points) in expected {
            let score = calculate_product_score(
                &records,
                &[product_score_finding(
                    id,
                    FindingSource::Rule,
                    Some("case"),
                    id,
                )],
            );
            assert_eq!(score.version, PRODUCT_SCORE_VERSION);
            assert_eq!(score.deductions.len(), 1, "missing deduction for {id}");
            assert_eq!(
                score.deductions[0].dimension, dimension,
                "wrong dimension for {id}"
            );
            assert_eq!(score.deductions[0].points, points, "wrong points for {id}");
        }
    }

    #[test]
    fn product_score_clamps_dimensions_and_uses_fixed_integer_weights() {
        let records = (0..10)
            .map(|index| Ok(completed_record(&format!("case-{index}"))))
            .collect::<Vec<_>>();
        let findings = (0..4)
            .map(|index| {
                product_score_finding(
                    "tool_event_failed",
                    FindingSource::Rule,
                    Some("case"),
                    &format!("failure-{index}"),
                )
            })
            .chain(std::iter::once(product_score_finding(
                "unexpected_tool_use",
                FindingSource::Rule,
                Some("case"),
                "constraint",
            )))
            .collect::<Vec<_>>();

        let score = calculate_product_score(&records, &findings);

        assert_eq!(score.dimensions.tool_reliability, 0);
        assert_eq!(score.dimensions.constraint_adherence, 75);
        assert_eq!(score.total, Some(71));
        assert_eq!(score.confidence, ProductScoreConfidence::Standard);
        assert!(score.deductions.iter().all(|deduction| {
            deduction.dimension == ProductScoreDimension::ToolReliability
                || deduction.dimension == ProductScoreDimension::ConstraintAdherence
        }));
    }

    #[test]
    fn product_diagnosis_maps_every_known_finding_to_fixed_product_guidance() {
        let cases = [
            (
                "tool_event_failed",
                ProductProblemArea::Toolchain,
                FindingSeverity::P0,
                "失败率为 0%",
            ),
            (
                "required_tool_missing",
                ProductProblemArea::Toolchain,
                FindingSeverity::P1,
                "调用率达到 100%",
            ),
            (
                "repeated_tool_use",
                ProductProblemArea::Toolchain,
                FindingSeverity::P2,
                "无重复调用告警",
            ),
            (
                "unexpected_tool_use",
                ProductProblemArea::Constraints,
                FindingSeverity::P1,
                "禁止工具调用 0 次",
            ),
            (
                "forbidden_tool_use",
                ProductProblemArea::Constraints,
                FindingSeverity::P1,
                "禁止工具调用 0 次",
            ),
            (
                "latency_outlier",
                ProductProblemArea::Performance,
                FindingSeverity::P2,
                "中位数的 2 倍",
            ),
            (
                "slow_high_token",
                ProductProblemArea::Performance,
                FindingSeverity::P1,
                "中位数的 2 倍",
            ),
            (
                "low_cache_hit_ratio",
                ProductProblemArea::CacheStability,
                FindingSeverity::P1,
                "不低于 25%",
            ),
            (
                "case_failed",
                ProductProblemArea::TaskCompletion,
                FindingSeverity::P0,
                "任务完成率达到 100%",
            ),
            (
                "timeout",
                ProductProblemArea::TaskCompletion,
                FindingSeverity::P0,
                "任务完成率达到 100%",
            ),
        ];

        for (id, area, severity, acceptance_fragment) in cases {
            let diagnoses: Vec<ProductDiagnosis> = diagnose(
                &[finding(
                    id,
                    FindingSource::Rule,
                    severity,
                    Some("case-1"),
                    "PRIVATE_CATEGORY",
                    "PRIVATE_TITLE",
                    "PRIVATE_EVIDENCE",
                )],
                &empty_judge(JudgeStatus::NotConfigured),
                &["case-1"],
            );
            assert_eq!(diagnoses.len(), 1, "missing diagnosis for {id}");
            let diagnosis = &diagnoses[0];
            assert_eq!(diagnosis.area, area, "wrong area for {id}");
            assert_eq!(diagnosis.severity, severity, "wrong severity for {id}");
            assert_eq!(diagnosis.source, FindingSource::Rule);
            assert!(!diagnosis.conclusion.is_empty());
            assert!(!diagnosis.action.is_empty());
            assert!(diagnosis.acceptance.contains("连续 3 次"));
            assert!(diagnosis.acceptance.contains(acceptance_fragment));
            assert_eq!(diagnosis.affected_case_ids, vec!["case-1"]);
            assert_eq!(diagnosis.affected_case_count, 1);
        }
    }

    #[test]
    fn product_diagnosis_aggregates_area_uses_highest_rule_severity_and_safe_case_ids() {
        let diagnoses = diagnose(
            &[
                finding(
                    "repeated_tool_use",
                    FindingSource::Rule,
                    FindingSeverity::P2,
                    Some("case-b"),
                    "x",
                    "x",
                    "x",
                ),
                finding(
                    "tool_event_failed",
                    FindingSource::Rule,
                    FindingSeverity::P0,
                    Some("case-a"),
                    "x",
                    "x",
                    "x",
                ),
                finding(
                    "required_tool_missing",
                    FindingSource::Rule,
                    FindingSeverity::P1,
                    Some("case-b"),
                    "x",
                    "x",
                    "x",
                ),
                finding(
                    "required_tool_missing",
                    FindingSource::Rule,
                    FindingSeverity::P1,
                    Some("unsafe|PRIVATE_SENTINEL"),
                    "x",
                    "x",
                    "x",
                ),
            ],
            &empty_judge(JudgeStatus::NotConfigured),
            &["case-a", "case-b"],
        );

        assert_eq!(diagnoses.len(), 1);
        assert_eq!(diagnoses[0].severity, FindingSeverity::P0);
        assert_eq!(diagnoses[0].affected_case_ids, vec!["case-a", "case-b"]);
        assert_eq!(diagnoses[0].affected_case_count, 2);
        assert_eq!(
            diagnoses[0].evidence,
            "规则命中 4 次，涉及 2 个安全用例标识。"
        );
    }

    #[test]
    fn product_diagnosis_sorts_equal_severity_by_affected_count_descending_then_area() {
        let diagnoses = diagnose(
            &[
                finding(
                    "tool_event_failed",
                    FindingSource::Rule,
                    FindingSeverity::P1,
                    Some("tool-one"),
                    "x",
                    "x",
                    "x",
                ),
                finding(
                    "unexpected_tool_use",
                    FindingSource::Rule,
                    FindingSeverity::P1,
                    Some("constraint-one"),
                    "x",
                    "x",
                    "x",
                ),
                finding(
                    "unexpected_tool_use",
                    FindingSource::Rule,
                    FindingSeverity::P1,
                    Some("constraint-two"),
                    "x",
                    "x",
                    "x",
                ),
            ],
            &empty_judge(JudgeStatus::NotConfigured),
            &["tool-one", "constraint-one", "constraint-two"],
        );

        assert_eq!(diagnoses[0].area, ProductProblemArea::Constraints);
        assert_eq!(diagnoses[0].affected_case_count, 2);
        assert_eq!(diagnoses[1].area, ProductProblemArea::Toolchain);
        assert_eq!(diagnoses[1].affected_case_count, 1);
    }

    #[test]
    fn product_diagnosis_rule_source_wins_and_judge_only_keeps_provenance() {
        let rule = finding(
            "repeated_tool_use",
            FindingSource::Rule,
            FindingSeverity::P2,
            Some("rule-case"),
            "x",
            "x",
            "x",
        );
        let judge_finding = finding(
            "tool_event_failed",
            FindingSource::Judge,
            FindingSeverity::P0,
            Some("judge-case"),
            "x",
            "x",
            "x",
        );
        let mut judge = completed_judge();
        judge.findings.push(judge_finding);
        let mixed = diagnose(&[rule], &judge, &["rule-case", "judge-case"]);
        assert_eq!(mixed[0].source, FindingSource::Rule);
        assert_eq!(mixed[0].severity, FindingSeverity::P2);
        assert_eq!(mixed[0].affected_case_ids, vec!["rule-case"]);
        assert!(!mixed[0].conclusion.contains("AI 推断"));

        let mut judge = completed_judge();
        judge.findings.push(finding(
            "latency_outlier",
            FindingSource::Judge,
            FindingSeverity::P1,
            Some("judge-case"),
            "x",
            "x",
            "x",
        ));
        let judge_only = diagnose(&[], &judge, &["judge-case"]);
        assert_eq!(judge_only[0].source, FindingSource::Judge);
        assert_eq!(judge_only[0].severity, FindingSeverity::P1);
        assert!(judge_only[0].conclusion.starts_with("[AI 推断]"));
    }

    #[test]
    fn product_diagnosis_excludes_judge_findings_when_judge_is_not_usable() {
        let judge_finding = finding(
            "latency_outlier",
            FindingSource::Judge,
            FindingSeverity::P1,
            Some("judge-case"),
            "x",
            "x",
            "x",
        );
        for status in [
            JudgeStatus::Failed {
                reason: "hidden".to_string(),
            },
            JudgeStatus::NotConfigured,
            JudgeStatus::SkippedSameModel {
                reason: "same".to_string(),
            },
        ] {
            let mut judge = completed_judge();
            judge.status = status;
            judge.findings.push(judge_finding.clone());
            assert!(!judge_report_is_usable(&judge));
            assert!(diagnose(&[], &judge, &["judge-case"]).is_empty());
        }

        let mut invalid_reports = Vec::new();
        let mut missing = completed_judge();
        missing.dimensions.pop();
        invalid_reports.push(missing);
        let mut duplicate = completed_judge();
        duplicate.dimensions[5].dimension = "task_completion".to_string();
        invalid_reports.push(duplicate);
        let mut score = completed_judge();
        score.dimensions[0].score = 101;
        invalid_reports.push(score);
        let mut confidence = completed_judge();
        confidence.dimensions[0].confidence = f32::NAN;
        invalid_reports.push(confidence);
        let mut evidence = completed_judge();
        evidence.dimensions[0].evidence = "  ".to_string();
        invalid_reports.push(evidence);
        for mut judge in invalid_reports {
            judge.findings.push(judge_finding.clone());
            assert!(!judge_report_is_usable(&judge));
            assert!(diagnose(&[], &judge, &["judge-case"]).is_empty());
        }

        let mut valid = completed_judge();
        valid.findings.push(judge_finding);
        assert!(judge_report_is_usable(&valid));
        assert_eq!(diagnose(&[], &valid, &["judge-case"]).len(), 1);
    }

    #[test]
    fn product_diagnosis_uses_selected_findings_actual_highest_severity() {
        let diagnoses = diagnose(
            &[
                finding(
                    "latency_outlier",
                    FindingSource::Rule,
                    FindingSeverity::P0,
                    Some("a"),
                    "x",
                    "x",
                    "x",
                ),
                finding(
                    "slow_high_token",
                    FindingSource::Rule,
                    FindingSeverity::P2,
                    Some("b"),
                    "x",
                    "x",
                    "x",
                ),
            ],
            &empty_judge(JudgeStatus::NotConfigured),
            &["a", "b"],
        );
        assert_eq!(diagnoses[0].severity, FindingSeverity::P0);
    }

    #[test]
    fn product_diagnosis_has_stable_severity_area_order_and_five_area_cap() {
        let findings = vec![
            product_score_finding("latency_outlier", FindingSource::Rule, Some("p"), "x"),
            product_score_finding("low_cache_hit_ratio", FindingSource::Rule, Some("c"), "x"),
            product_score_finding("unexpected_tool_use", FindingSource::Rule, Some("x"), "x"),
            product_score_finding("required_tool_missing", FindingSource::Rule, Some("t"), "x"),
            product_score_finding("case_failed", FindingSource::Rule, Some("f"), "x"),
        ];
        let first = diagnose(
            &findings,
            &empty_judge(JudgeStatus::NotConfigured),
            &["p", "c", "x", "t", "f"],
        );
        let second = diagnose(
            &findings.into_iter().rev().collect::<Vec<_>>(),
            &empty_judge(JudgeStatus::NotConfigured),
            &["p", "c", "x", "t", "f"],
        );

        assert_eq!(first, second);
        assert_eq!(first.len(), 5);
        assert_eq!(
            first.iter().map(|item| item.area).collect::<Vec<_>>(),
            vec![
                ProductProblemArea::TaskCompletion,
                ProductProblemArea::Toolchain,
                ProductProblemArea::Constraints,
                ProductProblemArea::Performance,
                ProductProblemArea::CacheStability,
            ]
        );
    }

    #[test]
    fn product_diagnosis_unknown_or_empty_findings_return_no_diagnoses() {
        assert!(diagnose(&[], &empty_judge(JudgeStatus::NotConfigured), &[]).is_empty());
        assert!(diagnose(
            &[product_score_finding(
                "unknown_finding",
                FindingSource::Rule,
                Some("case"),
                "x",
            )],
            &empty_judge(JudgeStatus::NotConfigured),
            &["case"],
        )
        .is_empty());
    }

    #[test]
    fn product_diagnosis_never_copies_untrusted_finding_text() {
        let sentinel = "PRIVATE_PROMPT_ANSWER_TOKEN_SENTINEL";
        let diagnoses = diagnose(
            &[finding(
                "tool_event_failed",
                FindingSource::Rule,
                FindingSeverity::P0,
                Some("unsafe|PRIVATE_CASE_SENTINEL"),
                sentinel,
                sentinel,
                sentinel,
            )],
            &empty_judge(JudgeStatus::NotConfigured),
            &["safe-case"],
        );
        let rendered = serde_json::to_string(&diagnoses).expect("serialize diagnosis");
        assert!(!rendered.contains("PRIVATE"));
        assert_eq!(diagnoses[0].affected_case_count, 0);
    }

    #[test]
    fn product_diagnosis_only_emits_non_sensitive_ids_from_trusted_suite_allowlist() {
        let long_hash = "ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";
        let mut findings = vec![
            finding(
                "tool_event_failed",
                FindingSource::Rule,
                FindingSeverity::P0,
                Some("normal-case"),
                "x",
                "x",
                "x",
            ),
            finding(
                "required_tool_missing",
                FindingSource::Rule,
                FindingSeverity::P1,
                Some(long_hash),
                "x",
                "x",
                "x",
            ),
            finding(
                "repeated_tool_use",
                FindingSource::Rule,
                FindingSeverity::P2,
                Some("not-in-suite"),
                "x",
                "x",
                "x",
            ),
        ];
        let sensitive_ids = [
            "glpat-privatevalue",
            "xoxb-privatevalue",
            "AKIAABCDEFGHIJKLMNOP",
            "ghp_privatevalue",
            "github_pat_privatevalue",
            "sk-privatevalue",
            "Bearer-privatevalue",
        ];
        findings.extend(sensitive_ids.iter().map(|case_id| {
            finding(
                "tool_event_failed",
                FindingSource::Rule,
                FindingSeverity::P0,
                Some(case_id),
                "x",
                "x",
                "x",
            )
        }));
        let mut allowed = vec!["normal-case", long_hash];
        allowed.extend(sensitive_ids);
        let diagnoses = diagnose(
            &findings,
            &empty_judge(JudgeStatus::NotConfigured),
            &allowed,
        );
        let serialized = serde_json::to_string(&diagnoses).expect("serialize diagnosis");
        assert_eq!(
            diagnoses[0].affected_case_ids,
            vec![long_hash.to_string(), "normal-case".to_string()]
        );
        for sensitive_id in sensitive_ids {
            assert!(!serialized.contains(sensitive_id));
        }
        assert!(!serialized.contains("not-in-suite"));
    }

    #[test]
    fn markdown_report_renders_fixed_sections_sources_and_sorted_priorities() {
        let jsonl_path = markdown_jsonl_path("fixed-sections");
        let mut metadata = report_metadata("markdown` # heading **bold** [link](url) <tag>");
        metadata.model = "model` # heading **bold** [link](url) <tag>".to_string();
        let records = vec![Ok(completed_record(
            "case|` # heading **bold** [link](url) <tag>",
        ))];
        let findings = vec![
            finding(
                "p0",
                FindingSource::Rule,
                FindingSeverity::P0,
                Some("case|one"),
                "execution",
                "rule` # heading **bold** [link](url) <tag>",
                "line one\nline two ` # heading **bold** [link](url) <tag>",
            ),
            finding(
                "p1",
                FindingSource::Judge,
                FindingSeverity::P1,
                None,
                "quality",
                "judge title",
                "judge evidence",
            ),
            finding(
                "p2",
                FindingSource::Rule,
                FindingSeverity::P2,
                None,
                "performance",
                "minor title",
                "minor evidence",
            ),
        ];
        let judge = completed_judge();
        let limitations = vec!["仅适用于当前样本".to_string()];
        let report = EvalMarkdownReport {
            metadata: &metadata,
            records: &records,
            findings: &findings,
            judge: &judge,
            product_score: &empty_product_score(),
            product_diagnoses: &[],
            limitations: &limitations,
        };

        let outcome = write_markdown_report(&jsonl_path, &report).expect("write report");

        let headings = [
            "## 运行结论",
            "## 产品问题与改进方向",
            "## 产品健康评分",
            "## 关键指标",
            "## 逐用例诊断",
            "## 工具与性能观察",
            "## 确定性规则发现",
            "## 独立 Judge 质量评分",
            "## P0/P1/P2 改进建议",
            "## 评测限制与可比性说明",
        ];
        let positions = headings.map(|heading| {
            outcome
                .markdown
                .find(heading)
                .expect("fixed heading exists")
        });
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(outcome.markdown.contains("[规则事实]"));
        assert!(outcome.markdown.contains("[AI 推断]"));
        let p0 = outcome.markdown.find("### P0").expect("P0 heading");
        let p1 = outcome.markdown.find("### P1").expect("P1 heading");
        let p2 = outcome.markdown.find("### P2").expect("P2 heading");
        assert!(p0 < p1 && p1 < p2);
        for escaped in ["\\`", "\\#", "\\*\\*", "\\[link\\]\\(url\\)", "\\<tag\\>"] {
            assert!(outcome.markdown.contains(escaped));
        }
        assert!(outcome.markdown.contains("case\\|\\`"));
        assert!(outcome.markdown.contains("line one<br>line two"));
        assert!(!outcome.markdown.contains("**bold**"));
        assert!(!outcome.markdown.contains("[link](url)"));
        assert!(!outcome.markdown.contains("<tag>"));
        assert_eq!(outcome.path, jsonl_path.with_extension("md"));
        assert!(outcome.path.is_file());
        assert!(!jsonl_path.with_extension("md.tmp").exists());
        let _ = std::fs::remove_dir_all(jsonl_path.parent().expect("parent"));
    }

    #[test]
    fn markdown_product_score_renders_product_first_sections_and_fixed_deductions() {
        let path = markdown_jsonl_path("product-score");
        let metadata = report_metadata("product-score");
        let mut failed = completed_record("safe-case");
        failed.status = "Failed".to_string();
        let records = vec![Ok(failed)];
        let findings = vec![product_score_finding(
            "tool_event_failed",
            FindingSource::Rule,
            Some("safe-case"),
            "runtime detail must not replace fixed deduction evidence",
        )];
        let judge = empty_judge(JudgeStatus::NotConfigured);
        let score = calculate_product_score(&records, &findings);
        let diagnoses = diagnose(&findings, &judge, &["safe-case"]);
        let report = EvalMarkdownReport {
            metadata: &metadata,
            records: &records,
            findings: &findings,
            judge: &judge,
            product_score: &score,
            product_diagnoses: &diagnoses,
            limitations: &[],
        };

        let markdown = write_markdown_report(&path, &report)
            .expect("write product score report")
            .markdown;
        let headings = [
            "## 运行结论",
            "## 产品问题与改进方向",
            "## 产品健康评分",
            "## 关键指标",
            "## 逐用例诊断",
            "## 工具与性能观察",
            "## 确定性规则发现",
            "## 独立 Judge 质量评分",
            "## P0/P1/P2 改进建议",
            "## 评测限制与可比性说明",
        ];
        let positions = headings.map(|heading| markdown.find(heading).expect("fixed heading"));
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        for expected in [
            "[规则事实]",
            "工具链",
            "P1",
            "影响范围：1 个用例（safe\\-case）",
            "证据：规则命中 1 次",
            "建议动作：",
            "验收标准：",
            "总分：80/100",
            "等级：良好",
            PRODUCT_SCORE_VERSION,
            "任务完成：65",
            "工具可靠性：70",
            "约束遵循：100",
            "性能效率：100",
            "运行稳定性：100",
            "置信度：小样本",
            "样本量较小",
            "case_failed",
            "tool_event_failed",
            "Evaluation case did not complete.",
            "A tool event failed.",
            "公开榜单分数：不可用",
            "未使用官方数据集、协议、评分器与固定版本",
            "不能直接与 BFCL 比较",
            "仅可在相同 case 集、配置、模型、环境与评分公式版本之间进行内部比较",
        ] {
            assert!(markdown.contains(expected), "missing {expected}");
        }
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn markdown_product_score_empty_state_is_explicit_and_cautious() {
        let path = markdown_jsonl_path("product-empty");
        let metadata = report_metadata("product-empty");
        let judge = empty_judge(JudgeStatus::NotConfigured);
        let score = empty_product_score();
        let report = EvalMarkdownReport {
            metadata: &metadata,
            records: &[],
            findings: &[],
            judge: &judge,
            product_score: &score,
            product_diagnoses: &[],
            limitations: &[],
        };

        let markdown = write_markdown_report(&path, &report)
            .expect("empty report")
            .markdown;
        assert!(
            markdown.contains("本次 smoke 未发现规则可识别的问题；样本较小，不能证明产品无问题")
        );
        assert!(markdown.contains("扩大样本"));
        assert!(markdown.contains("连续运行"));
        assert!(markdown.contains("总分：不可用"));
        for unavailable in [
            "任务完成：不可用",
            "工具可靠性：不可用",
            "约束遵循：不可用",
            "性能效率：不可用",
            "运行稳定性：不可用",
        ] {
            assert!(markdown.contains(unavailable), "missing {unavailable}");
        }
        assert!(!markdown.contains("任务完成：100"));
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn markdown_empty_diagnosis_distinguishes_unmapped_rule_and_judge_only_findings() {
        let metadata = report_metadata("unmapped-diagnosis");
        let score = empty_product_score();
        let unknown_rule = finding(
            "future_rule",
            FindingSource::Rule,
            FindingSeverity::P1,
            Some("safe-case"),
            "safe",
            "safe",
            "safe",
        );
        let rule_path = markdown_jsonl_path("unmapped-rule-diagnosis");
        let rule_judge = empty_judge(JudgeStatus::NotConfigured);
        let rule_report = EvalMarkdownReport {
            metadata: &metadata,
            records: &[],
            findings: &[unknown_rule],
            judge: &rule_judge,
            product_score: &score,
            product_diagnoses: &[],
            limitations: &[],
        };
        let rule_markdown = write_markdown_report(&rule_path, &rule_report)
            .expect("unmapped rule report")
            .markdown;
        assert!(rule_markdown.contains("存在尚未归纳的问题，请查看确定性规则发现"));
        assert!(!rule_markdown.contains("本次 smoke 未发现规则可识别的问题"));

        let mut judge = completed_judge();
        let unknown_judge = finding(
            "future_judge",
            FindingSource::Judge,
            FindingSeverity::P2,
            Some("safe-case"),
            "safe",
            "safe",
            "safe",
        );
        judge.findings.push(unknown_judge.clone());
        let judge_path = markdown_jsonl_path("unmapped-judge-diagnosis");
        let judge_report = EvalMarkdownReport {
            metadata: &metadata,
            records: &[],
            findings: &[unknown_judge],
            judge: &judge,
            product_score: &score,
            product_diagnoses: &[],
            limitations: &[],
        };
        let judge_markdown = write_markdown_report(&judge_path, &judge_report)
            .expect("unmapped Judge report")
            .markdown;
        assert!(judge_markdown.contains("存在尚未归纳的 AI 推断，请查看独立 Judge 质量评分"));
        assert!(!judge_markdown.contains("存在尚未归纳的问题，请查看确定性规则发现"));

        let _ = std::fs::remove_dir_all(rule_path.parent().expect("parent"));
        let _ = std::fs::remove_dir_all(judge_path.parent().expect("parent"));
    }

    #[test]
    fn markdown_judge_requires_exact_shared_six_dimension_contract() {
        let path = markdown_jsonl_path("judge-extra-dimension");
        let metadata = report_metadata("judge-extra-dimension");
        let mut judge = completed_judge();
        judge.dimensions.push(JudgeDimensionScore {
            dimension: "extra_dimension".to_string(),
            score: 80,
            confidence: 0.8,
            evidence: "safe evidence".to_string(),
        });
        let report = EvalMarkdownReport {
            metadata: &metadata,
            records: &[],
            findings: &[],
            judge: &judge,
            product_score: &empty_product_score(),
            product_diagnoses: &[],
            limitations: &[],
        };

        let markdown = write_markdown_report(&path, &report)
            .expect("invalid Judge safely degrades")
            .markdown;
        assert!(markdown.contains("状态：无效 Judge 结果（已降级）"));
        assert!(!markdown.contains("状态：已完成"));
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn markdown_product_score_dynamic_diagnosis_is_escaped_and_credential_guarded() {
        let metadata = report_metadata("product-private");
        let judge = empty_judge(JudgeStatus::NotConfigured);
        let score = empty_product_score();
        for (index, value) in ["**unsafe** [link](url)", "api_key=secret-value"]
            .iter()
            .enumerate()
        {
            let path = markdown_jsonl_path(&format!("product-private-{index}"));
            let diagnosis = ProductDiagnosis {
                area: ProductProblemArea::Toolchain,
                severity: FindingSeverity::P1,
                source: FindingSource::Judge,
                affected_case_ids: vec!["safe-case".to_string()],
                affected_case_count: 1,
                conclusion: (*value).to_string(),
                evidence: "safe evidence".to_string(),
                action: "safe action".to_string(),
                acceptance: "safe acceptance".to_string(),
            };
            let report = EvalMarkdownReport {
                metadata: &metadata,
                records: &[],
                findings: &[],
                judge: &judge,
                product_score: &score,
                product_diagnoses: &[diagnosis],
                limitations: &[],
            };
            let outcome = write_markdown_report(&path, &report);
            if index == 0 {
                let markdown = outcome.expect("unsafe Markdown is escaped").markdown;
                assert!(markdown.contains("\\*\\*unsafe\\*\\*"));
                assert!(markdown.contains("[AI 推断]"));
                assert!(!markdown.contains("**unsafe**"));
            } else {
                assert!(outcome
                    .expect_err("credential rejected")
                    .to_string()
                    .contains("sensitive"));
            }
            let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
        }
    }

    #[test]
    fn markdown_product_score_is_invariant_when_judge_fails_or_is_not_configured() {
        let metadata = report_metadata("product-judge-invariance");
        let records = vec![Ok(completed_record("stable"))];
        let findings = vec![product_score_finding(
            "repeated_tool_use",
            FindingSource::Rule,
            Some("stable"),
            "safe",
        )];
        let score = calculate_product_score(&records, &findings);
        let statuses = [
            JudgeStatus::NotConfigured,
            JudgeStatus::Failed {
                reason: "hidden".to_string(),
            },
        ];
        for (index, status) in statuses.into_iter().enumerate() {
            let path = markdown_jsonl_path(&format!("product-judge-{index}"));
            let judge = empty_judge(status);
            let report = EvalMarkdownReport {
                metadata: &metadata,
                records: &records,
                findings: &findings,
                judge: &judge,
                product_score: &score,
                product_diagnoses: &[],
                limitations: &[],
            };
            let markdown = write_markdown_report(&path, &report)
                .expect("judge-independent score")
                .markdown;
            assert!(markdown.contains("总分：98/100"));
            assert!(markdown.contains("Product Score 不受影响"));
            assert!(markdown.contains("检查 Judge 模型配置与响应格式"));
            let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
        }
    }

    #[test]
    fn markdown_product_score_uses_needs_improvement_for_fair_grade() {
        let path = markdown_jsonl_path("product-fair-grade");
        let metadata = report_metadata("product-fair-grade");
        let judge = completed_judge();
        let mut score = empty_product_score();
        score.total = Some(70);
        score.grade = Some(ProductGrade::Fair);
        score.confidence = ProductScoreConfidence::Standard;
        let report = EvalMarkdownReport {
            metadata: &metadata,
            records: &[],
            findings: &[],
            judge: &judge,
            product_score: &score,
            product_diagnoses: &[],
            limitations: &[],
        };

        let markdown = write_markdown_report(&path, &report)
            .expect("fair score report")
            .markdown;
        assert!(markdown.contains("等级：需改进"));
        assert!(!markdown.contains("等级：一般"));
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn markdown_report_never_renders_analysis_or_raw_errors() {
        let jsonl_path = markdown_jsonl_path("private-fields");
        let metadata = report_metadata("private-fields");
        let mut record = completed_record("private");
        record.status = "Error".to_string();
        record.error = Some("Authorization: Bearer raw-error-secret".to_string());
        record.analysis = EvalAnalysisMaterial {
            user_message: "secret prompt".to_string(),
            assistant_text: "secret answer".to_string(),
            tool_events: Vec::new(),
        };
        let records = vec![
            Ok(record),
            Err(anyhow::anyhow!("api_key=raw-result-secret")),
        ];
        let judge = empty_judge(JudgeStatus::NotConfigured);
        let report = EvalMarkdownReport {
            metadata: &metadata,
            records: &records,
            findings: &[],
            judge: &judge,
            product_score: &empty_product_score(),
            product_diagnoses: &[],
            limitations: &[],
        };

        let outcome = write_markdown_report(&jsonl_path, &report).expect("write safe report");

        for secret in [
            "secret prompt",
            "secret answer",
            "raw-error-secret",
            "raw-result-secret",
        ] {
            assert!(!outcome.markdown.contains(secret));
        }
        assert!(outcome.markdown.contains("有（详情已隐藏）"));
        let _ = std::fs::remove_dir_all(jsonl_path.parent().expect("parent"));
    }

    #[test]
    fn markdown_report_renders_completed_judge_dimensions_in_fixed_order() {
        let metadata = report_metadata("judge-statuses");
        let records: Vec<anyhow::Result<EvalRecord>> = Vec::new();
        let dimensions = [
            "overall_quality",
            "safety_boundaries",
            "efficiency",
            "tool_choice",
            "correctness",
            "task_completion",
            "extra_dimension",
        ]
        .map(|dimension| JudgeDimensionScore {
            dimension: dimension.to_string(),
            score: 80,
            confidence: 0.8,
            evidence: "safe evidence".to_string(),
        })
        .to_vec();
        let statuses = vec![
            JudgeReport {
                status: JudgeStatus::Completed,
                dimensions,
                findings: Vec::new(),
            },
            empty_judge(JudgeStatus::NotConfigured),
            empty_judge(JudgeStatus::Failed {
                reason: "provider unavailable".to_string(),
            }),
            empty_judge(JudgeStatus::SkippedSameModel {
                reason: "same model".to_string(),
            }),
        ];

        for (index, judge) in statuses.iter().enumerate() {
            let jsonl_path = markdown_jsonl_path(&format!("judge-{index}"));
            let report = EvalMarkdownReport {
                metadata: &metadata,
                records: &records,
                findings: &[],
                judge,
                product_score: &empty_product_score(),
                product_diagnoses: &[],
                limitations: &[],
            };
            let outcome = write_markdown_report(&jsonl_path, &report).expect("write judge report");
            match &judge.status {
                JudgeStatus::Completed => {
                    let required = [
                        "task_completion",
                        "correctness",
                        "tool_choice",
                        "efficiency",
                        "safety_boundaries",
                        "overall_quality",
                    ];
                    let positions = required.map(|dimension| {
                        outcome
                            .markdown
                            .find(dimension)
                            .expect("required dimension")
                    });
                    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
                    assert!(outcome.markdown.contains("状态：已完成"));
                    assert!(!outcome.markdown.contains("未提供"));
                    assert!(!outcome.markdown.contains("extra_dimension"));
                }
                JudgeStatus::NotConfigured => assert!(outcome.markdown.contains("未配置")),
                JudgeStatus::Failed { .. } => assert!(outcome.markdown.contains("失败")),
                JudgeStatus::SkippedSameModel { .. } => {
                    assert!(outcome.markdown.contains("已跳过"))
                }
            }
            let _ = std::fs::remove_dir_all(jsonl_path.parent().expect("parent"));
        }
    }

    #[test]
    fn markdown_report_downgrades_malformed_completed_judge_results() {
        let metadata = report_metadata("malformed-judge");
        let records: Vec<anyhow::Result<EvalRecord>> = Vec::new();
        let dimension = |name: &str| JudgeDimensionScore {
            dimension: name.to_string(),
            score: 80,
            confidence: 0.8,
            evidence: "safe evidence".to_string(),
        };
        let valid = [
            "task_completion",
            "correctness",
            "tool_choice",
            "efficiency",
            "safety_boundaries",
            "overall_quality",
        ]
        .map(dimension)
        .to_vec();
        let mut duplicate = valid.clone();
        duplicate.push(dimension("task_completion"));
        let mut out_of_range = valid.clone();
        out_of_range[0].score = 101;
        let mut nan_confidence = valid.clone();
        nan_confidence[1].confidence = f32::NAN;
        let mut out_of_range_confidence = valid.clone();
        out_of_range_confidence[1].confidence = 1.1;
        let mut missing = valid.clone();
        missing.pop();
        let mut empty_evidence = valid;
        empty_evidence[2].evidence.clear();

        for (index, dimensions) in [
            duplicate,
            out_of_range,
            nan_confidence,
            out_of_range_confidence,
            missing,
            empty_evidence,
        ]
        .into_iter()
        .enumerate()
        {
            let path = markdown_jsonl_path(&format!("malformed-judge-{index}"));
            let judge = JudgeReport {
                status: JudgeStatus::Completed,
                dimensions,
                findings: Vec::new(),
            };
            let findings = vec![
                finding(
                    "judge-sentinel",
                    FindingSource::Judge,
                    FindingSeverity::P0,
                    None,
                    "tool_use",
                    "malformed judge sentinel",
                    "judge evidence sentinel",
                ),
                finding(
                    "rule-sentinel",
                    FindingSource::Rule,
                    FindingSeverity::P1,
                    None,
                    "tool_use",
                    "rule sentinel remains",
                    "rule evidence sentinel",
                ),
            ];
            let report = EvalMarkdownReport {
                metadata: &metadata,
                records: &records,
                findings: &findings,
                judge: &judge,
                product_score: &empty_product_score(),
                product_diagnoses: &[],
                limitations: &[],
            };
            let outcome = write_markdown_report(&path, &report).expect("write degraded report");
            assert!(outcome.markdown.contains("状态：无效 Judge 结果（已降级）"));
            assert!(!outcome.markdown.contains("状态：已完成"));
            assert_eq!(outcome.markdown.matches("未提供").count(), 18);
            assert!(!outcome.markdown.contains("malformed judge sentinel"));
            assert!(!outcome.markdown.contains("judge evidence sentinel"));
            assert!(outcome.markdown.contains("rule sentinel remains"));
            let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
        }
    }

    #[test]
    fn markdown_report_handles_empty_data_and_rejects_sensitive_output_fail_closed() {
        let metadata = report_metadata("empty");
        let records: Vec<anyhow::Result<EvalRecord>> = Vec::new();
        let judge = empty_judge(JudgeStatus::NotConfigured);
        let safe_path = markdown_jsonl_path("empty");
        let safe_limitations = vec!["input_tokens 字段名不是凭据".to_string()];
        let safe_report = EvalMarkdownReport {
            metadata: &metadata,
            records: &records,
            findings: &[],
            judge: &judge,
            product_score: &empty_product_score(),
            product_diagnoses: &[],
            limitations: &safe_limitations,
        };
        let safe = write_markdown_report(&safe_path, &safe_report).expect("empty report is stable");
        assert!(safe.markdown.contains("用例总数 | 0"));
        assert!(safe.markdown.contains("Token 统计 | 不可用"));

        for (index, credential) in [
            "Authorization : Bearer secret-value",
            "Authorization\t=\tBasic dXNlcjpwYXNzd29yZA==",
            "Authorization: custom-secret",
            "api key \t: \n secret-value",
            "api_key=secret-value",
            "api-key = secret-value",
            "apikey: secret-value",
            "Cookie \t= session=secret-value",
            "password: hunter2",
            "passwd = hunter2",
            "token: secret",
            "access_token = short-secret",
            "access-token: short-secret",
            "client_secret=short-secret",
            "client-secret: short-secret",
            "OPENAI_API_KEY=secret",
            "GITHUB_TOKEN=secret",
            "X-API-Key: secret",
            "X-Auth-Token = secret",
            "{\"api_key\":\"secret\"}",
            "{\"api key\":\"secret\"}",
            "{'access_token': 'secret'}",
            "ghp_0123456789abcdefghijklmnopqrstuvwxyz",
            "github_pat_0123456789abcdefghijklmnopqrstuvwxyz",
            "sk-0123456789abcdefghijklmnopqrstuvwxyz",
            "sk_0123456789abcdefghijklmnopqrstuvwxyz",
        ]
        .iter()
        .enumerate()
        {
            let leaked_finding = finding(
                "leak",
                FindingSource::Rule,
                FindingSeverity::P0,
                None,
                "security",
                "credential leak",
                credential,
            );
            let leaked_path = markdown_jsonl_path(&format!("leaked-{index}"));
            let leaked_report = EvalMarkdownReport {
                metadata: &metadata,
                records: &records,
                findings: &[leaked_finding],
                judge: &judge,
                product_score: &empty_product_score(),
                product_diagnoses: &[],
                limitations: &[],
            };

            let error = write_markdown_report(&leaked_path, &leaked_report)
                .expect_err("sensitive markdown must be rejected");
            assert!(error.to_string().contains("sensitive"));
            assert!(!leaked_path.with_extension("md").exists());
            assert!(!leaked_path.with_extension("md.tmp").exists());
            let _ = std::fs::remove_dir_all(leaked_path.parent().expect("parent"));
        }

        let _ = std::fs::remove_dir_all(safe_path.parent().expect("parent"));
    }

    #[test]
    fn markdown_report_credential_guard_allows_safe_metric_names() {
        let metadata = report_metadata("safe-guard");
        let records: Vec<anyhow::Result<EvalRecord>> = Vec::new();
        let judge = empty_judge(JudgeStatus::NotConfigured);

        for (index, safe_text) in [
            "input_tokens",
            "output_tokens",
            "cache_hit_tokens",
            "api key rotation is recommended",
            "the api key concept: safe",
            "api keyboard: value",
            "authorization policy is enabled",
            "basic validation",
            "bearer market",
            "Authorization: disabled",
            "Authorization: Bearer",
            "Authorization: Basic",
            "Cookie: unavailable",
            "password: redacted",
            "token: hidden",
            "token: 42",
            "input token: 50000",
            "token: null",
            "token: none",
            "token: not configured",
            "token: not_configured",
            "mytoken: ordinary-field",
            "passwordless: enabled",
            "sha256 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "model vendor/model-name-with-a-very-long-safe-identifier-20260812",
            "path C:/very/long/safe/path/with-20260812/build/artifacts/report.jsonl",
            "sk-short",
        ]
        .iter()
        .enumerate()
        {
            let path = markdown_jsonl_path(&format!("safe-guard-{index}"));
            let limitations = vec![safe_text.to_string()];
            let report = EvalMarkdownReport {
                metadata: &metadata,
                records: &records,
                findings: &[],
                judge: &judge,
                product_score: &empty_product_score(),
                product_diagnoses: &[],
                limitations: &limitations,
            };
            write_markdown_report(&path, &report).expect("safe text must not be rejected");
            let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
        }
    }

    #[test]
    fn markdown_report_tool_and_performance_section_uses_analysis_categories() {
        let path = markdown_jsonl_path("tool-categories");
        let metadata = report_metadata("tool-categories");
        let records: Vec<anyhow::Result<EvalRecord>> = Vec::new();
        let judge = empty_judge(JudgeStatus::NotConfigured);
        let relevant = [
            ("tool_event_failed", "execution"),
            ("unexpected_tool_use", "tool_use"),
            ("required_tool_missing", "tool_use"),
            ("repeated_tool_use", "efficiency"),
            ("slow_high_token", "efficiency"),
            ("low_cache_hit_ratio", "cache"),
            ("latency_outlier", "latency"),
        ];
        let mut findings = relevant
            .iter()
            .enumerate()
            .map(|(index, (id, category))| {
                finding(
                    id,
                    FindingSource::Rule,
                    FindingSeverity::P2,
                    None,
                    category,
                    &format!("category title {index}"),
                    "safe evidence",
                )
            })
            .collect::<Vec<_>>();
        findings.push(finding(
            "case_failed",
            FindingSource::Rule,
            FindingSeverity::P0,
            None,
            "execution",
            "generic case failure sentinel",
            "safe evidence",
        ));
        let report = EvalMarkdownReport {
            metadata: &metadata,
            records: &records,
            findings: &findings,
            judge: &judge,
            product_score: &empty_product_score(),
            product_diagnoses: &[],
            limitations: &[],
        };

        let outcome = write_markdown_report(&path, &report).expect("write category report");
        let start = outcome
            .markdown
            .find("## 工具与性能观察")
            .expect("tool section");
        let end = outcome
            .markdown
            .find("## 确定性规则发现")
            .expect("next section");
        let section = &outcome.markdown[start..end];
        assert!(!section.contains("- 无。"));
        for index in 0..relevant.len() {
            assert!(section.contains(&format!("category title {index}")));
        }
        assert!(!section.contains("generic case failure sentinel"));
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn markdown_report_aggregates_u64_max_values_without_overflow() {
        let path = markdown_jsonl_path("max-metrics");
        let metadata = report_metadata("max-metrics");
        let mut first = completed_record("max-a");
        first.elapsed_ms = u64::MAX;
        first.usage = Some(TurnUsage {
            input_tokens: u64::MAX,
            output_tokens: u64::MAX,
            cache_hit_tokens: u64::MAX,
            cache_miss_tokens: u64::MAX,
            ..Default::default()
        });
        let mut second = completed_record("max-b");
        second.elapsed_ms = u64::MAX;
        second.usage = first.usage;
        let records = vec![Ok(first), Ok(second)];
        let judge = empty_judge(JudgeStatus::NotConfigured);
        let report = EvalMarkdownReport {
            metadata: &metadata,
            records: &records,
            findings: &[],
            judge: &judge,
            product_score: &empty_product_score(),
            product_diagnoses: &[],
            limitations: &[],
        };

        let outcome = write_markdown_report(&path, &report).expect("max metrics do not overflow");
        assert!(outcome.markdown.contains("汇总 36893488147419103230 ms"));
        assert!(outcome.markdown.contains("中位数 18446744073709551615 ms"));
        assert!(outcome.markdown.contains("输入 36893488147419103230"));
        assert!(outcome.markdown.contains("输出 36893488147419103230"));
        assert!(outcome.markdown.contains("命中 36893488147419103230"));
        assert!(outcome.markdown.contains("未命中 36893488147419103230"));
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn markdown_report_refuses_to_overwrite_existing_final_file() {
        let jsonl_path = markdown_jsonl_path("existing");
        let final_path = jsonl_path.with_extension("md");
        std::fs::write(&final_path, "existing report").expect("seed final report");
        let metadata = report_metadata("existing");
        let records: Vec<anyhow::Result<EvalRecord>> = Vec::new();
        let judge = empty_judge(JudgeStatus::NotConfigured);
        let report = EvalMarkdownReport {
            metadata: &metadata,
            records: &records,
            findings: &[],
            judge: &judge,
            product_score: &empty_product_score(),
            product_diagnoses: &[],
            limitations: &[],
        };

        let error = write_markdown_report(&jsonl_path, &report)
            .expect_err("existing final file must not be overwritten");
        assert!(error.to_string().contains("already exists"));
        assert_eq!(
            std::fs::read_to_string(&final_path).expect("read existing report"),
            "existing report"
        );
        assert!(!jsonl_path.with_extension("md.tmp").exists());
        let _ = std::fs::remove_dir_all(jsonl_path.parent().expect("parent"));
    }

    #[test]
    fn markdown_report_does_not_delete_a_preexisting_temporary_file() {
        let jsonl_path = markdown_jsonl_path("existing-tmp");
        let temporary_path = jsonl_path.with_extension("md.tmp");
        std::fs::write(&temporary_path, "other writer").expect("seed temporary report");
        let metadata = report_metadata("existing-tmp");
        let records: Vec<anyhow::Result<EvalRecord>> = Vec::new();
        let judge = empty_judge(JudgeStatus::NotConfigured);
        let report = EvalMarkdownReport {
            metadata: &metadata,
            records: &records,
            findings: &[],
            judge: &judge,
            product_score: &empty_product_score(),
            product_diagnoses: &[],
            limitations: &[],
        };

        write_markdown_report(&jsonl_path, &report)
            .expect_err("preexisting temporary file must preserve create-new semantics");
        assert_eq!(
            std::fs::read_to_string(&temporary_path).expect("read temporary report"),
            "other writer"
        );
        assert!(!jsonl_path.with_extension("md").exists());
        let _ = std::fs::remove_dir_all(jsonl_path.parent().expect("parent"));
    }

    #[test]
    fn rules_case_failure_is_single_p0_with_error_evidence() {
        let cases = vec![rule_case("broken", ToolExpectation::Optional)];
        let mut record = completed_record("broken");
        record.status = "Error".to_string();
        record.error = Some("provider exploded".to_string());

        let analysis = analyze_rules(&cases, &[Ok(record)]);

        let failures = analysis
            .findings
            .iter()
            .filter(|finding| finding.id == "case_failed")
            .collect::<Vec<_>>();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].severity, FindingSeverity::P0);
        assert_eq!(failures[0].category, "execution");
        assert!(failures[0].evidence.contains("provider failed"));
        assert!(!failures[0].evidence.contains("provider exploded"));
    }

    #[test]
    fn rules_timeout_is_single_case_failure() {
        let cases = vec![rule_case("timeout", ToolExpectation::Optional)];
        let mut record = completed_record("timeout");
        record.status = "timeout".to_string();
        record.error = Some("timeout after 60000ms".to_string());

        let analysis = analyze_rules(&cases, &[Ok(record)]);

        let failures = analysis
            .findings
            .iter()
            .filter(|finding| finding.id == "case_failed")
            .collect::<Vec<_>>();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].evidence.contains("request timed out"));
        assert!(!failures[0].evidence.contains("60000ms"));
    }

    #[test]
    fn rules_equivalent_failures_for_same_case_are_deduplicated() {
        let cases = vec![rule_case("broken", ToolExpectation::Optional)];
        let failed_record = || {
            let mut record = completed_record("broken");
            record.status = "Error".to_string();
            record.error = Some("same provider error".to_string());
            record
        };

        let analysis = analyze_rules(&cases, &[Ok(failed_record()), Ok(failed_record())]);

        assert_eq!(
            analysis
                .findings
                .iter()
                .filter(|finding| finding.id == "case_failed")
                .count(),
            1
        );
    }

    #[test]
    fn rules_runner_error_result_is_p0_and_does_not_leak_prompt() {
        let cases = vec![rule_case("runner", ToolExpectation::Optional)];
        let analysis = analyze_rules(&cases, &[Err(anyhow::anyhow!("runner unavailable"))]);

        assert_eq!(analysis.findings.len(), 1);
        assert_eq!(analysis.findings[0].id, "case_failed");
        assert_eq!(analysis.findings[0].severity, FindingSeverity::P0);
        assert!(analysis.findings[0].evidence.contains("runner failed"));
        assert!(!analysis.findings[0].evidence.contains("runner unavailable"));
        assert!(!analysis.findings[0]
            .evidence
            .contains("private prompt sentinel"));
    }

    #[test]
    fn rules_slow_high_token_case_is_p1() {
        let cases = vec![rule_case("expensive", ToolExpectation::Optional)];
        let mut record = completed_record("expensive");
        record.elapsed_ms = 30_000;
        record.usage = Some(TurnUsage {
            input_tokens: 40_000,
            ..Default::default()
        });

        let analysis = analyze_rules(&cases, &[Ok(record)]);

        assert!(analysis.findings.iter().any(|finding| {
            finding.id == "slow_high_token" && finding.severity == FindingSeverity::P1
        }));
    }

    #[test]
    fn rules_slow_high_token_requires_both_thresholds() {
        for (case_id, elapsed_ms, input_tokens) in
            [("fast", 29_999, 40_000), ("small", 30_000, 39_999)]
        {
            let cases = vec![rule_case(case_id, ToolExpectation::Optional)];
            let mut record = completed_record(case_id);
            record.elapsed_ms = elapsed_ms;
            record.usage = Some(TurnUsage {
                input_tokens,
                ..Default::default()
            });
            let analysis = analyze_rules(&cases, &[Ok(record)]);
            assert!(!analysis
                .findings
                .iter()
                .any(|finding| finding.id == "slow_high_token"));
        }
    }

    #[test]
    fn rules_tool_expectations_failed_events_cache_and_repetition() {
        let cases = vec![
            rule_case("forbidden", ToolExpectation::Forbidden),
            rule_case("required", ToolExpectation::Required),
            rule_case("tools", ToolExpectation::Optional),
        ];
        let mut forbidden = completed_record("forbidden");
        forbidden.analysis.tool_events = vec![EvalToolEvent {
            name: "shell".to_string(),
            failed: false,
        }];
        let required = completed_record("required");
        let mut tools = completed_record("tools");
        tools.usage = Some(TurnUsage {
            input_tokens: 40_000,
            cache_hit_tokens: 10,
            cache_miss_tokens: 90,
            ..Default::default()
        });
        tools.analysis.tool_events = vec![
            EvalToolEvent {
                name: "lookup".to_string(),
                failed: true,
            },
            EvalToolEvent {
                name: "lookup".to_string(),
                failed: false,
            },
            EvalToolEvent {
                name: "lookup".to_string(),
                failed: false,
            },
        ];

        let analysis = analyze_rules(&cases, &[Ok(forbidden), Ok(required), Ok(tools)]);

        for (id, severity) in [
            ("tool_event_failed", FindingSeverity::P0),
            ("unexpected_tool_use", FindingSeverity::P1),
            ("required_tool_missing", FindingSeverity::P1),
            ("low_cache_hit_ratio", FindingSeverity::P1),
            ("repeated_tool_use", FindingSeverity::P2),
        ] {
            assert!(analysis
                .findings
                .iter()
                .any(|finding| finding.id == id && finding.severity == severity));
        }
    }

    #[test]
    fn rules_latency_outlier_uses_successful_even_median_and_thresholds() {
        let cases =
            ["a", "b", "slow", "not_outlier"].map(|id| rule_case(id, ToolExpectation::Optional));
        let records = [4_000, 4_000, 12_000, 8_000]
            .into_iter()
            .zip(cases.iter())
            .map(|(elapsed, case)| {
                let mut record = completed_record(&case.case_id);
                record.elapsed_ms = elapsed;
                Ok(record)
            })
            .collect::<Vec<_>>();

        let analysis = analyze_rules(&cases, &records);

        assert!(analysis.findings.iter().any(|finding| {
            finding.id == "latency_outlier" && finding.case_id.as_deref() == Some("slow")
        }));
        assert!(!analysis.findings.iter().any(|finding| {
            finding.id == "latency_outlier" && finding.case_id.as_deref() == Some("not_outlier")
        }));
    }

    #[test]
    fn rules_latency_single_peer_and_exact_half_median_boundaries() {
        assert!(!latency_exceeds_twice_median(12_000, &[]));
        assert!(latency_exceeds_twice_median(12_000, &[4_000]));
        assert!(!latency_exceeds_twice_median(10_001, &[5_000, 5_001]));
        assert!(latency_exceeds_twice_median(10_002, &[5_000, 5_001]));

        let cases = ["peer", "outlier"].map(|id| rule_case(id, ToolExpectation::Optional));
        let records = [4_000, 12_000]
            .into_iter()
            .zip(cases.iter())
            .map(|(elapsed, case)| {
                let mut record = completed_record(&case.case_id);
                record.elapsed_ms = elapsed;
                Ok(record)
            })
            .collect::<Vec<_>>();
        let analysis = analyze_rules(&cases, &records);
        assert!(analysis.findings.iter().any(|finding| {
            finding.id == "latency_outlier" && finding.case_id.as_deref() == Some("outlier")
        }));

        for (target, should_report) in [(10_001, false), (10_002, true)] {
            let cases =
                ["low", "high", "target"].map(|id| rule_case(id, ToolExpectation::Optional));
            let records = [5_000, 5_001, target]
                .into_iter()
                .zip(cases.iter())
                .map(|(elapsed, case)| {
                    let mut record = completed_record(&case.case_id);
                    record.elapsed_ms = elapsed;
                    Ok(record)
                })
                .collect::<Vec<_>>();
            let analysis = analyze_rules(&cases, &records);
            assert_eq!(
                analysis.findings.iter().any(|finding| {
                    finding.id == "latency_outlier" && finding.case_id.as_deref() == Some("target")
                }),
                should_report
            );
        }
    }

    #[test]
    fn rules_latency_absolute_threshold_is_inclusive() {
        for (target, should_report) in [(9_999, false), (10_000, true)] {
            let cases =
                ["peer_a", "peer_b", "target"].map(|id| rule_case(id, ToolExpectation::Optional));
            let records = [4_000, 4_000, target]
                .into_iter()
                .zip(cases.iter())
                .map(|(elapsed, case)| {
                    let mut record = completed_record(&case.case_id);
                    record.elapsed_ms = elapsed;
                    Ok(record)
                })
                .collect::<Vec<_>>();
            let analysis = analyze_rules(&cases, &records);
            assert_eq!(
                analysis.findings.iter().any(|finding| {
                    finding.id == "latency_outlier" && finding.case_id.as_deref() == Some("target")
                }),
                should_report
            );
        }
    }

    #[test]
    fn rules_latency_median_handles_u64_max_without_overflow() {
        assert!(!latency_exceeds_twice_median(
            u64::MAX,
            &[u64::MAX - 2, u64::MAX]
        ));

        let cases = ["a", "b", "slow"].map(|id| rule_case(id, ToolExpectation::Optional));
        let records = [u64::MAX - 2, u64::MAX, 10_000]
            .into_iter()
            .zip(cases.iter())
            .map(|(elapsed, case)| {
                let mut record = completed_record(&case.case_id);
                record.elapsed_ms = elapsed;
                Ok(record)
            })
            .collect::<Vec<_>>();

        let analysis = analyze_rules(&cases, &records);

        assert!(!analysis.findings.iter().any(|finding| {
            finding.id == "latency_outlier" && finding.case_id.as_deref() == Some("slow")
        }));
    }

    #[test]
    fn rules_latency_outlier_is_safe_for_empty_and_single_success() {
        let empty = analyze_rules(&[], &[]);
        assert!(empty.findings.is_empty());

        let cases = vec![rule_case("only", ToolExpectation::Optional)];
        let mut only = completed_record("only");
        only.elapsed_ms = 12_000;
        let single = analyze_rules(&cases, &[Ok(only)]);
        assert!(!single
            .findings
            .iter()
            .any(|finding| finding.id == "latency_outlier"));
    }

    #[test]
    fn rules_cache_zero_denominator_and_exact_quarter_do_not_report() {
        let cases = ["zero", "quarter"].map(|id| rule_case(id, ToolExpectation::Optional));
        let records = [(0, 0), (25, 75)]
            .into_iter()
            .zip(cases.iter())
            .map(|((hit, miss), case)| {
                let mut record = completed_record(&case.case_id);
                record.usage = Some(TurnUsage {
                    input_tokens: 40_000,
                    cache_hit_tokens: hit,
                    cache_miss_tokens: miss,
                    ..Default::default()
                });
                Ok(record)
            })
            .collect::<Vec<_>>();

        let analysis = analyze_rules(&cases, &records);

        assert!(!analysis
            .findings
            .iter()
            .any(|finding| finding.id == "low_cache_hit_ratio"));
    }

    #[test]
    fn rules_cache_ratio_uses_overflow_safe_exact_arithmetic() {
        assert!(is_low_cache_hit_ratio(u64::MAX / 4 + 1, u64::MAX - 1));

        let cases = vec![rule_case("large_cache", ToolExpectation::Optional)];
        let mut record = completed_record("large_cache");
        record.usage = Some(TurnUsage {
            input_tokens: 40_000,
            cache_hit_tokens: u64::MAX / 4 + 1,
            cache_miss_tokens: u64::MAX - 1,
            ..Default::default()
        });

        let analysis = analyze_rules(&cases, &[Ok(record)]);

        assert!(analysis.findings.iter().any(|finding| {
            finding.id == "low_cache_hit_ratio" && finding.severity == FindingSeverity::P1
        }));
    }

    #[test]
    fn rules_all_findings_are_actionable_and_do_not_leak_answer() {
        let cases = vec![rule_case("leak", ToolExpectation::Forbidden)];
        let mut record = completed_record("leak");
        record.status = "Error".to_string();
        record.error = Some("provider failed".to_string());
        record.elapsed_ms = 30_000;
        record.usage = Some(TurnUsage {
            input_tokens: 40_000,
            cache_hit_tokens: 1,
            cache_miss_tokens: 99,
            ..Default::default()
        });
        record.analysis.assistant_text = "private answer sentinel".to_string();
        record.analysis.tool_events = vec![
            EvalToolEvent {
                name: "lookup".to_string(),
                failed: true,
            },
            EvalToolEvent {
                name: "lookup".to_string(),
                failed: false,
            },
            EvalToolEvent {
                name: "lookup".to_string(),
                failed: false,
            },
        ];

        let analysis = analyze_rules(&cases, &[Ok(record)]);

        assert!(!analysis.findings.is_empty());
        assert!(analysis.findings.iter().all(|finding| {
            !finding.evidence.is_empty()
                && !finding.impact.is_empty()
                && !finding.recommendation.is_empty()
                && !finding.evidence.contains("private answer sentinel")
                && !finding.impact.contains("private answer sentinel")
                && !finding.recommendation.contains("private answer sentinel")
                && finding.evidence.chars().count() <= 300
        }));
        assert!(!serde_json::to_string(&analysis.findings)
            .expect("serialize findings")
            .contains("private answer sentinel"));
    }

    #[test]
    fn rules_failure_evidence_is_fail_closed_for_all_error_text() {
        let prompt = "private prompt sentinel";
        let answer = "private answer sentinel";
        let raw_errors = vec![
            (format!("unknown failure containing {prompt} and {answer}"), "error details redacted"),
            (r#"unknown {\"message\":\"private prompt sentinel\",\"answer\":\"private answer sentinel\"}"#.to_string(), "error details redacted"),
            ("unknown Authorization: Bearer abcdefgh".to_string(), "authentication or permission failed"),
            ("unknown Authorization:Bearer abcdefgh".to_string(), "authentication or permission failed"),
            ("unknown authorization = Bearer abcdefgh".to_string(), "authentication or permission failed"),
            ("unknown api_key=secret-key".to_string(), "error details redacted"),
            ("unknown api_key = secret-key".to_string(), "error details redacted"),
            ("unknown x-api-key: secret-key".to_string(), "error details redacted"),
            ("unknown Cookie: sid=secret-cookie".to_string(), "error details redacted"),
            ("unknown 0123456789abcdef0123456789abcdef".to_string(), "error details redacted"),
            (format!("unknown {}", "x".repeat(600)), "error details redacted"),
        ];

        for (index, (raw_error, expected_category)) in raw_errors.into_iter().enumerate() {
            let case_id = format!("private_error_{index}");
            let cases = vec![rule_case(&case_id, ToolExpectation::Optional)];
            let mut record = completed_record(&case_id);
            record.status = "Error".to_string();
            record.analysis.user_message = prompt.to_string();
            record.analysis.assistant_text = answer.to_string();
            record.error = Some(raw_error.clone());

            let analysis = analyze_rules(&cases, &[Ok(record)]);
            let evidence = &analysis
                .findings
                .iter()
                .find(|finding| finding.id == "case_failed")
                .expect("failure finding")
                .evidence;

            assert_eq!(classify_error(&raw_error), expected_category);
            assert!(evidence.contains(expected_category));
            assert!(!evidence.contains(&raw_error));
            assert!(!evidence.contains(prompt));
            assert!(!evidence.contains(answer));
            assert!(!evidence.contains("abcdefgh"));
            assert!(!evidence.contains("secret-key"));
            assert!(!evidence.contains("secret-cookie"));
            assert!(!evidence.contains("0123456789abcdef0123456789abcdef"));
            assert!(evidence.chars().count() <= 300);
        }
    }

    #[test]
    fn rules_error_classification_uses_only_fixed_safe_categories() {
        for (error, category) in [
            ("request timed out after 5s", "request timed out"),
            (
                "HTTP 401 unauthorized",
                "authentication or permission failed",
            ),
            ("HTTP 403 forbidden", "authentication or permission failed"),
            ("rate limit HTTP 429", "rate limited"),
            ("provider connection reset", "provider failed"),
            ("runner unavailable", "runner failed"),
            ("basic validation failed", "error details redacted"),
            ("basically unavailable", "error details redacted"),
            ("unexpected socket close", "error details redacted"),
            (
                "unknown dump Authorization: Bearer secret timeout forbidden 401",
                "error details redacted",
            ),
            (
                "the prompt says timeout forbidden 401",
                "error details redacted",
            ),
            ("upstream returned status 4010", "error details redacted"),
            ("upstream returned status 4031", "error details redacted"),
            ("upstream returned status 4290", "error details redacted"),
        ] {
            assert_eq!(classify_error(error), category);
        }

        for (error, code) in [
            ("HTTP 401 unauthorized", "401"),
            ("upstream status 403: forbidden", "403"),
            ("upstream status 429", "429"),
        ] {
            assert!(has_http_status(error, code));
        }
        for (error, code) in [
            ("upstream status 4010", "401"),
            ("upstream status 4031", "403"),
            ("upstream status 4290", "429"),
            ("upstream http401", "401"),
        ] {
            assert!(!has_http_status(error, code));
        }
    }

    #[test]
    fn rules_tool_labels_use_an_exact_canonical_allowlist() {
        for canonical in [
            "web_search",
            "fetch_url",
            "exec_shell",
            "read_file",
            "write_file",
            "append_file",
            "edit_file",
            "mcp_pinvou3_present_artifact",
            "kb_search",
            "kb_open_source",
        ] {
            assert_eq!(canonical_tool_label(canonical), Some(canonical));
        }

        let unknown_names = [
            "sk-proj-secret".to_string(),
            "api_key_secret".to_string(),
            "private-prompt-sentinel".to_string(),
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".to_string(),
        ];
        for (index, unknown_name) in unknown_names.into_iter().enumerate() {
            assert_eq!(canonical_tool_label(&unknown_name), None);
            let case_id = format!("unsafe_tool_{index}");
            let cases = vec![rule_case(&case_id, ToolExpectation::Optional)];
            let mut record = completed_record(&case_id);
            record.analysis.tool_events = (0..3)
                .map(|iteration| EvalToolEvent {
                    name: unknown_name.clone(),
                    failed: iteration == 0,
                })
                .collect();
            let analysis = analyze_rules(&cases, &[Ok(record)]);
            let tool_findings = analysis
                .findings
                .iter()
                .filter(|finding| {
                    finding.id == "tool_event_failed" || finding.id == "repeated_tool_use"
                })
                .collect::<Vec<_>>();
            assert_eq!(tool_findings.len(), 2);
            assert!(tool_findings.iter().all(|finding| {
                finding.evidence.contains("[redacted-tool]")
                    && !finding.evidence.contains(&unknown_name)
                    && finding.evidence.chars().count() <= 300
            }));
        }
    }

    #[test]
    fn rules_canonical_tool_names_survive_in_findings() {
        for safe_name in ["web_search", "mcp_pinvou3_present_artifact"] {
            let cases = vec![rule_case(safe_name, ToolExpectation::Optional)];
            let mut record = completed_record(safe_name);
            record.analysis.tool_events = vec![EvalToolEvent {
                name: safe_name.to_string(),
                failed: true,
            }];
            let analysis = analyze_rules(&cases, &[Ok(record)]);
            assert!(analysis
                .findings
                .iter()
                .any(|finding| finding.evidence.contains(safe_name)));
        }
    }

    #[test]
    fn rules_unknown_tool_identity_is_counted_before_redaction() {
        let cases =
            ["distinct", "same", "two_repeated"].map(|id| rule_case(id, ToolExpectation::Optional));
        let mut distinct = completed_record("distinct");
        distinct.analysis.tool_events = ["unknown-one", "unknown-two", "unknown-three"]
            .map(|name| EvalToolEvent {
                name: name.to_string(),
                failed: false,
            })
            .to_vec();
        let mut same = completed_record("same");
        same.analysis.tool_events = (0..3)
            .map(|_| EvalToolEvent {
                name: "private-tool".to_string(),
                failed: false,
            })
            .collect();
        let mut two_repeated = completed_record("two_repeated");
        two_repeated.analysis.tool_events = ["private-a", "private-b"]
            .into_iter()
            .flat_map(|name| {
                (0..3).map(move |_| EvalToolEvent {
                    name: name.to_string(),
                    failed: false,
                })
            })
            .collect();

        let analysis = analyze_rules(&cases, &[Ok(distinct), Ok(same), Ok(two_repeated)]);
        let repetitions = analysis
            .findings
            .iter()
            .filter(|finding| finding.id == "repeated_tool_use")
            .collect::<Vec<_>>();
        assert_eq!(repetitions.len(), 2);
        assert!(repetitions
            .iter()
            .all(|finding| finding.evidence.contains("[redacted-tool]")));
        assert!(!repetitions.iter().any(|finding| {
            [
                "unknown-one",
                "unknown-two",
                "unknown-three",
                "private-tool",
                "private-a",
                "private-b",
            ]
            .iter()
            .any(|name| finding.evidence.contains(name))
        }));
        assert_eq!(
            repetitions
                .iter()
                .filter(|finding| finding.case_id.as_deref() == Some("same"))
                .count(),
            1
        );
        assert_eq!(
            repetitions
                .iter()
                .filter(|finding| finding.case_id.as_deref() == Some("two_repeated"))
                .count(),
            1
        );
    }

    #[test]
    fn rules_unknown_status_is_not_persisted() {
        let status = format!("private-status-{}", "x".repeat(600));
        let cases = vec![rule_case("status", ToolExpectation::Optional)];
        let mut record = completed_record("status");
        record.status = status.clone();

        let analysis = analyze_rules(&cases, &[Ok(record)]);
        let failure = analysis
            .findings
            .iter()
            .find(|finding| finding.id == "case_failed")
            .expect("failure finding");
        assert!(failure.evidence.contains("non-completed"));
        assert!(!failure.evidence.contains(&status));
        assert!(failure.evidence.chars().count() <= 300);
    }

    #[test]
    fn rules_finding_safety_limits_every_diagnostic_text_field() {
        let long = "界".repeat(400);
        let mut finding = finding(
            "long",
            FindingSource::Judge,
            FindingSeverity::P2,
            Some("case"),
            "quality",
            &long,
            &long,
        );
        finding.impact = long.clone();
        finding.recommendation = long;

        enforce_finding_safety(&mut finding);

        for value in [
            &finding.title,
            &finding.evidence,
            &finding.impact,
            &finding.recommendation,
        ] {
            assert_eq!(value.chars().count(), 300);
            assert!(value.ends_with('…'));
        }

        let merged = merge_findings(
            RuleAnalysis {
                findings: Vec::new(),
                limitations: Vec::new(),
            },
            vec![finding],
        );
        assert!(merged[0].title.chars().count() <= 300);
        assert!(merged[0].evidence.chars().count() <= 300);
        assert!(merged[0].impact.chars().count() <= 300);
        assert!(merged[0].recommendation.chars().count() <= 300);
    }

    #[test]
    fn rules_repeat_threshold_and_tool_name_order_are_deterministic() {
        let cases = ["twice", "multiple"].map(|id| rule_case(id, ToolExpectation::Optional));
        let mut twice = completed_record("twice");
        twice.analysis.tool_events = (0..2)
            .map(|_| EvalToolEvent {
                name: "twice".to_string(),
                failed: false,
            })
            .collect();
        let mut multiple = completed_record("multiple");
        multiple.analysis.tool_events = [
            "web_search",
            "fetch_url",
            "web_search",
            "fetch_url",
            "web_search",
            "fetch_url",
        ]
        .map(|name| EvalToolEvent {
            name: name.to_string(),
            failed: false,
        })
        .to_vec();

        let analysis = analyze_rules(&cases, &[Ok(twice), Ok(multiple)]);
        let repetitions = analysis
            .findings
            .iter()
            .filter(|finding| finding.id == "repeated_tool_use")
            .collect::<Vec<_>>();

        assert_eq!(repetitions.len(), 2);
        assert!(repetitions[0].evidence.contains("`fetch_url`"));
        assert!(repetitions[1].evidence.contains("`web_search`"));
        assert!(repetitions
            .iter()
            .all(|finding| finding.case_id.as_deref() == Some("multiple")));
    }

    #[test]
    fn rules_healthy_five_case_batch_only_reports_sample_limitation() {
        let cases = (0..5)
            .map(|index| rule_case(&format!("healthy_{index}"), ToolExpectation::Optional))
            .collect::<Vec<_>>();
        let records = cases
            .iter()
            .map(|case| Ok(completed_record(&case.case_id)))
            .collect::<Vec<_>>();

        let analysis = analyze_rules(&cases, &records);

        assert!(analysis.findings.is_empty());
        assert_eq!(analysis.limitations.len(), 1);
        assert!(analysis.limitations[0].contains("trend"));
    }

    #[test]
    fn rules_findings_are_deterministically_sorted() {
        let cases = vec![
            rule_case("z", ToolExpectation::Forbidden),
            rule_case("a", ToolExpectation::Required),
        ];
        let mut z = completed_record("z");
        z.analysis.tool_events.push(EvalToolEvent {
            name: "shell".to_string(),
            failed: true,
        });
        let analysis = analyze_rules(&cases, &[Ok(z), Ok(completed_record("a"))]);
        let keys = analysis
            .findings
            .iter()
            .map(|finding| {
                (
                    finding.severity,
                    finding.case_id.clone(),
                    finding.id.clone(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            keys,
            vec![
                (
                    FindingSeverity::P0,
                    Some("z".to_string()),
                    "tool_event_failed".to_string(),
                ),
                (
                    FindingSeverity::P1,
                    Some("a".to_string()),
                    "required_tool_missing".to_string(),
                ),
                (
                    FindingSeverity::P1,
                    Some("z".to_string()),
                    "unexpected_tool_use".to_string(),
                ),
            ]
        );
    }

    #[test]
    fn rules_merge_deduplicates_exact_evidence_but_preserves_conflicts() {
        let rule = finding(
            "rule_id",
            FindingSource::Rule,
            FindingSeverity::P1,
            Some("case_a"),
            "quality",
            "  Repeated   tool use ",
            "three calls",
        );
        let duplicate = finding(
            "judge_duplicate",
            FindingSource::Judge,
            FindingSeverity::P2,
            Some("case_a"),
            "quality",
            "repeated tool USE",
            "three calls",
        );
        let conflicting = finding(
            "judge_conflict",
            FindingSource::Judge,
            FindingSeverity::P0,
            Some("case_a"),
            "quality",
            "Repeated tool use",
            "five calls",
        );

        let merged = merge_findings(
            RuleAnalysis {
                findings: vec![rule],
                limitations: Vec::new(),
            },
            vec![duplicate, conflicting],
        );

        assert_eq!(merged.len(), 2);
        assert!(merged
            .iter()
            .any(|finding| finding.source == FindingSource::Rule));
        assert!(merged
            .iter()
            .any(|finding| finding.evidence == "five calls"));
        assert_eq!(merged[0].severity, FindingSeverity::P0);
    }

    #[test]
    fn rules_merge_deduplicates_rule_findings_before_judge_merge() {
        let duplicate = finding(
            "rule_duplicate",
            FindingSource::Rule,
            FindingSeverity::P2,
            Some("case_a"),
            "quality",
            "repeated tool use",
            "three calls",
        );
        let merged = merge_findings(
            RuleAnalysis {
                findings: vec![duplicate.clone(), duplicate],
                limitations: Vec::new(),
            },
            Vec::new(),
        );

        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn rules_merge_applies_safety_before_deduplication_and_keeps_rule_priority() {
        let shared_prefix = "x".repeat(300);
        let rule = finding(
            "rule",
            FindingSource::Rule,
            FindingSeverity::P1,
            Some("case_a"),
            "quality",
            "same title",
            &format!("{shared_prefix}a"),
        );
        let judge = finding(
            "judge",
            FindingSource::Judge,
            FindingSeverity::P2,
            Some("case_a"),
            "quality",
            "same title",
            &format!("{shared_prefix}b"),
        );
        let merged = merge_findings(
            RuleAnalysis {
                findings: vec![rule],
                limitations: Vec::new(),
            },
            vec![judge],
        );

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].source, FindingSource::Rule);
        assert_eq!(merged[0].evidence.chars().count(), 300);

        let duplicate_judges = merge_findings(
            RuleAnalysis {
                findings: Vec::new(),
                limitations: Vec::new(),
            },
            vec![
                finding(
                    "first",
                    FindingSource::Judge,
                    FindingSeverity::P1,
                    Some("case_a"),
                    "quality",
                    "same title",
                    &format!("{shared_prefix}a"),
                ),
                finding(
                    "second",
                    FindingSource::Judge,
                    FindingSeverity::P1,
                    Some("case_a"),
                    "quality",
                    "same title",
                    &format!("{shared_prefix}b"),
                ),
            ],
        );
        assert_eq!(duplicate_judges.len(), 1);
    }

    #[test]
    fn rules_sort_preserves_input_order_when_required_keys_match() {
        let first = finding(
            "same_id",
            FindingSource::Rule,
            FindingSeverity::P1,
            Some("case_a"),
            "quality",
            "same title",
            "z evidence first",
        );
        let second = finding(
            "same_id",
            FindingSource::Judge,
            FindingSeverity::P1,
            Some("case_a"),
            "quality",
            "different title",
            "a evidence second",
        );
        let mut findings = vec![first, second];

        sort_findings(&mut findings);

        assert_eq!(findings[0].evidence, "z evidence first");
        assert_eq!(findings[1].evidence, "a evidence second");
    }

    #[test]
    fn eval_record_serialization_omits_analysis_material() {
        let mut record = completed_record("private_analysis");
        record.analysis = EvalAnalysisMaterial {
            user_message: "secret prompt".to_string(),
            assistant_text: "secret answer".to_string(),
            tool_events: vec![EvalToolEvent {
                name: "secret tool event".to_string(),
                failed: true,
            }],
        };

        let json = serde_json::to_string(&record).expect("serialize record");

        assert!(!json.contains("secret prompt"));
        assert!(!json.contains("secret answer"));
        assert!(!json.contains("secret tool event"));
        assert!(!json.contains("tool_events"));
    }

    #[tokio::test]
    async fn mock_smoke_immediate() {
        let mock = MockRuntime::immediate();
        let runner = PinvouChatRunner::new(mock);
        let case = EvalCase {
            case_id: "smoke_immediate".to_string(),
            user_message: "hi".to_string(),
            mode: AppMode::Yolo,
            restrict_tools: false,
            timeout_ms: 5_000,
            tool_expectation: ToolExpectation::Optional,
        };
        let record = runner.run_case(&case).await.expect("run_case failed");
        assert_eq!(record.case_id, "smoke_immediate");
        assert_eq!(record.status, "Completed");
        assert!(record.usage.is_some());
        assert!(record.error.is_none());
    }

    #[tokio::test]
    async fn mock_analysis_material_reaches_record_without_serializing() {
        let mock = MockRuntime::new(MockConfig {
            assistant_text: "private mock answer sentinel".to_string(),
            tool_events: vec![RuntimeToolEvent {
                name: "private mock tool sentinel".to_string(),
                failed: true,
            }],
            ..Default::default()
        });
        let runner = PinvouChatRunner::new(mock);
        let case = EvalCase::smoke("analysis_chain", "private mock prompt sentinel");

        let record = runner.run_case(&case).await.expect("run_case failed");

        assert_eq!(record.analysis.user_message, "private mock prompt sentinel");
        assert_eq!(
            record.analysis.assistant_text,
            "private mock answer sentinel"
        );
        assert_eq!(
            record.analysis.tool_events,
            vec![EvalToolEvent {
                name: "private mock tool sentinel".to_string(),
                failed: true,
            }]
        );
        let json = serde_json::to_string(&record).expect("serialize record");
        assert!(!json.contains("private mock prompt sentinel"));
        assert!(!json.contains("private mock answer sentinel"));
        assert!(!json.contains("private mock tool sentinel"));
    }

    #[tokio::test]
    async fn mock_smoke_delayed() {
        let mock = MockRuntime::new(MockConfig {
            delay_ms: 100,
            ..Default::default()
        });
        let runner = PinvouChatRunner::new(mock);
        let case = EvalCase {
            case_id: "smoke_delayed".to_string(),
            user_message: "hello".to_string(),
            mode: AppMode::Yolo,
            restrict_tools: false,
            timeout_ms: 5_000,
            tool_expectation: ToolExpectation::Optional,
        };
        let record = runner.run_case(&case).await.expect("run_case failed");
        assert_eq!(record.status, "Completed");
        assert!(record.elapsed_ms >= 100);
    }

    #[tokio::test]
    async fn mock_smoke_timeout() {
        let mock = MockRuntime::new(MockConfig {
            delay_ms: 10_000,
            ..Default::default()
        });
        let runner = PinvouChatRunner::new(mock);
        let case = EvalCase {
            case_id: "smoke_timeout".to_string(),
            user_message: "slow".to_string(),
            mode: AppMode::Yolo,
            restrict_tools: false,
            timeout_ms: 200,
            tool_expectation: ToolExpectation::Optional,
        };
        let record = runner.run_case(&case).await.expect("run_case failed");
        assert_eq!(record.status, "timeout");
        assert!(record.error.is_some());
    }

    #[tokio::test]
    async fn mock_smoke_error() {
        let mock = MockRuntime::new(MockConfig {
            delay_ms: 0,
            status: "Error".to_string(),
            error: Some("mock provider crashed".to_string()),
            usage: None,
            ..Default::default()
        });
        let runner = PinvouChatRunner::new(mock);
        let case = EvalCase::smoke("smoke_error", "oops");
        let record = runner.run_case(&case).await.expect("run_case failed");
        assert_eq!(record.status, "Error");
        assert_eq!(record.error.as_deref(), Some("mock provider crashed"));
        assert!(record.usage.is_none());
    }

    #[tokio::test]
    async fn suite_preserves_case_order_and_reports_success() {
        let cases = vec![
            EvalCase::smoke("suite_order_a", "first"),
            EvalCase::smoke("suite_order_b", "second"),
        ];
        let mut observed = Vec::new();

        let suite = run_eval_suite(MockRuntime::immediate(), &cases, |_, result| {
            observed.push(result.as_ref().expect("case succeeds").case_id.clone());
            Ok(())
        })
        .await
        .expect("suite succeeds");

        assert_eq!(observed, ["suite_order_a", "suite_order_b"]);
        assert_eq!(suite.records.len(), 2);
        assert!(suite.all_succeeded());

        clean("eval_suite_order_a");
        clean("eval_suite_order_b");
    }

    #[tokio::test]
    async fn suite_model_factory_gives_each_case_a_fresh_selection_from_same_snapshot() {
        let runtime = MockRuntime::immediate();
        let observer = runtime.clone();
        let cases = vec![
            EvalCase::smoke("suite_model_a", "first"),
            EvalCase::smoke("suite_model_b", "second"),
        ];
        let mut sequence = 0_u64;

        run_eval_suite_with_model_factory(
            runtime,
            &cases,
            |_| {
                sequence += 1;
                Ok(Some(
                    crate::features::assistant::eval::analysis::EvalModelSelection::new(
                        format!("case-token-{sequence}"),
                        Some("tested-a".to_string()),
                        crate::features::assistant::eval::analysis::ModelIdentity::new(
                            "provider-a",
                            "wire-a",
                        ),
                    ),
                ))
            },
            |_, _| Ok(()),
        )
        .await
        .expect("run pinned suite");

        assert_eq!(
            observer.prepared_model_ids(),
            vec![Some("tested-a".to_string()), Some("tested-a".to_string())]
        );
    }

    #[tokio::test]
    async fn suite_keeps_running_after_a_case_failure() {
        let runtime = MockRuntime::new(MockConfig {
            status: "Error".to_string(),
            error: Some("provider failed".to_string()),
            usage: None,
            ..Default::default()
        });
        let cases = vec![
            EvalCase::smoke("suite_error_a", "first"),
            EvalCase::smoke("suite_error_b", "second"),
        ];
        let mut observed = Vec::new();

        let suite = run_eval_suite(runtime, &cases, |_, result| {
            observed.push(result.as_ref().expect("case is recorded").case_id.clone());
            Ok(())
        })
        .await
        .expect("suite continues");

        assert_eq!(observed, ["suite_error_a", "suite_error_b"]);
        assert_eq!(suite.records.len(), 2);
        assert!(!suite.all_succeeded());

        clean("eval_suite_error_a");
        clean("eval_suite_error_b");
    }

    #[test]
    fn jsonl_report_is_incremental_and_atomically_finalized() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os("PINVOU3_HOME");
        let home = std::env::temp_dir().join(format!(
            "pinvou-eval-report-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::env::set_var("PINVOU3_HOME", &home);

        let mut writer = EvalReportWriter::create(report_metadata("run/report:1"))
            .expect("create report writer");
        let temporary_path = writer.temporary_path().to_path_buf();
        assert!(temporary_path.is_file());

        writer
            .append("case_a", &Ok(completed_record("case_a")))
            .expect("append successful case");
        let mut failed_record = completed_record("case_b");
        failed_record.status = "Error".to_string();
        failed_record.error = Some("private prompt sk-1234567890abcdef".to_string());
        writer
            .append("case_b", &Ok(failed_record))
            .expect("append failed record");
        let case_error = anyhow::anyhow!("Bearer private-case-error-token");
        writer
            .append("case_c", &Err(case_error))
            .expect("append failed case");
        let final_path = writer
            .finish(
                false,
                Some("failed"),
                Some(73),
                Some(PRODUCT_SCORE_VERSION),
                None,
            )
            .expect("finalize report");

        assert!(!temporary_path.exists());
        assert!(final_path.is_file());
        let lines = std::fs::read_to_string(&final_path)
            .expect("read report")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("valid json line"))
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0]["type"], "run");
        assert_eq!(lines[1]["type"], "case");
        assert_eq!(lines[2]["type"], "case");
        assert_eq!(lines[3]["type"], "case_error");
        assert_eq!(lines[3]["case_id"], "case_c");
        assert_eq!(lines[4]["type"], "complete");
        assert_eq!(lines[0]["metadata"]["mode"], "product");
        assert_eq!(lines[1]["record"]["case_id"], "case_a");
        assert_eq!(lines[1]["record"]["milestones"], serde_json::json!([]));
        assert!(lines[1]["record"].get("error").is_some());
        assert!(lines[1]["record"]["error"].is_null());
        assert_eq!(lines[2]["record"]["case_id"], "case_b");
        assert_eq!(lines[2]["record"]["error"], "runner_error");
        assert_eq!(lines[3]["error"], "case_execution_failed");
        assert_eq!(lines[4]["all_succeeded"], false);
        assert_eq!(lines[4]["analysis_status"], "failed");
        assert_eq!(lines[4]["product_score"], 73);
        assert_eq!(lines[4]["product_score_version"], PRODUCT_SCORE_VERSION);
        assert!(lines[4].get("markdown_report").is_none());
        let persisted = std::fs::read_to_string(&final_path).expect("read persisted JSONL");
        assert!(!persisted.contains("private prompt"));
        assert!(!persisted.contains("sk-1234567890abcdef"));
        assert!(!persisted.contains("private-case-error-token"));

        let _ = std::fs::remove_dir_all(&home);
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }

    #[test]
    fn eval_product_score_wiring_empty_jsonl_complete_omits_optional_score_fields() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os("PINVOU3_HOME");
        let home = std::env::temp_dir().join(format!(
            "pinvou-eval-empty-score-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::env::set_var("PINVOU3_HOME", &home);

        let writer =
            EvalReportWriter::create(report_metadata("empty-score")).expect("create report writer");
        let final_path = writer
            .finish(
                true,
                Some("not_configured"),
                None,
                Some(PRODUCT_SCORE_VERSION),
                None,
            )
            .expect("finalize report");
        let complete = std::fs::read_to_string(final_path)
            .expect("read report")
            .lines()
            .last()
            .map(|line| serde_json::from_str::<Value>(line).expect("valid complete line"))
            .expect("complete line");

        assert!(complete.get("product_score").is_none());
        assert!(complete.get("product_score_version").is_none());

        let _ = std::fs::remove_dir_all(&home);
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }
}
