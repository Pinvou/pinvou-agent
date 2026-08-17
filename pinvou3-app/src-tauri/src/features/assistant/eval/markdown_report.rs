//! Safe, private Markdown analysis reports for a completed evaluation run.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::analysis::{
    judge_report_is_usable, EvalFinding, FindingSeverity, FindingSource, JudgeReport, JudgeStatus,
    ProductDiagnosis, ProductGrade, ProductProblemArea, ProductScore, ProductScoreConfidence,
    ProductScoreDimension,
};
use super::report::EvalRunMetadata;
use super::EvalRecord;

const JUDGE_DIMENSIONS: [&str; 6] = [
    "task_completion",
    "correctness",
    "tool_choice",
    "efficiency",
    "safety_boundaries",
    "overall_quality",
];
const TOOL_PERFORMANCE_FINDING_IDS: [&str; 7] = [
    "tool_event_failed",
    "unexpected_tool_use",
    "required_tool_missing",
    "repeated_tool_use",
    "slow_high_token",
    "low_cache_hit_ratio",
    "latency_outlier",
];
const CREDENTIAL_KEYS: [&str; 10] = [
    "api_key",
    "apikey",
    "authorization",
    "cookie",
    "access_token",
    "auth_token",
    "client_secret",
    "password",
    "passwd",
    "token",
];

pub struct EvalMarkdownReport<'a> {
    pub metadata: &'a EvalRunMetadata,
    pub records: &'a [Result<EvalRecord>],
    pub findings: &'a [EvalFinding],
    pub judge: &'a JudgeReport,
    pub product_score: &'a ProductScore,
    pub product_diagnoses: &'a [ProductDiagnosis],
    pub limitations: &'a [String],
}

#[derive(Debug)]
pub struct MarkdownReportOutcome {
    pub path: PathBuf,
    pub markdown: String,
}

/// Render a safety-filtered Markdown report and atomically place it beside its JSONL source.
pub fn write_markdown_report(
    jsonl_path: &Path,
    report: &EvalMarkdownReport<'_>,
) -> Result<MarkdownReportOutcome> {
    let final_path = jsonl_path.with_extension("md");
    let temporary_path = jsonl_path.with_extension("md.tmp");
    if final_path.exists() {
        bail!("Markdown report already exists: {}", final_path.display());
    }

    let markdown = render_markdown(report);
    if contains_sensitive_content(&markdown) {
        bail!("sensitive credential pattern detected in Markdown report");
    }

    let mut temporary_created = false;
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)
            .with_context(|| format!("create Markdown report {}", temporary_path.display()))?;
        temporary_created = true;
        file.write_all(markdown.as_bytes())
            .context("write Markdown report")?;
        file.flush().context("flush Markdown report")?;
        file.sync_all().context("sync Markdown report")?;
        drop(file);

        fs::hard_link(&temporary_path, &final_path).with_context(|| {
            format!(
                "publish Markdown report {} -> {} without overwrite",
                temporary_path.display(),
                final_path.display()
            )
        })?;
        // Publication has already succeeded. Temporary-file deletion is best-effort, so a
        // cleanup failure may leave `.tmp` beside the durable final report.
        let _ = fs::remove_file(&temporary_path);
        Ok(())
    })();

    if result.is_err() && temporary_created {
        let _ = fs::remove_file(&temporary_path);
    }
    result?;

    Ok(MarkdownReportOutcome {
        path: final_path,
        markdown,
    })
}

