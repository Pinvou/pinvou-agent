//! Nonsecret incident DATA. Only the native policy observer can record a fault;
//! only authenticated zero-custody completion can mark its recovery complete.
use super::{Incarnation, Status};
use crate::{
    error::{Error, Result},
    json,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Kind {
    #[serde(rename = "retention_time_fault")]
    RetentionTimeFault,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Cause {
    #[serde(rename = "invalid_protected_time_policy")]
    InvalidProtectedTimePolicy,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PolicyFault {
    kind: Kind,
    cause: Cause,
    detected_revision: u64,
    previous_time_sha256: String,
    policy_sha256: String,
    incarnation: Incarnation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovered_revision: Option<u64>,
}
impl PolicyFault {
    pub(super) fn observed(
        revision: u64,
        previous: &[u8],
        policy_sha256: String,
        incarnation: Incarnation,
    ) -> Self {
        Self {
            kind: Kind::RetentionTimeFault,
            cause: Cause::InvalidProtectedTimePolicy,
            detected_revision: revision,
            previous_time_sha256: json::sha256(previous),
            policy_sha256,
            incarnation,
            recovered_revision: None,
        }
    }
    pub(crate) fn is_active(&self) -> bool {
        self.recovered_revision.is_none()
    }
    pub(crate) fn recovered(&self, revision: u64) -> Self {
        let mut next = self.clone();
        if next.is_active() {
            next.recovered_revision = Some(revision);
        }
        next
    }
    pub(crate) fn validate(
        &self,
        revision: u64,
        previous: Option<&str>,
        status: Status,
        incarnation: Option<&Incarnation>,
    ) -> Result<()> {
        self.incarnation.validate()?;
        if self.detected_revision < 2
            || self.detected_revision > revision
            || !json::digest(&self.previous_time_sha256)
            || !json::digest(&self.policy_sha256)
            || self.detected_revision == revision
                && previous != Some(self.previous_time_sha256.as_str())
            || self.detected_revision == revision
                && (status != Status::Unknown || incarnation != Some(&self.incarnation))
            || self.is_active() && status == Status::CleanupVerifiedZero
            || self
                .recovered_revision
                .is_some_and(|v| v > revision || v < self.detected_revision.saturating_add(2))
        {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }
    pub(crate) fn transition(
        old: Option<&Self>,
        next: Option<&Self>,
        status: Status,
        revision: u64,
        previous: &[u8],
    ) -> Result<()> {
        match (old, next) {
            (None, None) => Ok(()),
            (Some(old), Some(next)) if old == next => Ok(()),
            (old, Some(next))
                if old.is_none_or(|v| !v.is_active())
                    && next.is_active()
                    && status == Status::Unknown
                    && next.detected_revision == revision
                    && next.previous_time_sha256 == json::sha256(previous) =>
            {
                Ok(())
            }
            (Some(old), Some(next))
                if old.is_active()
                    && status == Status::CleanupVerifiedZero
                    && old.recovered(revision) == *next =>
            {
                Ok(())
            }
            _ => Err(Error::StateCorrupt),
        }
    }
}
