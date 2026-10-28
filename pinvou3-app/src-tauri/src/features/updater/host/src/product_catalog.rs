//! One bounded, append-only installation event; never a protected capability.
use crate::{
    error::{Error, Result},
    json,
    profile::Seed,
    registration::HostRegistration,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Event {
    pub(crate) version: u8,
    pub(crate) kind: String,
    pub(crate) revision: u64,
    pub(crate) previous_sha256: Option<String>,
    pub(crate) product: String,
    pub(crate) original_root_sha256: String,
    pub(crate) original_root_size: u64,
    pub(crate) profile_id: String,
    pub(crate) scope: String,
    pub(crate) component: String,
    pub(crate) target_key: String,
    pub(crate) seed_sha256: String,
}

/// A product-local ordering of original enrollment events. The registry owner
/// must authenticate its protected Head and full membership independently.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProductEvent {
    pub(crate) revision: u64,
    pub(crate) previous_sha256: Option<String>,
    pub(crate) enrollment: String,
}
impl ProductEvent {
    pub(crate) fn parse(raw: &[u8]) -> Result<Self> {
        let value: Self = json::decode(raw, false)?;
        if json::encode(&value, false)? != raw
            || value.revision == 0
            || value.revision > json::SAFE_MAX
            || (value.revision == 1) != value.previous_sha256.is_none()
            || value
                .previous_sha256
                .as_ref()
                .is_some_and(|s| !json::digest(s))
        {
            return Err(Error::StateCorrupt);
        }
        Event::parse(value.enrollment.as_bytes())?;
        Ok(value)
    }
    pub(crate) fn next(previous: Option<(&Self, &[u8])>, enrollment: &[u8]) -> Result<Self> {
        Event::parse(enrollment)?;
        let (revision, previous_sha256) = match previous {
            None => (1, None),
            Some((old, raw)) => {
                if Self::parse(raw)? != *old {
                    return Err(Error::StateCorrupt);
                }
                (
                    old.revision
                        .checked_add(1)
                        .filter(|v| *v <= json::SAFE_MAX)
                        .ok_or(Error::Unavailable)?,
                    Some(json::sha256(raw)),
                )
            }
        };
        Ok(Self {
            revision,
            previous_sha256,
            enrollment: std::str::from_utf8(enrollment)
                .map_err(|_| Error::StateCorrupt)?
                .to_owned(),
        })
    }
    pub(crate) fn follows(&self, old: &Self, raw: &[u8]) -> bool {
        self.revision == old.revision.checked_add(1).unwrap_or(0)
            && self.previous_sha256.as_deref() == Some(json::sha256(raw).as_str())
    }
}
impl Event {
    pub(crate) fn parse(raw: &[u8]) -> Result<Self> {
        let value: Self = json::decode(raw, false)?;
        if json::encode(&value, false)? != raw {
            return Err(Error::StateCorrupt);
        }
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<()> {
        if self.version != 1
            || !matches!(self.kind.as_str(), "production" | "lab")
            || self.revision == 0
            || self.revision > json::SAFE_MAX
            || (self.revision == 1) != self.previous_sha256.is_none()
            || self
                .previous_sha256
                .as_ref()
                .is_some_and(|s| !json::digest(s))
            || !json::identifier(&self.product)
            || !json::identifier(&self.profile_id)
            || !json::identifier(&self.component)
            || !json::identifier(&self.target_key)
            || !json::digest(&self.original_root_sha256)
            || !json::digest(&self.seed_sha256)
            || !json::digest(&self.scope)
            || self.original_root_size == 0
            || self.original_root_size > json::RECORD_MAX as u64
        {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }
    pub(crate) fn enroll(
        previous: Option<(&Self, &[u8])>,
        seed: &Seed,
        host: &HostRegistration,
    ) -> Result<Self> {
        if host.product != seed.product
            || host.component != seed.component
            || host.target_key != seed.target_key
            || host.profile_id != seed.profile_id
            || host.profile_kind != seed.profile_kind
            || host.genesis_seed_sha256 != seed.seed_sha256()
            || host.revision != 1
        {
            return Err(Error::ScopeConflict);
        }
        let (revision, previous_sha256) = match previous {
            None => (1, None),
            Some((old, raw)) => {
                old.validate()?;
                if old.kind != seed.profile_kind || json::encode(old, false)? != raw {
                    return Err(Error::ScopeConflict);
                }
                (
                    old.revision
                        .checked_add(1)
                        .filter(|n| *n <= json::SAFE_MAX)
                        .ok_or(Error::Unavailable)?,
                    Some(json::sha256(raw)),
                )
            }
        };
        let event = Self {
            version: 1,
            kind: seed.profile_kind.clone(),
            revision,
            previous_sha256,
            product: seed.product.clone(),
            original_root_sha256: seed.original_root.sha256.clone(),
            original_root_size: seed.original_root.size,
            profile_id: seed.profile_id.clone(),
            scope: host.installation_scope_id.clone(),
            component: seed.component.clone(),
            target_key: seed.target_key.clone(),
            seed_sha256: host.genesis_seed_sha256.clone(),
        };
        event.validate()?;
        Ok(event)
    }
    pub(crate) fn follows(&self, old: &Self, raw: &[u8]) -> Result<()> {
        self.validate()?;
        old.validate()?;
        if self.kind != old.kind
            || self.revision != old.revision.checked_add(1).ok_or(Error::StateCorrupt)?
            || self.previous_sha256.as_deref() != Some(json::sha256(raw).as_str())
        {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }
    pub(crate) fn matches(&self, seed: &Seed, host: &HostRegistration) -> bool {
        self.kind == seed.profile_kind
            && self.product == seed.product
            && self.profile_id == host.profile_id
            && self.scope == host.installation_scope_id
            && self.component == host.component
            && self.target_key == host.target_key
            && self.seed_sha256 == host.genesis_seed_sha256
            && self.original_root_sha256 == seed.original_root.sha256
            && self.original_root_size == seed.original_root.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event() -> Event {
        Event {
            version: 1,
            kind: "production".into(),
            revision: 1,
            previous_sha256: None,
            product: "Product".into(),
            original_root_sha256: "a".repeat(64),
            original_root_size: 100,
            profile_id: "one".into(),
            scope: "1".repeat(64),
            component: "app".into(),
            target_key: "windows-x64".into(),
            seed_sha256: "f".repeat(64),
        }
    }
    #[test]
    fn event_chain_refuses_wrong_predecessor_and_revision() {
        let one = event();
        let raw = json::encode(&one, false).unwrap();
        let mut two = one.clone();
        two.revision = 2;
        two.previous_sha256 = Some(json::sha256(&raw));
        two.profile_id = "two".into();
        two.scope = "2".repeat(64);
        two.follows(&one, &raw).unwrap();
        two.previous_sha256 = Some("0".repeat(64));
        assert!(two.follows(&one, &raw).is_err());
        two.previous_sha256 = Some(json::sha256(&raw));
        two.revision = 3;
        assert!(two.follows(&one, &raw).is_err());
    }
    #[test]
    fn event_parse_is_closed_and_product_case_is_preserved() {
        let one = event();
        let raw = json::encode(&one, false).unwrap();
        assert!(Event::parse(&raw).is_ok());
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        value["extra"] = serde_json::Value::Bool(true);
        assert!(Event::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        let mut other = one.clone();
        other.product = "product".into();
        assert_ne!(
            json::sha256(one.product.as_bytes()),
            json::sha256(other.product.as_bytes())
        );
    }
    #[test]
    fn product_event_chain_refuses_changed_history() {
        let enrollment = json::encode(&event(), false).unwrap();
        let first = ProductEvent::next(None, &enrollment).unwrap();
        let first_raw = json::encode(&first, false).unwrap();
        let mut second = ProductEvent::next(Some((&first, &first_raw)), &enrollment).unwrap();
        assert!(second.follows(&first, &first_raw));
        second.previous_sha256 = Some("0".repeat(64));
        assert!(!second.follows(&first, &first_raw));
        assert!(ProductEvent::parse(b"{}").is_err());
        let mut changed = first_raw.clone();
        changed.push(b' ');
        assert!(ProductEvent::next(Some((&first, &changed)), &enrollment).is_err());
    }
}