fn render_markdown(report: &EvalMarkdownReport<'_>) -> String {
    let judge_usable = judge_report_is_usable(report.judge);
    let total = report.records.len();
    let completed = report
        .records
        .iter()
        .filter(|entry| {
            entry
                .as_ref()
                .is_ok_and(|record| record.status.eq_ignore_ascii_case("completed"))
        })
        .count();
    let failed = total.saturating_sub(completed);
    let elapsed = report
        .records
        .iter()
        .filter_map(|entry| entry.as_ref().ok().map(|record| record.elapsed_ms))
        .collect::<Vec<_>>();
    let elapsed_total = elapsed.iter().copied().map(u128::from).sum::<u128>();
    let elapsed_median = median(&elapsed);
    let usages = report
        .records
        .iter()
        .filter_map(|entry| entry.as_ref().ok()?.usage)
        .collect::<Vec<_>>();

    let mut out = String::new();
    out.push_str("# Pinvou 私有评测分析报告\n\n");
    out.push_str("## 运行结论\n\n");
    out.push_str(&format!(
        "运行 {} 共执行 {} 个用例，完成 {} 个，未完成 {} 个。模型：{}。\n\n",
        markdown_text(&report.metadata.run_id),
        total,
        completed,
        failed,
        markdown_text(&report.metadata.model)
    ));

    out.push_str("## 产品问题与改进方向\n\n");
    render_product_diagnoses(
        &mut out,
        report.product_diagnoses,
        report.findings,
        judge_usable,
    );

    out.push_str("## 产品健康评分\n\n");
    render_product_score(&mut out, report.product_score, report.judge);

    out.push_str("## 关键指标\n\n");
    out.push_str("| 指标 | 值 |\n|---|---|\n");
    out.push_str(&format!("| 用例总数 | {total} |\n"));
    out.push_str(&format!("| Completed | {completed} |\n"));
    out.push_str(&format!("| Failed | {failed} |\n"));
    if elapsed.is_empty() {
        out.push_str("| 耗时统计 | 不可用 |\n");
    } else {
        out.push_str(&format!(
            "| 耗时统计 | 汇总 {} ms；中位数 {} ms |\n",
            elapsed_total,
            elapsed_median.unwrap_or_default()
        ));
    }
    if usages.is_empty() {
        out.push_str("| Token 统计 | 不可用 |\n");
        out.push_str("| Cache 统计 | 不可用 |\n\n");
    } else {
        let input = usages
            .iter()
            .map(|usage| u128::from(usage.input_tokens))
            .sum::<u128>();
        let output = usages
            .iter()
            .map(|usage| u128::from(usage.output_tokens))
            .sum::<u128>();
        let cache_hit = usages
            .iter()
            .map(|usage| u128::from(usage.cache_hit_tokens))
            .sum::<u128>();
        let cache_miss = usages
            .iter()
            .map(|usage| u128::from(usage.cache_miss_tokens))
            .sum::<u128>();
        out.push_str(&format!(
            "| Token 统计 | {} 条可用；输入 {}；输出 {} |\n",
            usages.len(),
            input,
            output
        ));
        out.push_str(&format!(
            "| Cache 统计 | 命中 {}；未命中 {} |\n\n",
            cache_hit, cache_miss
        ));
    }

    out.push_str("## 逐用例诊断\n\n");
    out.push_str("| Case ID | 状态 | 耗时(ms) | 错误 |\n|---|---|---:|---|\n");
    if report.records.is_empty() {
        out.push_str("| - | 无用例 | - | 无 |\n");
    } else {
        for entry in report.records {
            match entry {
                Ok(record) => out.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    markdown_cell(&record.case_id),
                    markdown_cell(&record.status),
                    record.elapsed_ms,
                    if record.error.is_some() {
                        "有（详情已隐藏）"
                    } else {
                        "无"
                    }
                )),
                Err(_) => out.push_str("| - | 执行失败 | - | 有（详情已隐藏） |\n"),
            }
        }
    }
    out.push('\n');

    out.push_str("## 工具与性能观察\n\n");
    render_finding_subset(&mut out, report.findings, |finding| {
        finding_is_usable(finding, judge_usable)
            && TOOL_PERFORMANCE_FINDING_IDS.contains(&finding.id.as_str())
    });

    out.push_str("## 确定性规则发现\n\n");
    render_finding_subset(&mut out, report.findings, |finding| {
        finding.source == FindingSource::Rule
    });

    out.push_str("## 独立 Judge 质量评分\n\n");
    render_judge(&mut out, report.judge);

    out.push_str("## P0/P1/P2 改进建议\n\n");
    for severity in [
        FindingSeverity::P0,
        FindingSeverity::P1,
        FindingSeverity::P2,
    ] {
        out.push_str(&format!("### {}\n\n", severity_label(severity)));
        let matching = report
            .findings
            .iter()
            .filter(|finding| {
                finding.severity == severity && finding_is_usable(finding, judge_usable)
            })
            .collect::<Vec<_>>();
        if matching.is_empty() {
            out.push_str("- 无。\n\n");
        } else {
            for finding in matching {
                render_finding(&mut out, finding);
            }
        }
    }

    out.push_str("## 评测限制与可比性说明\n\n");
    if report.limitations.is_empty() {
        out.push_str("- 当前未记录额外限制；本报告仅描述本次运行，不代表趋势或公开榜单结果。\n");
    } else {
        for limitation in report.limitations {
            out.push_str(&format!("- {}\n", markdown_text(limitation)));
        }
        out.push_str("- 本报告仅描述本次运行，不代表趋势或公开榜单结果。\n");
    }
    out.push_str("- 公开榜单分数：不可用。未使用官方数据集、协议、评分器与固定版本，不能直接与 BFCL 比较。\n");
    out
}

