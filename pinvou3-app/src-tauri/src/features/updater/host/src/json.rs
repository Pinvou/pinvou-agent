//! Reuse the original strict duplicate/number parser; impose the smaller private IPC profile.
use crate::error::{Error, Result};
use pinvou_upgrade_codec::{Document, InputKind};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) const FRAME_MAX: usize = 65_536;
pub(crate) const RECORD_MAX: usize = 1_048_576;
pub(crate) const SAFE_MAX: u64 = 9_007_199_254_740_991;

pub(crate) fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub(crate) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=128).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(b))
}
fn private_bounds(value: &Value, depth: usize) -> bool {
    if depth > 16 {
        return false;
    }
    match value {
        Value::String(s) => s.len() <= 4096,
        Value::Array(a) => a.len() <= 128 && a.iter().all(|v| private_bounds(v, depth + 1)),
        Value::Object(o) => {
            o.len() <= 128
                && o.iter()
                    .all(|(k, v)| k.len() <= 4096 && private_bounds(v, depth + 1))
        }
        _ => true,
    }
}
pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8], ipc: bool) -> Result<T> {
    let doc = Document::parse(
        bytes,
        if ipc {
            InputKind::Credential
        } else {
            InputKind::Metadata
        },
    )
    .map_err(|_| {
        if ipc {
            Error::IpcInvalid
        } else {
            Error::StateCorrupt
        }
    })?;
    if ipc && !private_bounds(doc.value(), 1) {
        return Err(Error::IpcInvalid);
    }
    let canonical = doc.canonical_bytes().map_err(|_| Error::StateCorrupt)?;
    serde_json::from_slice(&canonical).map_err(|_| {
        if ipc {
            Error::IpcInvalid
        } else {
            Error::StateCorrupt
        }
    })
}
pub(crate) fn encode<T: Serialize>(value: &T, ipc: bool) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::StateCorrupt)?;
    let doc = Document::parse(
        &bytes,
        if ipc {
            InputKind::Credential
        } else {
            InputKind::Metadata
        },
    )
    .map_err(|_| Error::StateCorrupt)?;
    if ipc && !private_bounds(doc.value(), 1) {
        return Err(Error::IpcInvalid);
    }
    let canonical = doc.canonical_bytes().map_err(|_| Error::StateCorrupt)?;
    if canonical.len() > if ipc { FRAME_MAX } else { RECORD_MAX } {
        return Err(Error::StateCorrupt);
    }
    Ok(canonical)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_wire_rejects_duplicates_and_oversize_before_typed_decode() {
        assert!(decode::<Value>(br#"{"a":1,"a":2}"#, true).is_err());
        assert!(decode::<Value>(&vec![b' '; FRAME_MAX + 1], true).is_err());
        assert!(decode::<Value>(&serde_json::to_vec(&vec![0; 129]).unwrap(), true).is_err());
        assert!(decode::<Value>(&serde_json::to_vec(&"a".repeat(4097)).unwrap(), true).is_err());
        assert_eq!(decode::<Value>(br#"{"n":1}"#, true).unwrap()["n"], 1);
    }
}
