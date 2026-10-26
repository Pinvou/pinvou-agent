//! Live preparation ownership DATA reducer. This module cannot authenticate a
//! Session, observe time, acquire a lock, fence a writer, or release a fence.
//!
//! The private native adapter must derive observations from held objects, not
//! deserialize them from IPC. In ONE borrowed protected transaction it checks
//! original Session/current binding/inventory/other occupancy, assesses the
//! original budget, runs this reducer and publishes owner plus admission state.
//! It compares exact previous bytes and performs exact committed readback before
//! exposing any permission. Calling budget_window and then publishing separately
//! is insufficient. The immutable bootstrap namespace must remain unchanged.
//!
//! Cancellation is two phase: publish Releasing while the physical fence is
//! still held; perform release; then observe convergence/readback and publish
//! completion. A crash or unknown result retains occupancy. Recovery requires
//! actual fencing of old executors before publishing new epochs and never
//! recreates a budget, clears another owner's reservation, or unfreezes writers.

use crate::{
    BootstrapBinding,
    binding::{digest, identifier},
};
use pinvou_upgrade_codec::{Document, InputKind};
use serde::{Deserialize, Serialize};

pub const LIVE_NAMESPACE: &str = "preparation-owner-live-v1";
pub const LIVE_RECORD_MAXIMUM: usize = 262_144;
pub const MAX_PARTICIPANTS: usize = 128;
const SAFE_MAX: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidRecord,
    BindingMismatch,
    Stale,
    Occupied,
    OriginalIdentityMismatch,
    BudgetMismatch,
    BudgetUnavailable,
    Incomplete,
    UnsafeBoundary,
    WrongPhase,
    Capacity,
    CorruptReadSet,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PREPARATION_OWNER_{self:?}")
    }
}
impl std::error::Error for Error {}
type Result<T> = std::result::Result<T, Error>;

