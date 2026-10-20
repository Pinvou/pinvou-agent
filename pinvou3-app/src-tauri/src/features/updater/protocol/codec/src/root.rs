//! Root descriptor/policy consistency only, never bootstrap trust or signatures.
use crate::policy::{
    DeclaredRolePolicy, PolicyLookupError, RoleContextShape, RolePolicyData, select_declared_policy,
};
use crate::shape::{ROLES, TARGETS, closed, identifier, one_of, positive_safe_integer, sha256_hex};
use crate::{
    Document, Error, KeyDescriptor, MetadataSemanticsError, RootWindowMode, SuppliedProtocolTime,
    key_descriptor,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootPolicyError {
    Json(Error),
    InvalidClaims,
    InvalidKey,
    InvalidPolicy,
}
impl std::fmt::Display for RootPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::InvalidClaims => f.write_str("ROOT_CLAIMS_INVALID"),
            Self::InvalidKey => f.write_str("ROOT_DESCRIPTOR_INVALID"),
            Self::InvalidPolicy => f.write_str("ROOT_POLICY_INVALID"),
        }
    }
}
impl std::error::Error for RootPolicyError {}
/// Canonical supplied claims whose descriptor/policy structure is consistent.
/// No Debug/Serialize or public constructor. This does not attest valid curve
/// points, signature thresholds, Root authenticity, time validity or currentness.
pub struct RootPolicyShape {
    canonical: Vec<u8>,
    product: String,
    version: u64,
    policies: Vec<RolePolicyData>,
    keys: BTreeMap<String, KeyDescriptor>,
    issued_at: f64,
    expires_at: f64,
}
impl RootPolicyShape {
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical
    }
    pub fn product(&self) -> &str {
        &self.product
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    /// Original supplied window with expiry alone permitted. This checks neither
    /// lineage nor protected historical custody and cannot grant current rights.
    pub fn assert_supplied_historical_window(
        &self,
        now: &SuppliedProtocolTime,
    ) -> Result<(), MetadataSemanticsError> {
        crate::metadata_semantics::root_window(
            self.issued_at,
            self.expires_at,
            now,
            RootWindowMode::AllowExpired,
        )
    }
    /// Exact immutable descriptor from this same validated Root shape. This is
    /// not a curve validity, key authority or protected Root acceptance proof.
    pub fn declared_key_for(&self, key_id: &str) -> Option<&KeyDescriptor> {
        self.keys.get(key_id)
    }
    /// Ordinary fresh Root window at caller-supplied time. No expired-history
    /// option, trusted clock acquisition or protected Root acceptance is implied.
    pub fn assert_supplied_current_window(
        &self,
        now: &SuppliedProtocolTime,
    ) -> Result<(), MetadataSemanticsError> {
        crate::metadata_semantics::root_window(
            self.issued_at,
            self.expires_at,
            now,
            RootWindowMode::Current,
        )
    }
    /// Lookup only the exact declared product/role/component/channel/target tuple.
    /// This Root shape is not an authenticated Root or signature/key authority.
    pub fn declared_policy_for(
        &self,
        context: &RoleContextShape,
    ) -> Result<DeclaredRolePolicy<'_>, PolicyLookupError> {
        select_declared_policy(&self.product, &self.policies, context)
    }
}
impl Document {
    /// Match the frozen validateTrustedRoot structural contract only. The caller
    /// establishes the initial anchor, signatures, original clock and lineage.
    /// Issued/expiry are safe integer shapes here, not an observed time window.
    pub fn root_policy_shape(&self) -> Result<RootPolicyShape, RootPolicyError> {
        let root = closed(
            &self.value,
            &[
                "protocolVersion",
                "role",
                "product",
                "component",
                "scope",
                "version",
                "issuedAt",
                "expiresAt",
                "keys",
                "roles",
            ],
        )
        .ok_or(RootPolicyError::InvalidClaims)?;
        if root["protocolVersion"].as_f64() != Some(1.0)
            || root["role"].as_str() != Some("root")
            || !identifier(&root["product"])
            || !root["component"].is_null()
            || closed(&root["scope"], &[]).is_none()
            || !positive_safe_integer(&root["version"])
            || !nonnegative_safe_integer(&root["issuedAt"])
            || !nonnegative_safe_integer(&root["expiresAt"])
        {
            return Err(RootPolicyError::InvalidClaims);
        }
        let keys = root["keys"]
            .as_array()
            .filter(|keys| !keys.is_empty())
            .ok_or(RootPolicyError::InvalidKey)?;
        let mut key_ids = BTreeSet::new();
        let mut declared_keys = BTreeMap::new();
        for value in keys {
            let key = closed(value, &["keyId", "algorithm", "publicKey"])
                .ok_or(RootPolicyError::InvalidKey)?;
            if key["algorithm"].as_str() != Some("ed25519") || !sha256_hex(&key["keyId"]) {
                return Err(RootPolicyError::InvalidKey);
            }
            let public_key = key["publicKey"]
                .as_str()
                .ok_or(RootPolicyError::InvalidKey)?;
            let descriptor = key_descriptor(public_key).map_err(|_| RootPolicyError::InvalidKey)?;
            let id = key["keyId"].as_str().ok_or(RootPolicyError::InvalidKey)?;
            if descriptor.key_id() != id || !key_ids.insert(id) {
                return Err(RootPolicyError::InvalidKey);
            }
            declared_keys.insert(id.to_owned(), descriptor);
        }
        let roles = root["roles"]
            .as_array()
            .filter(|roles| !roles.is_empty())
            .ok_or(RootPolicyError::InvalidPolicy)?;
        let mut policies = BTreeSet::new();
        let mut root_count = 0;
        let mut declared = Vec::with_capacity(roles.len());
        for value in roles {
            let policy = closed(
                value,
                &[
                    "role",
                    "component",
                    "channel",
                    "targetKey",
                    "threshold",
                    "keyIds",
                ],
            )
            .ok_or(RootPolicyError::InvalidPolicy)?;
            if !one_of(&policy["role"], &ROLES)
                || !(policy["component"].is_null() || identifier(&policy["component"]))
                || !(policy["channel"].is_null()
                    || one_of(&policy["channel"], &["stable", "beta", "internal"]))
                || !(policy["targetKey"].is_null() || one_of(&policy["targetKey"], &TARGETS))
                || !positive_safe_integer(&policy["threshold"])
            {
                return Err(RootPolicyError::InvalidPolicy);
            }
            let role = policy["role"]
                .as_str()
                .ok_or(RootPolicyError::InvalidPolicy)?;
            let identity = (
                role,
                policy["component"].as_str(),
                policy["channel"].as_str(),
                policy["targetKey"].as_str(),
            );
            if !policies.insert(identity) {
                return Err(RootPolicyError::InvalidPolicy);
            }
            let members = policy["keyIds"]
                .as_array()
                .filter(|members| !members.is_empty())
                .ok_or(RootPolicyError::InvalidPolicy)?;
            let threshold = policy["threshold"]
                .as_f64()
                .ok_or(RootPolicyError::InvalidPolicy)?;
            if threshold > members.len() as f64 {
                return Err(RootPolicyError::InvalidPolicy);
            }
            let mut unique = BTreeSet::new();
            let mut member_ids = Vec::with_capacity(members.len());
            for member in members {
                let id = member.as_str().ok_or(RootPolicyError::InvalidPolicy)?;
                if !sha256_hex(member) || !key_ids.contains(id) || !unique.insert(id) {
                    return Err(RootPolicyError::InvalidPolicy);
                }
                member_ids.push(id.to_owned());
            }
            if role == "root" {
                root_count += 1;
                if identity.1.is_some()
                    || identity.2.is_some()
                    || identity.3.is_some()
                    || threshold < 2.0
                    || members.len() < 3
                {
                    return Err(RootPolicyError::InvalidPolicy);
                }
            } else if identity.1.is_none() {
                return Err(RootPolicyError::InvalidPolicy);
            }
            // Already positive integral binary64 <= actual members <=4096.
            // The bounded cast is exact; the u16 conversion is also checked.
            let threshold =
                u16::try_from(threshold as u64).map_err(|_| RootPolicyError::InvalidPolicy)?;
            declared.push(RolePolicyData {
                role: role.to_owned(),
                component: identity.1.map(str::to_owned),
                channel: identity.2.map(str::to_owned),
                target_key: identity.3.map(str::to_owned),
                threshold,
                key_ids: member_ids,
            });
        }
        if root_count != 1 {
            return Err(RootPolicyError::InvalidPolicy);
        }
        let canonical = self.canonical_bytes().map_err(RootPolicyError::Json)?;
        let product = root["product"]
            .as_str()
            .ok_or(RootPolicyError::InvalidClaims)?
            .to_owned();
        Ok(RootPolicyShape {
            canonical,
            product,
            // Already positive integral binary64 <= MAX_SAFE_INTEGER, so this
            // conversion is exact and within u64 on every supported target.
            version: root["version"]
                .as_f64()
                .ok_or(RootPolicyError::InvalidClaims)? as u64,
            policies: declared,
            keys: declared_keys,
            issued_at: root["issuedAt"]
                .as_f64()
                .ok_or(RootPolicyError::InvalidClaims)?,
            expires_at: root["expiresAt"]
                .as_f64()
                .ok_or(RootPolicyError::InvalidClaims)?,
        })
    }
}
fn nonnegative_safe_integer(value: &Value) -> bool {
    value.as_f64().is_some_and(|number| {
        (0.0..=9_007_199_254_740_991.0).contains(&number) && number.fract() == 0.0
    })
}
