//! Pinvou Review EXIT GATE。
//!
//! 用户开启品悟 review (`pinvou_review_enabled = true`) 后,accept_plan /
//! exit_plan_to_yolo 前必须通过此 GATE:plan_markdown 末尾必须含结构化的
//! `## PINVOU REVIEW REPORT` 表格,且所有 CRITICAL finding 已 RESOLVED 或
//! OVERRIDDEN_BY_USER。
//!
//! 设计依据:`docs/Pinvou-品悟设计.md` §4.2,以及本仓库 commit `7b983b6`
//! 的回滚教训(Qwen3.6 + advisory reminder 引导单飞不可靠,必须 blocking gate)。
//!
//! Fallback 路径(前端):LLM 没按格式输出表格时,前端 main.js 的
//! `synthesizeOverriddenReport()` 合成一个 OVERRIDDEN_BY_USER 占位表格,
//! 用户读完原话点 [✅ 直接执行] 仍能凿穿 GATE,避免死循环。

use serde::Serialize;

/// GATE 检查失败原因。前端据此决定是"自动重审"还是"提示用户强制继续"。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum GateError {
    /// plan 末尾没有 `## PINVOU REVIEW REPORT` heading。
    MissingReviewReport,
    /// 有 heading 但没找到 findings 表格(LLM 写错格式)。
    MalformedReport { hint: String },
    /// 表格存在,但有 CRITICAL 状态既非 RESOLVED 也非 OVERRIDDEN_BY_USER。
    UnresolvedCritical { unresolved_count: usize },
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingReviewReport => write!(
                f,
                "Pinvou 还没看过这个 plan(末尾缺 ## PINVOU REVIEW REPORT 表格)"
            ),
            Self::MalformedReport { hint } => write!(f, "Pinvou review 表格格式不对: {hint}"),
            Self::UnresolvedCritical { unresolved_count } => write!(
                f,
                "Pinvou 提了 {unresolved_count} 个 CRITICAL 没拍板,先回应再继续"
            ),
        }
    }
}

impl std::error::Error for GateError {}

/// 检查 plan_markdown 是否通过 GATE。
/// `Ok(())` = 放行,`Err(...)` = 阻塞,前端据 GateError 类型决定下一步。
pub fn check_exit_gate(plan_markdown: &str) -> Result<(), GateError> {
    let last_section = extract_last_section(plan_markdown).ok_or(GateError::MissingReviewReport)?;

    if !last_section
        .heading
        .eq_ignore_ascii_case("PINVOU REVIEW REPORT")
    {
        return Err(GateError::MissingReviewReport);
    }

    let findings = parse_findings_table(&last_section.body)
        .map_err(|hint| GateError::MalformedReport { hint })?;

    if findings.is_empty() {
        return Err(GateError::MalformedReport {
            hint: "findings 表格至少要 1 行(即使是 CLEAR 也要显式写出)".into(),
        });
    }

    let unresolved_count = findings
        .iter()
        .filter(|f| f.severity == Severity::Critical)
        .filter(|f| f.status != Status::Resolved && f.status != Status::OverriddenByUser)
        .count();

    if unresolved_count > 0 {
        return Err(GateError::UnresolvedCritical { unresolved_count });
    }

    Ok(())
}

struct Section<'a> {
    heading: String,
    body: &'a str,
}

/// 找文档中最后一个 `## ` heading,返回它的标题(去掉 `## ` 前缀,trim)和它下面的 body。
fn extract_last_section(md: &str) -> Option<Section<'_>> {
    let mut last_idx: Option<usize> = None;
    for (i, line) in md.lines().enumerate() {
        if let Some(rest) = line.strip_prefix("## ") {
            // 排除 `### ` 等更深层级
            if !rest.starts_with('#') {
                last_idx = Some(i);
            }
        }
    }
    let line_idx = last_idx?;
    let lines: Vec<&str> = md.lines().collect();
    let heading_line = lines[line_idx];
    let heading = heading_line.trim_start_matches("## ").trim().to_string();

    let body_start_byte = lines[..=line_idx]
        .iter()
        .map(|l| l.len() + 1)
        .sum::<usize>();
    let body = if body_start_byte >= md.len() {
        ""
    } else {
        &md[body_start_byte..]
    };

    Some(Section { heading, body })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Critical,
    Informational,
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Raised,
    Resolved,
    OverriddenByUser,
    Noted,
}