fn render_product_diagnoses(
    out: &mut String,
    diagnoses: &[ProductDiagnosis],
    findings: &[EvalFinding],
    judge_usable: bool,
) {
    if diagnoses.is_empty() {
        if findings
            .iter()
            .any(|finding| finding.source == FindingSource::Rule)
        {
            out.push_str(
                "存在尚未归纳的问题，请查看确定性规则发现。建议补充固定诊断映射后复评。\n\n",
            );
        } else if judge_usable
            && findings
                .iter()
                .any(|finding| finding.source == FindingSource::Judge)
        {
            out.push_str(
                "未发现规则可识别的问题；存在尚未归纳的 AI 推断，请查看独立 Judge 质量评分。\n\n",
            );
        } else {
            out.push_str(
                "本次 smoke 未发现规则可识别的问题；样本较小，不能证明产品无问题。建议扩大样本并连续运行，观察问题是否稳定复现。\n\n",
            );
        }
        return;
    }
    for diagnosis in diagnoses {
        let source = match diagnosis.source {
            FindingSource::Rule => "[规则事实]",
            FindingSource::Judge => "[AI 推断]",
        };
        let ids = if diagnosis.affected_case_ids.is_empty() {
            "无安全用例标识".to_string()
        } else {
            diagnosis
                .affected_case_ids
                .iter()
                .map(|case_id| markdown_text(case_id))
                .collect::<Vec<_>>()
                .join("、")
        };
        out.push_str(&format!(
            "### {} {} · {}\n\n- 结论：{}\n- 影响范围：{} 个用例（{}）\n- 证据：{}\n- 建议动作：{}\n- 验收标准：{}\n\n",
            source,
            product_area_label(diagnosis.area),
            severity_label(diagnosis.severity),
            markdown_text(&diagnosis.conclusion),
            diagnosis.affected_case_count,
            ids,
            markdown_text(&diagnosis.evidence),
            markdown_text(&diagnosis.action),
            markdown_text(&diagnosis.acceptance),
        ));
    }
}

