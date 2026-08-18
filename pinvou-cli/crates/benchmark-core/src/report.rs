use std::path::{Path, PathBuf};

use crate::security::validate_safe_text;
use crate::{Result, RunStore};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportArtifact {
    path: PathBuf,
}

impl ReportArtifact {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn publish_markdown_report(store: &RunStore, markdown: &str) -> Result<ReportArtifact> {
    for line in markdown.lines() {
        validate_safe_text(line)?;
    }
    let path = store.publish_new_bytes("report.md", markdown.as_bytes())?;
    Ok(ReportArtifact { path })
}

pub fn publish_score_json(store: &RunStore, json: &[u8]) -> Result<ReportArtifact> {
    if json.len() > 64 * 1024 || serde_json::from_slice::<serde_json::Value>(json).is_err() {
        return Err(crate::BenchmarkError::coded("invalid_score_artifact"));
    }
    let path = store.publish_new_bytes("score.json", json)?;
    Ok(ReportArtifact { path })
}
