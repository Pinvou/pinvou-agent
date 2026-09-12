//! `files` family: attachment ingest — converts a user-selected file into the
//! markdown text the model would see, mirroring
//! `pinvou3-app/src-tauri/src/app/commands/files.rs::ingest_file`.
//!
//! The GUI command is a thin wrapper over two public feature functions, and
//! this module calls exactly those:
//! - `validate_path` (`features::files::file_ingest::validate_path`) — the
//!   upload path policy (absolute, existing regular file, under `$HOME`,
//!   outside credential components). A missing or policy-rejected file is a
//!   runtime failure (exit 1), matching the CLI contract.
//! - `ingest_attachment` (`features::files::file_ingest::ingest_attachment`) —
//!   hard size/archive limits surface as stable wire codes
//!   (`attachment_file_too_large`, …) which the CLI reports verbatim; format
//!   degradation (missing pandoc/poppler, image without vision, binary) stays
//!   a *successful* ingest whose `warning` field carries the chip text, so the
//!   CLI also exits 0 and prints the warning line.
//!
//! The CLI never reads pixels or runs OCR itself; all conversion logic lives
//! in the feature layer. No Tauri host is booted.

use std::path::PathBuf;

use crate::support::{render, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::files::file_ingest::{self, IngestResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilesCommand {
    Ingest {
        path: PathBuf,
        output: Option<PathBuf>,
    },
}

const USAGE: &str = "usage: pinvou files ingest <PATH> [--output PATH]";

pub fn parse(values: &[String]) -> Result<FilesCommand, CliError> {
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(USAGE))?;
    match subcommand.as_str() {
        "ingest" => {
            let path = values
                .get(2)
                .map(PathBuf::from)
                .ok_or_else(|| CliError::usage("files ingest requires a PATH"))?;
            let rest = &values[3..];
            let mut output: Option<PathBuf> = None;
            let mut index = 0;
            while index < rest.len() {
                let token = rest[index].as_str();
                if token != "--output" {
                    return Err(CliError::usage(format!(
                        "unsupported files option: {token}"
                    )));
                }
                if output.is_some() {
                    return Err(CliError::usage("duplicate files option --output"));
                }
                let value = rest
                    .get(index + 1)
                    .ok_or_else(|| CliError::usage("files option --output requires a value"))?;
                if value.is_empty() || value.starts_with("--") {
                    return Err(CliError::usage("files option --output requires a value"));
                }
                output = Some(PathBuf::from(value));
                index += 2;
            }
            Ok(FilesCommand::Ingest { path, output })
        }
        _ => Err(CliError::usage(USAGE)),
    }
}

pub fn execute(command: FilesCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    let FilesCommand::Ingest {
        path,
        output: destination,
    } = command;
    let raw = path.to_string_lossy().into_owned();
    // Same two-step entry as the GUI `ingest_file` command: path policy first,
    // then the attachment ingest with hard limits as wire codes.
    let validated = file_ingest::validate_path(&raw)
        .map_err(|error| CliError::failed(format!("files ingest: {error}")))?;
    let result = file_ingest::ingest_attachment(&validated)
        .map_err(|code| CliError::failed(format!("files ingest: {code}")))?;
    match destination {
        Some(destination) => write_file(&result, &destination, output),
        None => print_result(&result, output),
    }
}

/// `--output PATH`: the extracted markdown (or an empty file when the ingest
/// produced only a placeholder, mirroring the GUI which would send no text)
/// is written to the file; stdout carries the summary fields.
fn write_file(
    result: &IngestResult,
    destination: &std::path::Path,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let markdown = result.markdown.clone().unwrap_or_default();
    std::fs::write(destination, markdown).map_err(|error| {
        CliError::failed(format!(
            "files ingest: cannot write {}: {error}",
            destination.display()
        ))
    })?;
    let mut value = ingest_json(result);
    if let Some(map) = value.as_object_mut() {
        map.insert(
            "output".to_owned(),
            serde_json::json!(destination.display().to_string()),
        );
    }
    let mut human = summary_lines(result);
    human.push(format!("Output: {}", destination.display()));
    Ok(success(render(output, human.join("\n"), &value)))
}

fn print_result(result: &IngestResult, output: OutputMode) -> Result<CliOutcome, CliError> {
    let value = ingest_json(result);
    let mut human = summary_lines(result);
    if let Some(markdown) = &result.markdown {
        human.push(markdown.clone());
    }
    Ok(success(render(output, human.join("\n"), &value)))
}

fn summary_lines(result: &IngestResult) -> Vec<String> {
    let mut lines = vec![
        format!("File: {}", result.basename),
        format!("Kind: {}", result.kind),
        format!("Tokens: {}", result.token_estimate),
        format!("Bytes: {}", result.byte_size),
    ];
    if let Some(warning) = &result.warning {
        lines.push(format!("Warning: {warning}"));
    }
    lines
}

fn ingest_json(result: &IngestResult) -> serde_json::Value {
    serde_json::json!({
        "kind": result.kind,
        "basename": result.basename,
        "path": result.path,
        "markdown": result.markdown,
        "token_estimate": result.token_estimate,
        "byte_size": result.byte_size,
        "warning": result.warning,
    })
}
