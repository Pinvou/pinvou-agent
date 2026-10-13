//! Shape and equality of supplied native identity facts; not actual OS identity.
use crate::shape::native_identity;
use crate::{Document, Error};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeIdentityError {
    Json(Error),
    InvalidIdentity,
    Mismatch,
}
impl std::fmt::Display for NativeIdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::InvalidIdentity => f.write_str("NATIVE_IDENTITY_SCHEMA_INVALID"),
            Self::Mismatch => f.write_str("NATIVE_IDENTITY_MISMATCH"),
        }
    }
}
impl std::error::Error for NativeIdentityError {}

/// Complete closed five-field shape, not an OS probe, signature/native package
/// authenticity, protected execution object or installation rights. Immutable,
/// privately constructed canonical bytes have no automatic Debug/Serialize.
pub struct NativeIdentityShape {
    canonical: Vec<u8>,
}
impl NativeIdentityShape {
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical
    }
}
impl Document {
    pub fn native_identity_shape(&self) -> Result<NativeIdentityShape, NativeIdentityError> {
        if !native_identity(&self.value) {
            return Err(NativeIdentityError::InvalidIdentity);
        }
        let canonical = self.canonical_bytes().map_err(NativeIdentityError::Json)?;
        Ok(NativeIdentityShape { canonical })
    }
}
/// Both values have already passed the original complete closed shape. Compare
/// every canonical field, not just size/hash. Equality attests only supplied facts.
pub fn assert_native_identity(
    expected: &NativeIdentityShape,
    actual: &NativeIdentityShape,
) -> Result<(), NativeIdentityError> {
    if expected.canonical == actual.canonical {
        Ok(())
    } else {
        Err(NativeIdentityError::Mismatch)
    }
}
