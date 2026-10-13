//! Canonical decimal-triple ordering only, not installed facts or eligibility.
use crate::shape::canonical_version;
use crate::{Document, Error};
use std::cmp::Ordering;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionError {
    Json(Error),
    InvalidVersion,
}
impl std::fmt::Display for VersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::InvalidVersion => f.write_str("APP_VERSION_INVALID"),
        }
    }
}
impl std::error::Error for VersionError {}
/// Validated decoded decimal triple, bounded by the original JSON string limit.
/// No public raw constructor or Debug/Serialize; no facts/currentness proof.
pub struct CanonicalAppVersion {
    text: String,
}
impl CanonicalAppVersion {
    pub fn as_str(&self) -> &str {
        &self.text
    }
}
impl Document {
    pub fn app_version_shape(&self) -> Result<CanonicalAppVersion, VersionError> {
        if !canonical_version(&self.value) {
            return Err(VersionError::InvalidVersion);
        }
        let text = self
            .value
            .as_str()
            .ok_or(VersionError::InvalidVersion)?
            .to_owned();
        Ok(CanonicalAppVersion { text })
    }
}
/// Match the frozen three-BigInt comparison without integer parsing or a new
/// magnitude/segment limit. Validated no-leading-zero ASCII decimal components
/// have exactly the same numeric order as length, then lexicographic bytes.
pub fn compare_app_versions(left: &CanonicalAppVersion, right: &CanonicalAppVersion) -> Ordering {
    for (a, b) in left.text.split('.').zip(right.text.split('.')) {
        let order = a.len().cmp(&b.len()).then_with(|| a.cmp(b));
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}