#[derive(Debug, Clone)]
struct Finding {
    severity: Severity,
    status: Status,
}

/// 解析 markdown 表格,提取 Severity 和 Status 两列。
/// 期望表头:`| Finding | Severity | Status | User Decision |`
fn parse_findings_table(body: &str) -> Result<Vec<Finding>, String> {
    let mut lines = body.lines().filter(|l| l.trim_start().starts_with('|'));
    let header = lines.next().ok_or_else(|| "找不到表格头".to_string())?;
    let sep = lines
        .next()
        .ok_or_else(|| "找不到表格分隔行(|---|)".to_string())?;
    if !sep.contains("---") {
        return Err("第二行不是分隔行(应该是 |---|---|...|)".into());
    }

    let header_cells: Vec<String> = split_md_row(header);
    let severity_idx = header_cells
        .iter()
        .position(|c| c.eq_ignore_ascii_case("Severity"))
        .ok_or_else(|| "表头缺 Severity 列".to_string())?;
    let status_idx = header_cells
        .iter()
        .position(|c| c.eq_ignore_ascii_case("Status"))
        .ok_or_else(|| "表头缺 Status 列".to_string())?;

    let mut findings = Vec::new();
    for (row_idx, line) in lines.enumerate() {
        let cells = split_md_row(line);
        if cells.len() <= severity_idx.max(status_idx) {
            continue;
        }
        let severity_raw = cells[severity_idx].trim();
        let status_raw = cells[status_idx].trim();
        if severity_raw.is_empty() && status_raw.is_empty() {
            continue;
        }
        let severity = parse_severity(severity_raw)
            .map_err(|e| format!("第 {} 行 Severity 不合法: {e}", row_idx + 1))?;
        let status = parse_status(status_raw)
            .map_err(|e| format!("第 {} 行 Status 不合法: {e}", row_idx + 1))?;
        findings.push(Finding { severity, status });
    }
    Ok(findings)
}

fn parse_severity(raw: &str) -> Result<Severity, String> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "CRITICAL" => Ok(Severity::Critical),
        "INFORMATIONAL" => Ok(Severity::Informational),
        "CLEAR" => Ok(Severity::Clear),
        other => Err(format!(
            "未知 severity `{other}`; 只允许 CRITICAL / INFORMATIONAL / CLEAR"
        )),
    }
}

fn parse_status(raw: &str) -> Result<Status, String> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "RAISED" => Ok(Status::Raised),
        "RESOLVED" => Ok(Status::Resolved),
        "OVERRIDDEN_BY_USER" => Ok(Status::OverriddenByUser),
        "NOTED" => Ok(Status::Noted),
        other => Err(format!(
            "未知 status `{other}`; 只允许 RAISED / RESOLVED / OVERRIDDEN_BY_USER / NOTED"
        )),
    }
}

