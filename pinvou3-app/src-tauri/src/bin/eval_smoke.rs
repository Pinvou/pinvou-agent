use std::path::Path;

use anyhow::{bail, Context, Result};

#[derive(Debug, PartialEq, Eq)]
struct CliArgs {
    judge_model_id: Option<String>,
}

fn parse_args<I, S>(args: I) -> Result<CliArgs>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let _program = args.next();
    let mut judge_model_id = None;
    while let Some(argument) = args.next() {
        match argument.as_ref() {
            "--mode" => {
                let mode = args.next().context("--mode requires product")?;
                match mode.as_ref() {
                    "product" => {}
                    "official-compatible" => {
                        bail!("official-compatible requires the future BFCL adapter")
                    }
                    value => bail!("unsupported eval mode: {value}"),
                }
            }
            "--judge-model-id" => {
                let value = args
                    .next()
                    .context("--judge-model-id requires a saved-model-id")?;
                let value = value.as_ref().trim();
                if value.is_empty() {
                    bail!("--judge-model-id requires a non-empty saved-model-id");
                }
                judge_model_id = Some(value.to_string());
            }
            "--help" | "-h" => {
                println!("Usage: eval_smoke [--mode product] [--judge-model-id <saved-model-id>]");
                std::process::exit(0);
            }
            value => bail!("unknown argument: {value}"),
        }
    }
    Ok(CliArgs { judge_model_id })
}

fn run() -> Result<i32> {
    let args = parse_args(std::env::args())?;
    let outcome = pinvou3_lib::run_product_eval_smoke(pinvou3_lib::EvalSmokeOptions {
        judge_model_id: args.judge_model_id,
    })?;
    let summary = format_summary(
        &outcome.jsonl_report_path,
        &outcome.markdown_report_path,
        outcome.product_score,
        outcome.product_score_version.as_deref(),
        pinvou3_lib::judge_status_label(&outcome.judge_status),
    );
    print!("{}", format_stdout(&outcome.markdown, &summary));
    Ok(if outcome.all_succeeded { 0 } else { 1 })
}

fn format_stdout(markdown: &str, summary: &str) -> String {
    format!("{}\n{}\n", markdown.trim_end(), summary)
}

fn format_summary(
    jsonl_path: &Path,
    markdown_path: &Path,
    product_score: Option<u8>,
    product_score_version: Option<&str>,
    judge_status: &str,
) -> String {
    let score = match (product_score, product_score_version) {
        (Some(score), Some(version)) => format!("{score}/100 ({version})"),
        _ => "unavailable".to_string(),
    };
    format!(
        "JSONL report: {}\nMarkdown report: {}\nProduct score: {}\nJudge status: {}",
        jsonl_path.display(),
        markdown_path.display(),
        score,
        judge_status
    )
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("eval_smoke failed: {error:#}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{format_stdout, format_summary, parse_args};
    use std::path::Path;

    #[test]
    fn eval_product_score_wiring_cli_summary_names_paths_score_and_judge_status() {
        let summary = format_summary(
            Path::new("C:/reports/run.jsonl"),
            Path::new("C:/reports/run.md"),
            Some(87),
            Some("pinvou-product-score/v1"),
            "failed",
        );

        assert!(summary.contains("JSONL report: C:/reports/run.jsonl"));
        assert!(summary.contains("Markdown report: C:/reports/run.md"));
        assert!(summary.contains("Product score: 87/100 (pinvou-product-score/v1)"));
        assert!(summary.contains("Judge status: failed"));
    }

    #[test]
    fn eval_product_score_wiring_cli_stdout_contains_markdown_and_summary() {
        let stdout = format_stdout("# report\n", "JSONL report: C:/reports/run.jsonl");

        assert!(stdout.starts_with("# report\n"));
        assert!(stdout.contains("JSONL report: C:/reports/run.jsonl"));
    }

    #[test]
    fn judge_model_id_is_parsed() {
        let args = parse_args([
            "eval_smoke",
            "--mode",
            "product",
            "--judge-model-id",
            "judge-a",
        ])
        .expect("parse judge model");

        assert_eq!(args.judge_model_id.as_deref(), Some("judge-a"));
    }

    #[test]
    fn judge_model_id_requires_a_value() {
        assert!(parse_args(["eval_smoke", "--judge-model-id"]).is_err());
    }

    #[test]
    fn judge_model_id_rejects_blank_values() {
        assert!(parse_args(["eval_smoke", "--judge-model-id", " "]).is_err());
    }

    #[test]
    fn official_compatible_mode_is_explicitly_unsupported() {
        let error = parse_args(["eval_smoke", "--mode", "official-compatible"])
            .unwrap_err()
            .to_string();
        assert!(error.contains("BFCL adapter"));
    }
}