fn positive(n: u64) -> bool {
    n > 0 && n <= SAFE_MAX
}
fn next(n: u64) -> Result<u64> {
    n.checked_add(1)
        .filter(|n| positive(*n))
        .ok_or(Error::Capacity)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Participant {
    pub id: String,
    /// Native incarnation identity; a reopened PID/name is insufficient.
    pub instance_sha256: String,
}
impl Participant {
    fn validate(&self) -> Result<()> {
        if identifier(&self.id) && digest(&self.instance_sha256) {
            Ok(())
        } else {
            Err(Error::InvalidRecord)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inventory {
    pub revision: u64,
    pub sha256: String,
    /// Exact sorted set, including disconnected users and background writers.
    pub participants: Vec<Participant>,
}
impl Inventory {
    fn validate(&self) -> Result<()> {
        if !positive(self.revision) || !digest(&self.sha256) {
            return Err(Error::InvalidRecord);
        }
        if self.participants.len() > MAX_PARTICIPANTS {
            return Err(Error::Capacity);
        }
        for (i, participant) in self.participants.iter().enumerate() {
            participant.validate()?;
            if i > 0 && self.participants[i - 1].id >= participant.id {
                return Err(Error::InvalidRecord);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Purpose {
    Preinstall,
    Install,
    Activate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparationIdentity {
    pub operation_id: String,
    pub original_session_id: String,
    pub original_session_sha256: String,
    pub request_lineage_sha256: String,
    pub purpose: Purpose,
}
impl PreparationIdentity {
    fn validate(&self) -> Result<()> {
        if identifier(&self.operation_id)
            && identifier(&self.original_session_id)
            && digest(&self.original_session_sha256)
            && digest(&self.request_lineage_sha256)
        {
            Ok(())
        } else {
            Err(Error::InvalidRecord)
        }
    }
}

/// Original protected PrepFreeze window identity and first start. The adapter
/// derives this from that actual window; a supplied reference is not authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BudgetOrigin {
    pub window_key_sha256: String,
    pub first_lower_ms: u64,
    pub first_upper_ms: u64,
    pub original_duration_ms: u64,
    pub original_deadline_ms: u64,
    pub original_session_cutoff_ms: u64,
}
impl BudgetOrigin {
    fn validate(&self) -> Result<()> {
        if !digest(&self.window_key_sha256)
            || self.first_lower_ms > self.first_upper_ms
            || self.first_upper_ms > SAFE_MAX
            || !positive(self.original_duration_ms)
            || self.first_lower_ms.checked_add(self.original_duration_ms)
                != Some(self.original_deadline_ms)
            || self.original_deadline_ms > SAFE_MAX
            || self.original_session_cutoff_ms > SAFE_MAX
        {
            return Err(Error::InvalidRecord);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum BudgetAssessment {
    Before {
        #[serde(rename = "remainingMs")]
        remaining_ms: u64,
    },
    Reached,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BudgetObservation {
    pub origin: BudgetOrigin,
    pub window_revision: u64,
    pub spent_max_ms: u64,
    /// May tighten, never extend even after a new telemetry Session.
    pub session_cutoff_ms: u64,
    pub assessment: BudgetAssessment,
}
impl BudgetObservation {
    fn validate(&self) -> Result<()> {
        self.origin.validate()?;
        if !positive(self.window_revision)
            || self.spent_max_ms > SAFE_MAX
            || self.spent_max_ms < self.origin.first_upper_ms - self.origin.first_lower_ms
            || self
                .origin
                .first_lower_ms
                .checked_add(self.spent_max_ms)
                .is_none_or(|n| n > SAFE_MAX)
            || self.session_cutoff_ms > self.origin.original_session_cutoff_ms
        {
            return Err(Error::BudgetMismatch);
        }
        if let BudgetAssessment::Before { remaining_ms } = self.assessment {
            let limit = self.origin.original_deadline_ms.min(self.session_cutoff_ms);
            let spent_until = self.origin.first_lower_ms + self.spent_max_ms;
            if !positive(remaining_ms) || spent_until >= limit || remaining_ms > limit - spent_until
            {
                return Err(Error::BudgetMismatch);
            }
        }
        Ok(())
    }
    fn successor(&self, later: &Self) -> Result<()> {
        later.validate()?;
        if later.origin != self.origin
            || later.window_revision < self.window_revision
            || later.spent_max_ms < self.spent_max_ms
            || later.session_cutoff_ms > self.session_cutoff_ms
            || (later.window_revision == self.window_revision && later != self)
            || (self.assessment == BudgetAssessment::Reached
                && later.assessment != BudgetAssessment::Reached)
        {
            return Err(Error::BudgetMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Token {
    pub revision: u64,
    pub owner_epoch: u64,
    pub freeze_epoch: Option<u64>,
}

/// Nonserialized observations of other owners, supplied only by the adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtherOccupancy {
    Clear,
    Execution,
    Reconciliation,
    Unknown,
}
pub struct Context<'a> {
    pub binding: &'a BootstrapBinding,
    pub inventory: &'a Inventory,
    pub other_occupancy: OtherOccupancy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HoldReason {
    TimeUnknown,
    BudgetExhausted,
    WriterRefused,
    Recovery,
    RevalidationRequired,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Phase {
    Coordinating,
    Frozen,
    Holding { reason: HoldReason },
    Releasing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Refusal {
    SaveFailed,
    ScopeInaccessible,
    WriterActive,
    Unsupported,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum AckStatus {
    SavedAndDrained {
        #[serde(rename = "receiptSha256")]
        receipt_sha256: String,
    },
    Refused {
        reason: Refusal,
    },
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParticipantAck {
    pub owner_epoch: u64,
    pub freeze_epoch: u64,
    pub inventory_revision: u64,
    pub inventory_sha256: String,
    pub participant: Participant,
    pub status: AckStatus,
}
impl ParticipantAck {
    fn validate(&self) -> Result<()> {
        self.participant.validate()?;
        if !positive(self.owner_epoch)
            || !positive(self.freeze_epoch)
            || !positive(self.inventory_revision)
            || !digest(&self.inventory_sha256)
        {
            return Err(Error::InvalidRecord);
        }
        if let AckStatus::SavedAndDrained { receipt_sha256 } = &self.status {
            if !digest(receipt_sha256) {
                return Err(Error::InvalidRecord);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preparation {
    identity: PreparationIdentity,
    inventory: Inventory,
    owner_epoch: u64,
    freeze_epoch: Option<u64>,
    budget: BudgetObservation,
    phase: Phase,
    acknowledgements: Vec<ParticipantAck>,
    fence_receipt_sha256: Option<String>,
    releasing_safety_sha256: Option<String>,
}
impl Preparation {
    pub fn identity(&self) -> &PreparationIdentity {
        &self.identity
    }
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }
    pub fn budget(&self) -> &BudgetObservation {
        &self.budget
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn owner_epoch(&self) -> u64 {
        self.owner_epoch
    }
    pub fn freeze_epoch(&self) -> Option<u64> {
        self.freeze_epoch
    }
    pub fn acknowledgements(&self) -> &[ParticipantAck] {
        &self.acknowledgements
    }
    pub fn fence_receipt_sha256(&self) -> Option<&str> {
        self.fence_receipt_sha256.as_deref()
    }
    fn validate(&self) -> Result<()> {
        self.identity.validate()?;
        self.inventory.validate()?;
        self.budget.validate()?;
        if !positive(self.owner_epoch)
            || self
                .freeze_epoch
                .is_some_and(|n| !positive(n) || n > self.owner_epoch)
            || (self.identity.purpose == Purpose::Preinstall) != self.freeze_epoch.is_none()
            || (self.identity.purpose == Purpose::Preinstall
                && (!self.acknowledgements.is_empty()
                    || self.fence_receipt_sha256.is_some()
                    || self.phase == Phase::Frozen))
            || self
                .fence_receipt_sha256
                .as_ref()
                .is_some_and(|s| !digest(s))
            || self
                .releasing_safety_sha256
                .as_ref()
                .is_some_and(|s| !digest(s))
            || (self.phase == Phase::Releasing) != self.releasing_safety_sha256.is_some()
        {
            return Err(Error::InvalidRecord);
        }
        if self.acknowledgements.len() > self.inventory.participants.len() {
            return Err(Error::InvalidRecord);
        }
        for (i, ack) in self.acknowledgements.iter().enumerate() {
            ack.validate()?;
            if !self.inventory.participants.contains(&ack.participant)
                || ack.owner_epoch != self.owner_epoch
                || Some(ack.freeze_epoch) != self.freeze_epoch
                || ack.inventory_revision != self.inventory.revision
                || ack.inventory_sha256 != self.inventory.sha256
                || (i > 0 && self.acknowledgements[i - 1].participant.id >= ack.participant.id)
            {
                return Err(Error::InvalidRecord);
            }
        }
        if self.phase == Phase::Frozen
            && (self.fence_receipt_sha256.is_none() || !self.complete_acks())
        {
            return Err(Error::Incomplete);
        }
        if matches!(self.phase, Phase::Coordinating | Phase::Frozen)
            && !matches!(self.budget.assessment, BudgetAssessment::Before { .. })
        {
            return Err(Error::BudgetMismatch);
        }
        if matches!(
            self.phase,
            Phase::Holding {
                reason: HoldReason::TimeUnknown
            }
        ) && self.budget.assessment != BudgetAssessment::Unknown
        {
            return Err(Error::BudgetMismatch);
        }
        if matches!(
            self.phase,
            Phase::Holding {
                reason: HoldReason::BudgetExhausted
            }
        ) && self.budget.assessment != BudgetAssessment::Reached
        {
            return Err(Error::BudgetMismatch);
        }
        Ok(())
    }
    fn complete_acks(&self) -> bool {
        self.acknowledgements.len() == self.inventory.participants.len()
            && self
                .acknowledgements
                .iter()
                .all(|a| matches!(a.status, AckStatus::SavedAndDrained { .. }))
    }
    fn observe_budget(&mut self, budget: &BudgetObservation) -> Result<()> {
        self.budget.successor(budget)?;
        self.budget = budget.clone();
        match budget.assessment {
            BudgetAssessment::Unknown => {
                self.phase = Phase::Holding {
                    reason: HoldReason::TimeUnknown,
                }
            }
            BudgetAssessment::Reached => {
                self.phase = Phase::Holding {
                    reason: HoldReason::BudgetExhausted,
                }
            }
            BudgetAssessment::Before { .. } => {
                if self.phase
                    == (Phase::Holding {
                        reason: HoldReason::TimeUnknown,
                    })
                {
                    self.phase = Phase::Holding {
                        reason: HoldReason::RevalidationRequired,
                    };
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationReference {
    operation_id: String,
    revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopeHead {
    preparation_owner_version: u8,
    binding: BootstrapBinding,
    revision: u64,
    owner_epoch: u64,
    freeze_epoch: u64,
    current: Option<OperationReference>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum OperationStatus {
    Active,
    Closed {
        #[serde(rename = "releaseReceiptSha256")]
        release_receipt_sha256: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationRecord {
    preparation_operation_version: u8,
    binding: BootstrapBinding,
    revision: u64,
    preparation: Preparation,
    status: OperationStatus,
}

/// Protected per-operation absence/presence DATA. Native reads must be in the
/// same TxR as the head and original budget; caller assertions are insufficient.
pub enum ExistingOperation<'a> {
    NoPrior,
    Active(&'a OperationRecord),
    Closed(&'a OperationRecord),
}
pub enum BudgetPresence<'a> {
    Absent,
    Present(&'a BudgetObservation),
}

/// Exact narrow write intent, never a storage transaction or native grant.
pub struct Mutation {
    pub next_head: ScopeHead,
    pub operation_write: OperationRecord,
}

/// Nonserialized reducer view of one head and one selected operation. Durable
/// closed operations remain individually addressable, with no lifetime array.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveOwnership {
    binding: BootstrapBinding,
    revision: u64,
    owner_epoch: u64,
    freeze_epoch: u64,
    operation_id: String,
    operation_revision: u64,
    current: Option<Preparation>,
    closed: Option<OperationRecord>,
}

/// These receipt hashes describe adapter observations. They cannot prove the
/// named facts, and must never be populated from untrusted request booleans.
pub struct FenceObservation {
    pub token: Token,
    pub inventory: Inventory,
    pub fence_receipt_sha256: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    NotCrossed,
    Crossed,
    Unknown,
}
pub struct StopObservation {
    pub token: Token,
    pub inventory: Inventory,
    pub boundary: Boundary,
    /// Exact one receipt per participant, after joined shutdown/drain and
    /// native descendant containment; an exit request is insufficient.
    pub stopped: Vec<ParticipantReceipt>,
    /// Identity of the protected pre-release safety snapshot: intact old data,
    /// no protected writes, and held fence. Completion names this SAME snapshot;
    /// the separate release receipt describes fresh post-release convergence.
    pub safety_receipt_sha256: String,
}
pub struct ParticipantReceipt {
    pub participant: Participant,
    pub receipt_sha256: String,
}
pub struct ReleaseObservation {
    pub safety: StopObservation,
    pub release_receipt_sha256: String,
}
pub struct RecoveryObservation {
    pub token: Token,
    pub inventory: Inventory,
    /// Physical fence reacquired/retained; every old executor is fenced.
    pub takeover_receipt_sha256: String,
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let raw = serde_json::to_vec(value).map_err(|_| Error::InvalidRecord)?;
    if raw.len() > LIVE_RECORD_MAXIMUM {
        return Err(Error::Capacity);
    }
    Document::parse(&raw, InputKind::Metadata)
        .and_then(|d| d.canonical_bytes())
        .map_err(|_| Error::InvalidRecord)
}
fn decode<T: serde::de::DeserializeOwned + Serialize>(raw: &[u8]) -> Result<T> {
    if raw.is_empty() || raw.len() > LIVE_RECORD_MAXIMUM {
        return Err(Error::Capacity);
    }
    let canonical = Document::parse(raw, InputKind::Metadata)
        .and_then(|d| d.canonical_bytes())
        .map_err(|_| Error::InvalidRecord)?;
    if canonical != raw {
        return Err(Error::InvalidRecord);
    }
    let value: T = serde_json::from_slice(raw).map_err(|_| Error::InvalidRecord)?;
    // Require explicit null members; serde alone allows omitted Option fields.
    if encode(&value)? != raw {
        return Err(Error::InvalidRecord);
    }
    Ok(value)
}
impl ScopeHead {
    /// DATA genesis only. Native initialization verifies protected immutable
    /// InitialOwnership and creates this once; missing reads never call this.
    pub fn initial(binding: &BootstrapBinding) -> Result<Self> {
        binding.validate().map_err(|_| Error::BindingMismatch)?;
        Ok(Self {
            preparation_owner_version: 1,
            binding: binding.clone(),
            revision: 0,
            owner_epoch: 0,
            freeze_epoch: 0,
            current: None,
        })
    }
    pub fn parse(raw: &[u8], binding: &BootstrapBinding) -> Result<Self> {
        let value: Self = decode(raw)?;
        value.validate()?;
        if &value.binding != binding {
            return Err(Error::BindingMismatch);
        }
        Ok(value)
    }
    pub fn record_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        encode(self)
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn current_operation_id(&self) -> Option<&str> {
        self.current.as_ref().map(|r| r.operation_id.as_str())
    }
    pub fn current_operation_revision(&self) -> Option<u64> {
        self.current.as_ref().map(|r| r.revision)
    }
    pub fn owner_epoch(&self) -> u64 {
        self.owner_epoch
    }
    pub fn freeze_epoch(&self) -> u64 {
        self.freeze_epoch
    }
    fn validate(&self) -> Result<()> {
        self.binding
            .validate()
            .map_err(|_| Error::BindingMismatch)?;
        if self.preparation_owner_version != 1
            || self.revision > SAFE_MAX
            || self.owner_epoch > self.revision
            || self.freeze_epoch > self.owner_epoch
            || (self.current.is_some() && self.owner_epoch == 0)
            || self.current.as_ref().is_some_and(|r| {
                !identifier(&r.operation_id) || !positive(r.revision) || r.revision > self.revision
            })
        {
            return Err(Error::InvalidRecord);
        }
        Ok(())
    }
}
impl OperationRecord {
    pub fn parse(raw: &[u8], binding: &BootstrapBinding, operation_id: &str) -> Result<Self> {
        let value: Self = decode(raw)?;
        value.validate()?;
        if &value.binding != binding {
            return Err(Error::BindingMismatch);
        }
        if value.operation_id() != operation_id {
            return Err(Error::OriginalIdentityMismatch);
        }
        Ok(value)
    }
    pub fn record_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        encode(self)
    }
    pub fn operation_id(&self) -> &str {
        &self.preparation.identity.operation_id
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn preparation(&self) -> &Preparation {
        &self.preparation
    }
    pub fn status(&self) -> &OperationStatus {
        &self.status
    }
    fn validate(&self) -> Result<()> {
        self.binding
            .validate()
            .map_err(|_| Error::BindingMismatch)?;
        self.preparation.validate()?;
        if self.preparation_operation_version != 1 || !positive(self.revision) {
            return Err(Error::InvalidRecord);
        }
        if let OperationStatus::Closed {
            release_receipt_sha256,
        } = &self.status
        {
            if self.preparation.phase != Phase::Releasing || !digest(release_receipt_sha256) {
                return Err(Error::InvalidRecord);
            }
        }
        Ok(())
    }
}
impl LiveOwnership {
    /// Compose actual adapter-read DATA. A protected operation and its original
    /// T26 budget must both exist or both genuinely be absent in this same TxR.
    /// Any partial/orphan/mismatched pair is corrupt, never NoPrior or healing.
    pub fn load(
        head: &ScopeHead,
        operation_id: &str,
        existing: ExistingOperation<'_>,
        budget: BudgetPresence<'_>,
    ) -> Result<Self> {
        head.validate()?;
        if !identifier(operation_id) {
            return Err(Error::InvalidRecord);
        }
        let mut value = Self {
            binding: head.binding.clone(),
            revision: head.revision,
            owner_epoch: head.owner_epoch,
            freeze_epoch: head.freeze_epoch,
            operation_id: operation_id.to_owned(),
            operation_revision: 0,
            current: None,
            closed: None,
        };
        match (existing, budget) {
            (ExistingOperation::NoPrior, BudgetPresence::Absent) => {
                if let Some(reference) = &head.current {
                    return Err(if reference.operation_id == operation_id {
                        Error::CorruptReadSet
                    } else {
                        Error::Occupied
                    });
                }
            }
            (ExistingOperation::Active(record), BudgetPresence::Present(budget)) => {
                record.validate().map_err(|_| Error::CorruptReadSet)?;
                if record.binding != head.binding
                    || record.operation_id() != operation_id
                    || record.status != OperationStatus::Active
                    || head.current.as_ref()
                        != Some(&OperationReference {
                            operation_id: operation_id.to_owned(),
                            revision: record.revision,
                        })
                    || record.preparation.owner_epoch != head.owner_epoch
                    || record
                        .preparation
                        .freeze_epoch
                        .is_some_and(|n| n != head.freeze_epoch)
                {
                    return Err(Error::CorruptReadSet);
                }
                record
                    .preparation
                    .budget
                    .successor(budget)
                    .map_err(|_| Error::CorruptReadSet)?;
                value.operation_revision = record.revision;
                value.current = Some(record.preparation.clone());
            }
            (ExistingOperation::Closed(record), BudgetPresence::Present(budget)) => {
                record.validate().map_err(|_| Error::CorruptReadSet)?;
                if record.binding != head.binding
                    || record.operation_id() != operation_id
                    || !matches!(record.status, OperationStatus::Closed { .. })
                    || head
                        .current
                        .as_ref()
                        .is_some_and(|r| r.operation_id == operation_id)
                    || record.preparation.owner_epoch > head.owner_epoch
                    || record
                        .preparation
                        .freeze_epoch
                        .is_some_and(|n| n > head.freeze_epoch)
                {
                    return Err(Error::CorruptReadSet);
                }
                if head.current.is_some() {
                    return Err(Error::Occupied);
                }
                record
                    .preparation
                    .budget
                    .successor(budget)
                    .map_err(|_| Error::CorruptReadSet)?;
                value.operation_revision = record.revision;
                value.closed = Some(record.clone());
            }
            _ => return Err(Error::CorruptReadSet),
        }
        value.validate()?;
        Ok(value)
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn current(&self) -> Option<&Preparation> {
        self.current.as_ref()
    }
    pub fn closed(&self) -> Option<&OperationRecord> {
        self.closed.as_ref()
    }
    pub fn token(&self) -> Option<Token> {
        self.current.as_ref().map(|p| Token {
            revision: self.revision,
            owner_epoch: p.owner_epoch,
            freeze_epoch: p.freeze_epoch,
        })
    }
    fn head(&self) -> ScopeHead {
        ScopeHead {
            preparation_owner_version: 1,
            binding: self.binding.clone(),
            revision: self.revision,
            owner_epoch: self.owner_epoch,
            freeze_epoch: self.freeze_epoch,
            current: self.current.as_ref().map(|_| OperationReference {
                operation_id: self.operation_id.clone(),
                revision: self.operation_revision,
            }),
        }
    }
    pub fn write_intent(&self) -> Result<Mutation> {
        self.validate()?;
        let operation_write = if let Some(preparation) = &self.current {
            OperationRecord {
                preparation_operation_version: 1,
                binding: self.binding.clone(),
                revision: self.operation_revision,
                preparation: preparation.clone(),
                status: OperationStatus::Active,
            }
        } else {
            self.closed.clone().ok_or(Error::WrongPhase)?
        };
        let next_head = self.head();
        next_head.record_bytes()?;
        operation_write.record_bytes()?;
        Ok(Mutation {
            next_head,
            operation_write,
        })
    }
    fn validate(&self) -> Result<()> {
        self.head().validate()?;
        if !identifier(&self.operation_id) || (self.current.is_some() && self.closed.is_some()) {
            return Err(Error::InvalidRecord);
        }
        if let Some(preparation) = &self.current {
            preparation.validate()?;
            if !positive(self.operation_revision)
                || preparation.identity.operation_id != self.operation_id
                || preparation.owner_epoch != self.owner_epoch
                || preparation
                    .freeze_epoch
                    .is_some_and(|n| n != self.freeze_epoch)
            {
                return Err(Error::CorruptReadSet);
            }
        } else if let Some(record) = &self.closed {
            record.validate()?;
            if record.binding != self.binding
                || record.operation_id() != self.operation_id
                || record.revision != self.operation_revision
                || !matches!(record.status, OperationStatus::Closed { .. })
            {
                return Err(Error::CorruptReadSet);
            }
        } else if self.operation_revision != 0 {
            return Err(Error::CorruptReadSet);
        }
        Ok(())
    }
    fn context(&self, context: &Context<'_>) -> Result<()> {
        self.validate()?;
        context.inventory.validate()?;
        if &self.binding != context.binding {
            return Err(Error::BindingMismatch);
        }
        if context.other_occupancy != OtherOccupancy::Clear {
            return Err(Error::Occupied);
        }
        if self
            .current
            .as_ref()
            .is_some_and(|p| p.inventory != *context.inventory)
        {
            return Err(Error::OriginalIdentityMismatch);
        }
        Ok(())
    }
    fn owned(&self, token: Token, context: &Context<'_>) -> Result<()> {
        self.context(context)?;
        if self.token() != Some(token) {
            return Err(Error::Stale);
        }
        Ok(())
    }
    fn finish(mut value: Self) -> Result<Self> {
        value.revision = next(value.revision)?;
        value.operation_revision = next(value.operation_revision)?;
        if let Some(record) = &mut value.closed {
            record.revision = value.operation_revision;
        }
        value.write_intent()?;
        Ok(value)
    }
    /// First acquisition or exact duplicate retry. A completed operation ID is
    /// never reusable, even with another Session or a new budget reference.
    pub fn acquire(
        &self,
        expected_revision: u64,
        context: &Context<'_>,
        identity: PreparationIdentity,
        budget: BudgetObservation,
    ) -> Result<Self> {
        self.context(context)?;
        if expected_revision != self.revision {
            return Err(Error::Stale);
        }
        identity.validate()?;
        budget.validate()?;
        if identity.operation_id != self.operation_id {
            return Err(Error::OriginalIdentityMismatch);
        }
        if let Some(current) = &self.current {
            if current.identity != identity {
                return Err(Error::Occupied);
            }
            if current.phase == Phase::Releasing {
                return if current.budget == budget {
                    Ok(self.clone())
                } else {
                    Err(Error::WrongPhase)
                };
            }
            let mut updated = self.clone();
            updated.current.as_mut().unwrap().observe_budget(&budget)?;
            if updated == *self {
                return Ok(updated);
            }
            return Self::finish(updated);
        }
        if self.closed.is_some() {
            return Err(Error::OriginalIdentityMismatch);
        }
        if !matches!(budget.assessment, BudgetAssessment::Before { .. }) {
            return Err(Error::BudgetUnavailable);
        }
        let mut updated = self.clone();
        updated.owner_epoch = next(self.owner_epoch)?;
        let freeze_epoch = if identity.purpose == Purpose::Preinstall {
            None
        } else {
            updated.freeze_epoch = next(self.freeze_epoch)?;
            Some(updated.freeze_epoch)
        };
        updated.current = Some(Preparation {
            identity,
            inventory: context.inventory.clone(),
            owner_epoch: updated.owner_epoch,
            freeze_epoch,
            budget,
            phase: Phase::Coordinating,
            acknowledgements: Vec::new(),
            fence_receipt_sha256: None,
            releasing_safety_sha256: None,
        });
        Self::finish(updated)
    }
    pub fn observe_budget(
        &self,
        token: Token,
        context: &Context<'_>,
        budget: &BudgetObservation,
    ) -> Result<Self> {
        self.owned(token, context)?;
        if self.current.as_ref().unwrap().phase == Phase::Releasing {
            return Err(Error::WrongPhase);
        }
        let mut updated = self.clone();
        updated.current.as_mut().unwrap().observe_budget(budget)?;
        if updated == *self {
            return Ok(updated);
        }
        Self::finish(updated)
    }
    /// Explicitly restart coordination under the STILL HELD native fence after
    /// a hold. The adapter transfers that fence to new owner/freeze epochs in
    /// the same protected publication, without releasing it. Old ACKs cannot
    /// satisfy the new cycle. Exhaustion remains terminal for this operation.
    pub fn resume(
        &self,
        context: &Context<'_>,
        observation: RecoveryObservation,
        budget: &BudgetObservation,
    ) -> Result<Self> {
        self.owned(observation.token, context)?;
        if observation.inventory != *context.inventory
            || !digest(&observation.takeover_receipt_sha256)
        {
            return Err(Error::OriginalIdentityMismatch);
        }
        let mut updated = self.clone();
        let current = updated.current.as_mut().unwrap();
        if !matches!(current.phase, Phase::Holding { .. }) {
            return Err(Error::WrongPhase);
        }
        current.observe_budget(budget)?;
        if !matches!(current.budget.assessment, BudgetAssessment::Before { .. }) {
            return Self::finish(updated);
        }
        updated.transfer_epochs()?;
        updated.current.as_mut().unwrap().phase = Phase::Coordinating;
        Self::finish(updated)
    }
    pub fn acknowledge(
        &self,
        token: Token,
        context: &Context<'_>,
        ack: ParticipantAck,
        budget: &BudgetObservation,
    ) -> Result<Self> {
        self.owned(token, context)?;
        ack.validate()?;
        let mut updated = self.clone();
        let current = updated.current.as_mut().unwrap();
        if current.phase != Phase::Coordinating || current.freeze_epoch.is_none() {
            return Err(Error::WrongPhase);
        }
        if !current.inventory.participants.contains(&ack.participant) {
            return Err(Error::OriginalIdentityMismatch);
        }
        if ack.owner_epoch != current.owner_epoch
            || Some(ack.freeze_epoch) != current.freeze_epoch
            || ack.inventory_revision != current.inventory.revision
            || ack.inventory_sha256 != current.inventory.sha256
        {
            return Err(Error::Stale);
        }
        current.observe_budget(budget)?;
        // A budget stop is durably returned instead of allowing late success.
        if current.phase != Phase::Coordinating {
            return Self::finish(updated);
        }
        match current
            .acknowledgements
            .binary_search_by(|a| a.participant.id.cmp(&ack.participant.id))
        {
            Ok(i) if current.acknowledgements[i] == ack => {
                return if updated == *self {
                    Ok(updated)
                } else {
                    Self::finish(updated)
                };
            }
            Ok(_) => return Err(Error::Stale),
            Err(i) => current.acknowledgements.insert(i, ack.clone()),
        }
        if !matches!(ack.status, AckStatus::SavedAndDrained { .. }) {
            current.phase = Phase::Holding {
                reason: HoldReason::WriterRefused,
            };
        }
        Self::finish(updated)
    }
    pub fn mark_frozen(
        &self,
        context: &Context<'_>,
        observation: FenceObservation,
        budget: &BudgetObservation,
    ) -> Result<Self> {
        self.owned(observation.token, context)?;
        if observation.inventory != *context.inventory || !digest(&observation.fence_receipt_sha256)
        {
            return Err(Error::OriginalIdentityMismatch);
        }
        let mut updated = self.clone();
        let current = updated.current.as_mut().unwrap();
        if current.phase != Phase::Coordinating || current.freeze_epoch.is_none() {
            return Err(Error::WrongPhase);
        }
        current.observe_budget(budget)?;
        if current.phase != Phase::Coordinating {
            return Self::finish(updated);
        }
        if !current.complete_acks() {
            return Err(Error::Incomplete);
        }
        current.fence_receipt_sha256 = Some(observation.fence_receipt_sha256);
        current.phase = Phase::Frozen;
        Self::finish(updated)
    }
    fn stopped(&self, context: &Context<'_>, observation: &StopObservation) -> Result<()> {
        self.owned(observation.token, context)?;
        if observation.boundary != Boundary::NotCrossed {
            return Err(Error::UnsafeBoundary);
        }
        if observation.inventory != *context.inventory
            || !digest(&observation.safety_receipt_sha256)
        {
            return Err(Error::OriginalIdentityMismatch);
        }
        let expected = if self.current.as_ref().unwrap().identity.purpose == Purpose::Preinstall {
            &[][..]
        } else {
            context.inventory.participants.as_slice()
        };
        if observation.stopped.len() != expected.len() {
            return Err(Error::Incomplete);
        }
        for (receipt, expected) in observation.stopped.iter().zip(expected) {
            if receipt.participant != *expected || !digest(&receipt.receipt_sha256) {
                return Err(Error::Incomplete);
            }
        }
        Ok(())
    }
    /// Publish this transition BEFORE touching the physical writer fence.
    pub fn begin_cancel(
        &self,
        context: &Context<'_>,
        observation: StopObservation,
    ) -> Result<Self> {
        self.stopped(context, &observation)?;
        let mut updated = self.clone();
        let current = updated.current.as_mut().unwrap();
        if current.phase == Phase::Releasing {
            return Err(Error::WrongPhase);
        }
        current.phase = Phase::Releasing;
        current.releasing_safety_sha256 = Some(observation.safety_receipt_sha256);
        Self::finish(updated)
    }
    /// Only actual post-release convergence/readback may close the record.
    pub fn complete_cancel(
        &self,
        context: &Context<'_>,
        observation: ReleaseObservation,
    ) -> Result<Self> {
        self.stopped(context, &observation.safety)?;
        if self.current.as_ref().unwrap().phase != Phase::Releasing {
            return Err(Error::WrongPhase);
        }
        if self
            .current
            .as_ref()
            .unwrap()
            .releasing_safety_sha256
            .as_deref()
            != Some(observation.safety.safety_receipt_sha256.as_str())
        {
            return Err(Error::OriginalIdentityMismatch);
        }
        if !digest(&observation.release_receipt_sha256) {
            return Err(Error::InvalidRecord);
        }
        let mut updated = self.clone();
        let old = updated.current.take().unwrap();
        updated.closed = Some(OperationRecord {
            preparation_operation_version: 1,
            binding: updated.binding.clone(),
            revision: updated.operation_revision,
            preparation: old,
            status: OperationStatus::Closed {
                release_receipt_sha256: observation.release_receipt_sha256,
            },
        });
        Self::finish(updated)
    }
    /// Change the observation generation while retaining native custody. The
    /// adapter must transfer its actual fence/admission bindings atomically;
    /// these new DATA epochs alone do not fence old executors.
    fn transfer_epochs(&mut self) -> Result<()> {
        self.owner_epoch = next(self.owner_epoch)?;
        let current = self.current.as_mut().ok_or(Error::WrongPhase)?;
        current.owner_epoch = self.owner_epoch;
        if current.freeze_epoch.is_some() {
            self.freeze_epoch = next(self.freeze_epoch)?;
            current.freeze_epoch = Some(self.freeze_epoch);
        }
        current.acknowledgements.clear();
        current.fence_receipt_sha256 = None;
        current.releasing_safety_sha256 = None;
        Ok(())
    }
    /// No new preparation starts here. Native old-executor fencing precedes the
    /// epoch transfer; the owner remains held until fresh safety observations.
    pub fn recover(
        &self,
        context: &Context<'_>,
        observation: RecoveryObservation,
        budget: &BudgetObservation,
    ) -> Result<Self> {
        self.owned(observation.token, context)?;
        if observation.inventory != *context.inventory
            || !digest(&observation.takeover_receipt_sha256)
        {
            return Err(Error::OriginalIdentityMismatch);
        }
        let mut updated = self.clone();
        updated.current.as_mut().unwrap().observe_budget(budget)?;
        updated.transfer_epochs()?;
        updated.current.as_mut().unwrap().phase = Phase::Holding {
            reason: HoldReason::Recovery,
        };
        Self::finish(updated)
    }
}

#[cfg(test)]
mod tests;