fn render_product_score(out: &mut String, score: &ProductScore, judge: &JudgeReport) {
    match (score.total, score.grade) {
        (Some(total), Some(grade)) => out.push_str(&format!(
            "- 总分：{total}/100\n- 等级：{}\n",
            product_grade_label(grade)
        )),
        _ => out.push_str("- 总分：不可用\n- 等级：不可用\n"),
    }
    out.push_str(&format!(
        "- 公式版本：{}\n- 置信度：{}\n",
        markdown_text(&score.version),
        product_confidence_label(score.confidence)
    ));
    if score.confidence == ProductScoreConfidence::LowSample {
        out.push_str("- 警告：样本量较小，分数仅用于本次 smoke 诊断。\n");
    }
    let dimension_value = |value: u8| {
        score
            .total
            .map(|_| value.to_string())
            .unwrap_or_else(|| "不可用".to_string())
    };
    let task_completion = dimension_value(score.dimensions.task_completion);
    let tool_reliability = dimension_value(score.dimensions.tool_reliability);
    let constraint_adherence = dimension_value(score.dimensions.constraint_adherence);
    let performance_efficiency = dimension_value(score.dimensions.performance_efficiency);
    let runtime_stability = dimension_value(score.dimensions.runtime_stability);
    out.push_str(&format!(
        "- 五项子分：任务完成：{}；工具可靠性：{}；约束遵循：{}；性能效率：{}；运行稳定性：{}\n",
        task_completion,
        tool_reliability,
        constraint_adherence,
        performance_efficiency,
        runtime_stability,
    ));
    out.push_str("\n| 子分 | 分数 |\n|---|---:|\n");
    out.push_str(&format!(
        "| 任务完成 | {} |\n| 工具可靠性 | {} |\n| 约束遵循 | {} |\n| 性能效率 | {} |\n| 运行稳定性 | {} |\n\n",
        task_completion,
        tool_reliability,
        constraint_adherence,
        performance_efficiency,
        runtime_stability,
    ));
    out.push_str("### 扣分明细\n\n");
    if score.deductions.is_empty() {
        out.push_str("- 无。\n\n");
    } else {
        for deduction in &score.deductions {
            let case_id = deduction
                .case_id
                .as_deref()
                .map(markdown_text)
                .unwrap_or_else(|| "整体".to_string());
            out.push_str(&format!(
                "- {} · Case: {} · {} · 扣 {} 分 · 证据：{}\n",
                markdown_text(&deduction.finding_id),
                case_id,
                product_dimension_label(deduction.dimension),
                deduction.points,
                markdown_text(&deduction.evidence),
            ));
        }
        out.push('\n');
    }
    if matches!(
        judge.status,
        JudgeStatus::Failed { .. } | JudgeStatus::NotConfigured
    ) {
        out.push_str(
            "Judge 失败或未配置；Product Score 不受影响。建议检查 Judge 模型配置与响应格式。\n\n",
        );
    }
    out.push_str("公开榜单分数：不可用。未使用官方数据集、协议、评分器与固定版本，不能直接与 BFCL 比较。\n\n");
    out.push_str(
        "Product Score 仅可在相同 case 集、配置、模型、环境与评分公式版本之间进行内部比较。\n\n",
    );
}

fn product_area_label(area: ProductProblemArea) -> &'static str {
    match area {
        ProductProblemArea::TaskCompletion => "任务完成",
        ProductProblemArea::Toolchain => "工具链",
        ProductProblemArea::Constraints => "约束遵循",
        ProductProblemArea::Performance => "性能",
        ProductProblemArea::CacheStability => "缓存稳定性",
    }
}

fn product_grade_label(grade: ProductGrade) -> &'static str {
    match grade {
        ProductGrade::Excellent => "优秀",
        ProductGrade::Good => "良好",
        ProductGrade::Fair => "需改进",
        ProductGrade::HighRisk => "高风险",
    }
}

fn product_confidence_label(confidence: ProductScoreConfidence) -> &'static str {
    match confidence {
        ProductScoreConfidence::Unavailable => "不可用",
        ProductScoreConfidence::LowSample => "小样本",
        ProductScoreConfidence::Standard => "标准",
    }
}

fn product_dimension_label(dimension: ProductScoreDimension) -> &'static str {
    match dimension {
        ProductScoreDimension::TaskCompletion => "任务完成",
        ProductScoreDimension::ToolReliability => "工具可靠性",
        ProductScoreDimension::ConstraintAdherence => "约束遵循",
        ProductScoreDimension::PerformanceEfficiency => "性能效率",
        ProductScoreDimension::RuntimeStability => "运行稳定性",
    }
}

fn render_finding_subset(
    out: &mut String,
    findings: &[EvalFinding],
    include: impl Fn(&EvalFinding) -> bool,
) {
    let mut rendered = false;
    for finding in findings.iter().filter(|finding| include(finding)) {
        render_finding(out, finding);
        rendered = true;
    }
    if !rendered {
        out.push_str("- 无。\n\n");
    }
}

fn render_finding(out: &mut String, finding: &EvalFinding) {
    let source = match finding.source {
        FindingSource::Rule => "[规则事实]",
        FindingSource::Judge => "[AI 推断]",
    };
    let case = finding
        .case_id
        .as_deref()
        .map(markdown_text)
        .unwrap_or_else(|| "整体".to_string());
    out.push_str(&format!(
        "- **{} {}**（{}，Case: {}）\n  - 证据：{}\n  - 影响：{}\n  - 建议：{}\n",
        source,
        markdown_text(&finding.title),
        severity_label(finding.severity),
        case,
        markdown_text(&finding.evidence),
        markdown_text(&finding.impact),
        markdown_text(&finding.recommendation)
    ));
}