fn split_md_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let stripped = trimmed.trim_start_matches('|').trim_end_matches('|');
    stripped.split('|').map(|c| c.trim().to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_OK: &str = r#"# Plan

Some content here.

## PINVOU REVIEW REPORT

| Finding | Severity | Status | User Decision |
|---------|----------|--------|---------------|
| 担忧 1 | CRITICAL | RESOLVED | Claw 已修 |
| 担忧 2 | INFORMATIONAL | RAISED | 用户判断不重要 |

**VERDICT**: clear — 可 ExitPlanMode
"#;

    const SAMPLE_MISSING: &str = r#"# Plan

## Steps

1. Do this
2. Do that
"#;

    const SAMPLE_UNRESOLVED: &str = r#"# Plan

## PINVOU REVIEW REPORT

| Finding | Severity | Status | User Decision |
|---------|----------|--------|---------------|
| 担忧 X | CRITICAL | RAISED | 待用户拍 |

**VERDICT**: 1 critical 待拍板
"#;

    const SAMPLE_OVERRIDDEN: &str = r#"# Plan

## PINVOU REVIEW REPORT

| Finding | Severity | Status | User Decision |
|---------|----------|--------|---------------|
| Pinvou 多虑了 | CRITICAL | OVERRIDDEN_BY_USER | 用户拍板继续 |

**VERDICT**: user override
"#;

    #[test]
    fn pass_when_all_critical_resolved() {
        assert!(check_exit_gate(SAMPLE_OK).is_ok());
    }

    #[test]
    fn reject_when_missing_report() {
        assert!(matches!(
            check_exit_gate(SAMPLE_MISSING),
            Err(GateError::MissingReviewReport)
        ));
    }

    #[test]
    fn reject_when_critical_unresolved() {
        match check_exit_gate(SAMPLE_UNRESOLVED) {
            Err(GateError::UnresolvedCritical { unresolved_count }) => {
                assert_eq!(unresolved_count, 1);
            }
            other => panic!("expected UnresolvedCritical, got {other:?}"),
        }
    }

    #[test]
    fn pass_when_overridden_by_user() {
        assert!(check_exit_gate(SAMPLE_OVERRIDDEN).is_ok());
    }

    #[test]
    fn reject_when_table_empty() {
        let plan = "# Plan\n\n## PINVOU REVIEW REPORT\n\n| Finding | Severity | Status | User Decision |\n|---|---|---|---|\n\n**VERDICT**: empty\n";
        assert!(matches!(
            check_exit_gate(plan),
            Err(GateError::MalformedReport { .. })
        ));
    }

    #[test]
    fn reject_when_heading_is_not_last() {
        let plan = "# Plan\n## PINVOU REVIEW REPORT\n\n| Finding | Severity | Status | User Decision |\n|---|---|---|---|\n| X | CRITICAL | RESOLVED | ok |\n\n## Other Section\n\nstuff\n";
        assert!(matches!(
            check_exit_gate(plan),
            Err(GateError::MissingReviewReport)
        ));
    }

    #[test]
    fn pass_with_only_clear_finding() {
        // 用户场景:Qwen3.6 没找出 CRITICAL,只标 CLEAR —— 应该放行。
        let plan = r#"# Plan

## PINVOU REVIEW REPORT

| Finding | Severity | Status | User Decision |
|---------|----------|--------|---------------|
| 方案合理,无明显风险 | CLEAR | NOTED | - |

**VERDICT**: clear
"#;
        assert!(check_exit_gate(plan).is_ok());
    }

    #[test]
    fn reject_unknown_severity_instead_of_silently_passing() {
        let plan = r#"# Plan

## PINVOU REVIEW REPORT

| Finding | Severity | Status | User Decision |
|---------|----------|--------|---------------|
| 模型把协议字段翻译了 | 需要改 | RAISED | 待用户拍 |

**VERDICT**: bad enum
"#;
        assert!(matches!(
            check_exit_gate(plan),
            Err(GateError::MalformedReport { .. })
        ));
    }

    #[test]
    fn reject_unknown_status_instead_of_silently_passing() {
        let plan = r#"# Plan

## PINVOU REVIEW REPORT

| Finding | Severity | Status | User Decision |
|---------|----------|--------|---------------|
| 状态拼错不能放行 | CRITICAL | DONE | 待用户拍 |

**VERDICT**: bad enum
"#;
        assert!(matches!(
            check_exit_gate(plan),
            Err(GateError::MalformedReport { .. })
        ));
    }
}
