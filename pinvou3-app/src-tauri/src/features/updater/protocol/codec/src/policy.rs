//! Exact declared-policy lookup only, not authorized keys or trusted Root.
use crate::{Document, Error};
use serde_json::{Value, json};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyLookupError {
    Json(Error),
    InvalidContext,
    ProductMismatch,
    NotDeclared,
}
impl std::fmt::Display for PolicyLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::InvalidContext => f.write_str("ROLE_CONTEXT_INVALID"),
            Self::ProductMismatch => f.write_str("ROLE_PRODUCT_MISMATCH"),
            Self::NotDeclared => f.write_str("ROLE_POLICY_NOT_DECLARED"),
        }
    }
}
impl std::error::Error for PolicyLookupError {}
/// Partial supplied signed-body context only. Component must explicitly exist;
/// only missing/null channel and targetKey become None. Full claims/trust stay
/// with the owner. No public constructor, mutable access, Debug or Serialize.
pub struct RoleContextShape {
    role: String,
    product: String,
    component: Option<String>,
    channel: Option<String>,
    target_key: Option<String>,
}
impl Document {
    pub fn role_context_shape(&self) -> Result<RoleContextShape, PolicyLookupError> {
        let body = self
            .value
            .as_object()
            .ok_or(PolicyLookupError::InvalidContext)?;
        let text = |name| {
            body.get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(PolicyLookupError::InvalidContext)
        };
        let component = body
            .get("component")
            .ok_or(PolicyLookupError::InvalidContext)?;
        let component = nullable_text(Some(component))?;
        let scope = body
            .get("scope")
            .and_then(Value::as_object)
            .ok_or(PolicyLookupError::InvalidContext)?;
        Ok(RoleContextShape {
            role: text("role")?,
            product: text("product")?,
            component,
            channel: nullable_text(scope.get("channel"))?,
            target_key: nullable_text(scope.get("targetKey"))?,
        })
    }
}
fn nullable_text(value: Option<&Value>) -> Result<Option<String>, PolicyLookupError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        _ => Err(PolicyLookupError::InvalidContext),
    }
}
// Root owns this immutable cache. Only its validated same-pass builder populates
// the fields; member order and exact nullable identity are never normalized.
pub(super) struct RolePolicyData {
    pub(super) role: String,
    pub(super) component: Option<String>,
    pub(super) channel: Option<String>,
    pub(super) target_key: Option<String>,
    pub(super) threshold: u16,
    pub(super) key_ids: Vec<String>,
}
/// Borrowed declared facts tied to the originating Root shape's lifetime.
/// Not a signature policy acceptance, key trust or authority certificate.
pub struct DeclaredRolePolicy<'a> {
    data: &'a RolePolicyData,
}
impl DeclaredRolePolicy<'_> {
    pub fn threshold(&self) -> u16 {
        self.data.threshold
    }
    pub fn key_ids(&self) -> &[String] {
        &self.data.key_ids
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Error> {
        Document {
            value: json!({"role":self.data.role,"component":self.data.component,
            "channel":self.data.channel,"targetKey":self.data.target_key,
            "threshold":self.data.threshold,"keyIds":self.data.key_ids}),
        }
        .canonical_bytes()
    }
}
pub(super) fn select_declared_policy<'a>(
    product: &str,
    policies: &'a [RolePolicyData],
    context: &RoleContextShape,
) -> Result<DeclaredRolePolicy<'a>, PolicyLookupError> {
    if product != context.product {
        return Err(PolicyLookupError::ProductMismatch);
    }
    // Root shape validation guarantees exact tuple uniqueness. There is no
    // generic/wildcard fallback, scope merging or synthesized default policy.
    let data = policies
        .iter()
        .find(|policy| {
            policy.role == context.role
                && policy.component == context.component
                && policy.channel == context.channel
                && policy.target_key == context.target_key
        })
        .ok_or(PolicyLookupError::NotDeclared)?;
    Ok(DeclaredRolePolicy { data })
}
