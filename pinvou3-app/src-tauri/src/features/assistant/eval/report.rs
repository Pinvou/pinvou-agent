//! 评测结果持久化与人类可读摘要。
//!
//! JSONL 报告先写入 `.tmp` 文件，每条 case 后立即 flush；只有完整批次写入
//! `complete` 记录后才原子改名为 `.jsonl`。因此进程中断时会留下可诊断的临时
//! 文件，但不会把不完整结果误当成正式报告。

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::{EvalMilestone, EvalMode, EvalRecord};
use crate::features::assistant::timing::TurnUsage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalRunMetadata {
    pub schema_version: u32,
    pub run_id: String,
    pub mode: EvalMode,
    pub case_set: String,
    pub case_set_version: String,
    pub pinvou_version: String,
    pub provider: String,
    pub model: String,
    pub started_at: String,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum EvalJsonLine<'a> {
    Run {
        metadata: &'a EvalRunMetadata,
    },
    Case {
        run_id: &'a str,
        record: PersistedEvalRecord<'a>,
    },
    CaseError {
        run_id: &'a str,
        case_id: &'a str,
        error: &'static str,
    },
    Complete {
        run_id: &'a str,
        finished_at: String,
        all_succeeded: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        analysis_status: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        product_score: Option<u8>,
        #[serde(skip_serializing_if = "Option::is_none")]
        product_score_version: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        markdown_report: Option<String>,
    },
}

#[derive(Serialize)]
struct PersistedEvalRecord<'a> {
    case_id: &'a str,
    session_id: &'a str,
    turn_id: &'a str,
    status: &'a str,
    error: Option<&'static str>,
    usage: &'a Option<TurnUsage>,
    milestones: &'a [EvalMilestone],
    elapsed_ms: u64,
}

impl<'a> From<&'a EvalRecord> for PersistedEvalRecord<'a> {
    fn from(record: &'a EvalRecord) -> Self {
        Self {
            case_id: &record.case_id,
            session_id: &record.session_id,
            turn_id: &record.turn_id,
            status: &record.status,
            error: persisted_error_category(record),
            usage: &record.usage,
            milestones: &record.milestones,
            elapsed_ms: record.elapsed_ms,
        }
    }
}

fn persisted_error_category(record: &EvalRecord) -> Option<&'static str> {
    if record.status.trim().eq_ignore_ascii_case("completed") && record.error.is_none() {
        None
    } else if record.status.trim().eq_ignore_ascii_case("timeout") {
        Some("timeout")
    } else {
        Some("runner_error")
    }
}

pub struct EvalReportWriter {
    run_id: String,
    temporary_path: PathBuf,
    final_path: PathBuf,
    writer: Option<BufWriter<File>>,
}

impl EvalReportWriter {
    pub fn create(metadata: EvalRunMetadata) -> Result<Self> {
        let reports_dir = crate::platform::paths::eval_reports_dir();
        fs::create_dir_all(&reports_dir)
            .with_context(|| format!("create eval report directory {}", reports_dir.display()))?;

        let timestamp = Utc::now().format("%Y%m%dT%H%M%S%3fZ");
        let run_id_for_filename = sanitize_filename_component(&metadata.run_id);
        let filename = format!("plep-smoke-{timestamp}-{run_id_for_filename}.jsonl");
        let final_path = reports_dir.join(&filename);
        let temporary_path = reports_dir.join(format!("{filename}.tmp"));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)
            .with_context(|| format!("create eval report {}", temporary_path.display()))?;
        let mut report = Self {
            run_id: metadata.run_id.clone(),
            temporary_path,
            final_path,
            writer: Some(BufWriter::new(file)),
        };
        report.write_line(&EvalJsonLine::Run {
            metadata: &metadata,
        })?;
        Ok(report)
    }

    pub fn temporary_path(&self) -> &Path {
        &self.temporary_path
    }

    pub fn final_path(&self) -> &Path {
        &self.final_path
    }

    pub fn append(&mut self, case_id: &str, record: &Result<EvalRecord>) -> Result<()> {
        let run_id = self.run_id.clone();
        match record {
            Ok(record) => self.write_line(&EvalJsonLine::Case {
                run_id: &run_id,
                record: PersistedEvalRecord::from(record),
            }),
            Err(_) => self.write_line(&EvalJsonLine::CaseError {
                run_id: &run_id,
                case_id,
                error: "case_execution_failed",
            }),
        }
    }

    pub fn finish(
        mut self,
        all_succeeded: bool,
        analysis_status: Option<&str>,
        product_score: Option<u8>,
        product_score_version: Option<&str>,
        markdown_report: Option<&Path>,
    ) -> Result<PathBuf> {
        let run_id = self.run_id.clone();
        let product_score_version = product_score.and(product_score_version).map(str::to_string);
        self.write_line(&EvalJsonLine::Complete {
            run_id: &run_id,
            finished_at: Utc::now().to_rfc3339(),
            all_succeeded,
            analysis_status: analysis_status.map(str::to_string),
            product_score,
            product_score_version,
            markdown_report: markdown_report.map(|path| path.display().to_string()),
        })?;

        let mut writer = self.writer.take().context("eval report writer is closed")?;
        writer.flush().context("flush eval report before rename")?;
        writer
            .get_ref()
            .sync_all()
            .context("sync eval report before rename")?;
        drop(writer);
        fs::rename(&self.temporary_path, &self.final_path).with_context(|| {
            format!(
                "finalize eval report {} -> {}",
                self.temporary_path.display(),
                self.final_path.display()
            )
        })?;
        Ok(self.final_path)
    }

    fn write_line<T: Serialize>(&mut self, value: &T) -> Result<()> {
        let writer = self
            .writer
            .as_mut()
            .context("eval report writer is closed")?;
        serde_json::to_writer(&mut *writer, value).context("serialize eval report line")?;
        writer
            .write_all(b"\n")
            .context("terminate eval report line")?;
        writer.flush().context("flush eval report line")?;
        Ok(())
    }
}

fn sanitize_filename_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "run".to_string()
    } else {
        sanitized
    }
}

/// 格式化评测报告为 Markdown 表格。
pub fn format_report(records: &[Result<EvalRecord>]) -> String {
    let mut out = String::new();
    out.push_str("| Case ID | Status | Elapsed(ms) | Input | Output | CacheHit | Error |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");
    for result in records {
        match result {
            Ok(record) => {
                let usage = record.usage;
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} | {} |\n",
                    escape_markdown_cell(&record.case_id),
                    escape_markdown_cell(&record.status),
                    record.elapsed_ms,
                    usage
                        .map(|value| value.input_tokens.to_string())
                        .unwrap_or_else(|| "-".into()),
                    usage
                        .map(|value| value.output_tokens.to_string())
                        .unwrap_or_else(|| "-".into()),
                    usage
                        .map(|value| value.cache_hit_tokens.to_string())
                        .unwrap_or_else(|| "-".into()),
                    escape_markdown_cell(record.error.as_deref().unwrap_or("-")),
                ));
            }
            Err(error) => {
                out.push_str(&format!(
                    "| ERROR | - | - | - | - | - | {} |\n",
                    escape_markdown_cell(&error.to_string())
                ));
            }
        }
    }
    out
}

fn escape_markdown_cell(value: &str) -> String {
    value.replace('|', "\\|").replace(['\r', '\n'], " ")
}
