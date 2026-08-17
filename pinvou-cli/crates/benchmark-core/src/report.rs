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
