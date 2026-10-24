//! Shared scheduled-message identity and persisted result-text contract.

pub const SCHEDULED_MESSAGE_MAX_CHARS: usize = 32 * 1024;
pub(crate) const MAX_TARGET_SESSION_LEN: usize = 128;
const DELIVERED_PREFIX: &str = "delivered to session ";

pub(crate) fn validate_target_identity(target: &str) -> Result<(), &'static str> {
    if target.is_empty()
        || target.len() > MAX_TARGET_SESSION_LEN
        || !target
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("invalid target session id");
    }
    let folded = target.to_ascii_lowercase();
    if target.starts_with('_')
        || ["sched-", "aux-", "eval_", "agentic_"]
            .iter()
            .any(|prefix| folded.starts_with(prefix))
    {
        return Err("target session is isolated or unattended, or reserved");
    }
    Ok(())
}

pub(crate) fn delivered_result(target: &str) -> String {
    format!("{DELIVERED_PREFIX}{target}")
}

pub(crate) fn delivered_target(summary: &str) -> Option<&str> {
    let target = summary.strip_prefix(DELIVERED_PREFIX)?;
    (!target.is_empty()
        && target.len() <= MAX_TARGET_SESSION_LEN
        && target
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then_some(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_boundaries_and_result_round_trip() {
        let boundary = "x".repeat(128);
        assert!(validate_target_identity(&boundary).is_ok());
        assert_eq!(
            delivered_target(&delivered_result(&boundary)),
            Some(boundary.as_str())
        );
        for bad in [
            "agentic_001",
            "AGENTIC_001",
            "_session_models",
            "aux-001",
            "sched-001",
            "eval_001",
        ] {
            assert!(validate_target_identity(bad).is_err(), "{bad}");
        }
        assert!(validate_target_identity(&"x".repeat(129)).is_err());
        assert_eq!(delivered_target("unrelated"), None);
    }
}
