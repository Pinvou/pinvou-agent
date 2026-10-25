//! Shared scalar/object predicates from frozen common Schema, not full validation.
use serde_json::{Map, Value};

pub(super) fn closed<'a>(value: &'a Value, fields: &[&str]) -> Option<&'a Map<String, Value>> {
    value.as_object().filter(|object| {
        object.len() == fields.len() && fields.iter().all(|name| object.contains_key(*name))
    })
}
pub(super) fn one_of(value: &Value, choices: &[&str]) -> bool {
    value.as_str().is_some_and(|text| choices.contains(&text))
}
// Frozen ASCII pattern ^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$ with Node's
// non-multiline Unicode RegExp semantics. Do not normalize or trim original text.
pub(super) fn identifier(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        let bytes = text.as_bytes();
        (1..=128).contains(&bytes.len())
            && bytes[0].is_ascii_alphanumeric()
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(byte))
    })
}

pub(super) fn sha256_hex(value: &Value) -> bool {
    value.as_str().is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

pub(super) const TARGETS: [&str; 4] = [
    "windows-x86_64-nsis-perMachine",
    "linux-x86_64-deb-perMachine",
    "linux-arm64-deb-perMachine",
    "macos-universal-dmg-perMachine",
];

// All parsed numbers are binary64, matching the frozen Node/Ajv reference.
pub(super) fn positive_safe_integer(value: &Value) -> bool {
    value.as_f64().is_some_and(|number| {
        (1.0..=9_007_199_254_740_991.0).contains(&number) && number.fract() == 0.0
    })
}

pub(super) fn native_identity(value: &Value) -> bool {
    let Some(facts) = closed(
        value,
        &["size", "sha256", "identity", "publisher", "format"],
    ) else {
        return false;
    };
    let size = positive_safe_integer(&facts["size"]);
    let hash = sha256_hex(&facts["sha256"]);
    size && hash
        && identifier(&facts["identity"])
        && identifier(&facts["publisher"])
        && one_of(
            &facts["format"],
            &["nsis", "deb", "dmg", "helper", "launcher"],
        )
}

pub(super) fn canonical_version(value: &Value) -> bool {
    let Some(text) = value.as_str() else {
        return false;
    };
    let mut parts = text.split('.');
    for _ in 0..3 {
        let Some(part) = parts.next() else {
            return false;
        };
        // Schema has no integer magnitude bound for version components. A long
        // valid component must not fail through integer parsing/overflow.
        if part.is_empty()
            || !part.bytes().all(|b| b.is_ascii_digit())
            || (part.starts_with('0') && part != "0")
        {
            return false;
        }
    }
    parts.next().is_none()
}

pub(super) const ROLES: [&str; 21] = [
    "root",
    "timestamp",
    "snapshot",
    "target",
    "release",
    "package",
    "decision",
    "download",
    "authorization-preinstall",
    "authorization-install",
    "authorization-activate",
    "helper-cleanup",
    "telemetry-event",
    "preinstall-transaction-event",
    "install-transaction-event",
    "activate-transaction-event",
    "reconciliation-action",
    "recovery-review-action",
    "reconciliation-event",
    "recovery-review-event",
    "time",
];