fn render_judge(out: &mut String, judge: &JudgeReport) {
    match &judge.status {
        JudgeStatus::Completed => {
            let valid = judge_report_is_usable(judge);
            if valid {
                out.push_str("状态：已完成。\n\n");
            } else {
                out.push_str("状态：无效 Judge 结果（已降级）。\n\n");
            }
            out.push_str("| 维度 | 分数 | 置信度 | 证据 |\n|---|---:|---:|---|\n");
            for required in JUDGE_DIMENSIONS {
                let dimension = valid.then(|| {
                    judge
                        .dimensions
                        .iter()
                        .find(|dimension| dimension.dimension == required)
                        .expect("validated Judge dimension")
                });
                if let Some(dimension) = dimension {
                    out.push_str(&format!(
                        "| {required} | {} | {:.2} | {} |\n",
                        dimension.score,
                        dimension.confidence,
                        markdown_cell(&dimension.evidence)
                    ));
                } else {
                    out.push_str(&format!("| {required} | 未提供 | 未提供 | 未提供 |\n"));
                }
            }
            out.push('\n');
        }
        JudgeStatus::NotConfigured => out.push_str("状态：未配置。\n\n"),
        JudgeStatus::SkippedSameModel { .. } => out.push_str("状态：已跳过（同模型隔离）。\n\n"),
        JudgeStatus::Failed { .. } => out.push_str("状态：失败（详情已隐藏）。\n\n"),
    }
}

fn finding_is_usable(finding: &EvalFinding, judge_usable: bool) -> bool {
    finding.source == FindingSource::Rule || judge_usable
}

fn severity_label(severity: FindingSeverity) -> &'static str {
    match severity {
        FindingSeverity::P0 => "P0",
        FindingSeverity::P1 => "P1",
        FindingSeverity::P2 => "P2",
    }
}

fn median(values: &[u64]) -> Option<u128> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        Some((u128::from(sorted[middle - 1]) + u128::from(sorted[middle])) / 2)
    } else {
        Some(u128::from(sorted[middle]))
    }
}

fn markdown_cell(value: &str) -> String {
    markdown_text(value)
}

fn markdown_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            escaped.push_str("<br>");
        } else if ch == '\n' {
            escaped.push_str("<br>");
        } else {
            if ch.is_ascii_punctuation() {
                escaped.push('\\');
            }
            escaped.push(ch);
        }
    }
    escaped
}

fn contains_sensitive_content(markdown: &str) -> bool {
    let lower = normalize_guard_text(markdown);
    contains_sensitive_assignment(&lower)
        || ["ghp_", "github_pat_", "sk-", "sk_"]
            .iter()
            .any(|prefix| contains_token_family(&lower, prefix))
}

fn normalize_guard_text(markdown: &str) -> String {
    let lower = markdown.to_ascii_lowercase().replace("<br>", " ");
    let mut unescaped = String::with_capacity(lower.len());
    let mut chars = lower.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek().is_some_and(|next| next.is_ascii_punctuation()) {
            unescaped.push(chars.next().expect("peeked punctuation"));
        } else {
            unescaped.push(ch);
        }
    }

    let mut normalized = String::with_capacity(unescaped.len());
    let mut whitespace = false;
    for ch in unescaped.chars() {
        if ch.is_ascii_whitespace() {
            if !whitespace {
                normalized.push(' ');
                whitespace = true;
            }
        } else {
            normalized.push(ch);
            whitespace = false;
        }
    }
    normalized
}

fn contains_sensitive_assignment(text: &str) -> bool {
    contains_spaced_api_key_assignment(text)
        || text
            .char_indices()
            .filter(|(_, ch)| matches!(*ch, ':' | '='))
            .any(|(delimiter, _)| {
                let Some(key) = assignment_key(&text[..delimiter]) else {
                    return false;
                };
                sensitive_key(&key) && assigned_value_is_sensitive(&text[delimiter + 1..])
            })
}

