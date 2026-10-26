//! Owner-approved initial engineering limits, not platform certification or
//! stage authority. Build inputs configure new installations only; reopening
//! an existing protected policy must never replace it with these values.

const SAFE_MAX: u64 = 9_007_199_254_740_991;
pub(crate) const DEFAULT_FREEZE_MS: u64 = 120_000;
pub(crate) const DEFAULT_BACKUP_MS: u64 = 30 * 60 * 1_000;

pub(crate) struct PreparationLimits {
    pub(crate) freeze_ms: u64,
    pub(crate) backup_ms: u64,
}

fn duration(value: Option<&str>, default: u64) -> Result<u64, &'static str> {
    let Some(text) = value else {
        return Ok(default);
    };
    if text.is_empty() || text.starts_with('0') || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err("HOST_PREPARATION_BUDGET_INVALID");
    }
    text.parse::<u64>()
        .ok()
        .filter(|v| *v > 0 && *v <= SAFE_MAX)
        .ok_or("HOST_PREPARATION_BUDGET_INVALID")
}

impl PreparationLimits {
    pub(crate) fn configured(
        freeze_ms: Option<&str>,
        backup_ms: Option<&str>,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            freeze_ms: duration(freeze_ms, DEFAULT_FREEZE_MS)?,
            backup_ms: duration(backup_ms, DEFAULT_BACKUP_MS)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_initial_limits_and_explicit_overrides_are_distinct() {
        let initial = PreparationLimits::configured(None, None).unwrap();
        assert_eq!((initial.freeze_ms, initial.backup_ms), (120_000, 1_800_000));
        let changed = PreparationLimits::configured(Some("45000"), Some("600000")).unwrap();
        assert_eq!((changed.freeze_ms, changed.backup_ms), (45_000, 600_000));
        let partial = PreparationLimits::configured(Some("1"), None).unwrap();
        assert_eq!((partial.freeze_ms, partial.backup_ms), (1, 1_800_000));
    }

    #[test]
    fn invalid_configuration_never_falls_back_to_initial_limits() {
        for input in [
            "",
            "0",
            "01",
            "+1",
            "-1",
            " 1",
            "1 ",
            "1.0",
            "1e3",
            "１２",
            "9007199254740992",
            "18446744073709551616",
        ] {
            assert!(PreparationLimits::configured(Some(input), None).is_err());
            assert!(PreparationLimits::configured(None, Some(input)).is_err());
        }
        assert_eq!(duration(Some("9007199254740991"), 1).unwrap(), SAFE_MAX);
    }
}
