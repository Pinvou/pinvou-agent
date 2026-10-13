//! Supplied expected context binding only, not authentication of its source.
use crate::{
    BoundedOutput, Document, EnvelopeShapeError, Error, SchemaCheckedEnvelope, SchemaError,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextBindingError {
    Json(Error),
    Envelope(EnvelopeShapeError),
    Schema(SchemaError),
    InvalidExpected,
    Mismatch,
}
impl std::fmt::Display for ContextBindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Json(error) => return error.fmt(f),
            Self::Envelope(error) => return error.fmt(f),
            Self::Schema(error) => return error.fmt(f),
            Self::InvalidExpected => "EXPECTED_CONTEXT_REQUIRED",
            Self::Mismatch => "CONTEXT_MISMATCH",
        })
    }
}
impl std::error::Error for ContextBindingError {}

/// Require independent expected facts for this complete Schema-checked envelope.
/// All four properties must be present. Extra expected properties are ignored
/// after Document's whole-input checks. Nothing creates expectations from a
/// received response, attests caller authority, verifies signatures or accepts
/// metadata persistently. Root component:null remains distinct from missing.
pub fn assert_supplied_expected_context(
    envelope: &SchemaCheckedEnvelope,
    expected: &Document,
) -> Result<(), ContextBindingError> {
    let expected = expected
        .value()
        .as_object()
        .filter(|expected| {
            ["role", "product", "component", "scope"]
                .iter()
                .all(|name| expected.contains_key(*name))
        })
        .ok_or(ContextBindingError::InvalidExpected)?;
    let signed = envelope.envelope().signed().value();
    // Full frozen Schema constrains these three fields to exact string/null
    // values, so Value equality reproduces original strict === without coercion.
    for name in ["role", "product", "component"] {
        if signed.get(name) != expected.get(name) {
            return Err(ContextBindingError::Mismatch);
        }
    }
    // Bind the complete scope, including channelRevision/installationScopeId.
    // Both borrowed subtrees passed raw bounds; no large Value clone is needed.
    let actual = canonical(&signed["scope"])?;
    let supplied = canonical(&expected["scope"])?;
    if actual != supplied {
        return Err(ContextBindingError::Mismatch);
    }
    Ok(())
}
fn canonical(value: &serde_json::Value) -> Result<Vec<u8>, ContextBindingError> {
    let mut output = BoundedOutput {
        bytes: Vec::new(),
        exceeded: false,
    };
    if serde_jcs::to_writer(&mut output, value).is_err() {
        return Err(ContextBindingError::Json(if output.exceeded {
            Error::OutputLimit
        } else {
            Error::Canonicalization
        }));
    }
    Ok(output.bytes)
}
