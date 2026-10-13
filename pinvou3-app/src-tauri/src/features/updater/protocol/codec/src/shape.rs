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