fn contains_spaced_api_key_assignment(text: &str) -> bool {
    let mut rest = text;
    while let Some(index) = rest.find("api") {
        let boundary_before = index == 0
            || rest[..index]
                .chars()
                .next_back()
                .is_some_and(|ch| !is_field_character(ch));
        let after_api = &rest[index + "api".len()..];
        let whitespace_count = after_api
            .chars()
            .take_while(|ch| ch.is_ascii_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
        if boundary_before && whitespace_count > 0 {
            let after_space = &after_api[whitespace_count..];
            if let Some(after_key) = after_space.strip_prefix("key") {
                let mut after_key =
                    after_key.trim_start_matches(|ch: char| ch.is_ascii_whitespace());
                if after_key
                    .chars()
                    .next()
                    .is_some_and(|ch| matches!(ch, '\'' | '"'))
                {
                    after_key =
                        after_key[1..].trim_start_matches(|ch: char| ch.is_ascii_whitespace());
                }
                if after_key.starts_with(':') || after_key.starts_with('=') {
                    return assigned_value_is_sensitive(&after_key[1..]);
                }
            }
        }
        rest = after_api;
    }
    false
}

fn assignment_key(before: &str) -> Option<String> {
    let before = before.trim_end_matches(|ch: char| ch.is_ascii_whitespace());
    let key = if let Some(quote) = before
        .chars()
        .next_back()
        .filter(|ch| matches!(ch, '\'' | '"'))
    {
        let without_quote = &before[..before.len() - quote.len_utf8()];
        let start = without_quote.rfind(quote)?;
        &without_quote[start + quote.len_utf8()..]
    } else {
        let start = before
            .char_indices()
            .rev()
            .take_while(|(_, ch)| is_field_character(*ch))
            .last()
            .map(|(index, _)| index)?;
        &before[start..]
    };
    if key.is_empty() || !key.chars().all(is_field_character) {
        return None;
    }
    Some(key.replace('-', "_").to_ascii_lowercase())
}

fn sensitive_key(key: &str) -> bool {
    CREDENTIAL_KEYS.contains(&key)
        || [
            "_api_key",
            "_token",
            "_access_token",
            "_auth_token",
            "_client_secret",
            "_password",
        ]
        .iter()
        .any(|suffix| key.ends_with(suffix))
}

fn assigned_value_is_sensitive(value: &str) -> bool {
    let value = value.trim_start_matches(|ch: char| ch.is_ascii_whitespace());
    let (value, quoted) = match value.chars().next() {
        Some(quote @ ('\'' | '"')) => (&value[quote.len_utf8()..], Some(quote)),
        _ => (value, None),
    };
    let end = value
        .char_indices()
        .find(|(_, ch)| {
            quoted.is_some_and(|quote| *ch == quote)
                || (quoted.is_none() && (ch.is_ascii_whitespace() || matches!(ch, ',' | '}' | ']')))
        })
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    let first = value[..end]
        .trim_matches(|ch: char| "\"'[]<>(){},;.".contains(ch))
        .to_ascii_lowercase();
    if first.is_empty() || first == "-" || first.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }
    if matches!(
        first.as_str(),
        "null" | "none" | "not_configured" | "unavailable" | "disabled" | "redacted" | "hidden"
    ) {
        return false;
    }
    if value.to_ascii_lowercase().starts_with("not configured") {
        return false;
    }
    if matches!(first.as_str(), "bearer" | "basic") {
        let remainder = value[end..].trim_start_matches(|ch: char| ch.is_ascii_whitespace());
        return assigned_value_is_sensitive(remainder);
    }
    true
}

fn contains_token_family(text: &str, prefix: &str) -> bool {
    let mut rest = text;
    while let Some(index) = rest.find(prefix) {
        let boundary_before = index == 0
            || rest[..index]
                .chars()
                .next_back()
                .is_some_and(|ch| !is_token_character(ch));
        let tail = rest[index + prefix.len()..]
            .chars()
            .take_while(|ch| is_token_character(*ch))
            .count();
        if boundary_before && tail >= 12 {
            return true;
        }
        rest = &rest[index + prefix.len()..];
    }
    false
}

fn is_field_character(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

fn is_token_character(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}
