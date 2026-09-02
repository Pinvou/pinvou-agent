use std::path::Path;

use crate::{BenchmarkError, Result};

const FORBIDDEN_PERSISTED_MARKERS: &[&str] = &[
    "authorization",
    "cookie",
    "api_key",
    "access_token",
    "ground_truth",
    "reference_answer",
    "hidden_test",
    "raw_prompt",
    "tool_input",
    "tool_output",
];

pub(crate) fn validate_component(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || value == "."
        || value == ".."
    {
        return Err(BenchmarkError::coded("invalid_identifier"));
    }
    Ok(())
}

pub(crate) fn validate_revision(value: &str) -> Result<()> {
    validate_safe_text(value)?;
    if value.eq_ignore_ascii_case("main") || value.eq_ignore_ascii_case("latest") {
        return Err(BenchmarkError::coded("mutable_revision"));
    }
    Ok(())
}

pub(crate) fn validate_safe_text(value: &str) -> Result<()> {
    let lower = value.to_ascii_lowercase();
    if value.len() > 512
        || value.chars().any(char::is_control)
        || FORBIDDEN_PERSISTED_MARKERS
            .iter()
            .any(|marker| lower.contains(marker))
    {
        return Err(BenchmarkError::coded("unsafe_persistence"));
    }
    Ok(())
}

pub(crate) fn ensure_explicit_base(base: &Path) -> Result<()> {
    if !base.is_absolute() || base.parent().is_none() {
        return Err(BenchmarkError::coded("unsafe_base_directory"));
    }
    Ok(())
}
