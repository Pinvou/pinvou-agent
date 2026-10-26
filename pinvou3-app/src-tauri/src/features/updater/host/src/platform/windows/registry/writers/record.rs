//! Closed canonical DATA. Native authority is minted only in the parent owner.
use super::*;
use crate::identity::{LogonId, ProcessSelector};
use pinvou_upgrade_ownership::live::{BudgetOrigin, Participant, PreparationIdentity};
use serde::{Deserialize, Serialize};

pub(super) const MAX_ACTIVE: usize = pinvou_upgrade_ownership::live::MAX_PARTICIPANTS;
const MAX_RECORD: usize = 65_536;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Genesis {
    version: u8,
    pub(super) scope: String,
    pub(super) genesis: String,
    bootstrap_sha256: String,
}
impl Genesis {
    pub(super) fn new(host: &HostRegistration, bootstrap: &[u8]) -> Result<Self> {
        Ok(Self {
            version: 1,
            scope: host.installation_scope_id.clone(),
            genesis: host.genesis_manifest_sha256.clone(),
            bootstrap_sha256: json::sha256(bootstrap),
        })
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Entry {
    pub(super) id: String,
    pub(super) revision: u64,
    pub(super) sha256: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Head {
    version: u8,
    pub(super) genesis_sha256: String,
    pub(super) revision: u64,
    pub(super) previous_sha256: Option<String>,
    pub(super) active: Vec<Entry>,
    pub(super) pending_release: Vec<Entry>,
    pub(super) changed: Option<Entry>,
    pub(super) released: bool,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Identity {
    pub(super) scope: String,
    pub(super) genesis: String,
    pub(super) service_sid: String,
    pub(super) origin_host_epoch: u64,
    pub(super) job_name: String,
    pub(super) original: ProcessSelector,
    pub(super) user_sid: String,
    pub(super) logon: LogonId,
    pub(super) session_id: u32,
    pub(super) app_image_sha256: String,
    pub(super) integrity: u32,
    pub(super) elevated: bool,
    pub(super) administrator: bool,
    pub(super) interactive: bool,
    pub(super) network: bool,
    pub(super) app_container: bool,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Drain {
    pub(super) identity: PreparationIdentity,
    pub(super) owner_epoch: u64,
    pub(super) freeze_epoch: u64,
    pub(super) inventory_revision: u64,
    pub(super) inventory_sha256: String,
    pub(super) participant: Participant,
    pub(super) budget_origin: BudgetOrigin,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Phase {
    Reserved,
    Admitted,
    Draining,
    Closed,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Row {
    version: u8,
    pub(super) id: String,
    pub(super) identity: Identity,
    pub(super) revision: u64,
    pub(super) previous_sha256: Option<String>,
    pub(super) first_record_sha256: Option<String>,
    pub(super) phase: Phase,
    pub(super) drain: Option<Drain>,
}
pub(super) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let raw = json::encode(value, false)?;
    if raw.len() > MAX_RECORD {
        return Err(Error::Unavailable);
    }
    Ok(raw)
}
pub(super) fn decode<T: serde::de::DeserializeOwned + Serialize>(raw: &[u8]) -> Result<T> {
    if raw.is_empty() || raw.len() > MAX_RECORD {
        return Err(Error::StateCorrupt);
    }
    let value: T = json::decode(raw, false)?;
    if encode(&value)? != raw {
        return Err(Error::StateCorrupt);
    }
    Ok(value)
}
fn positive(n: u64) -> bool {
    n > 0 && n <= json::SAFE_MAX
}
fn sid(value: &str, service: bool) -> bool {
    if service {
        return value.strip_prefix("S-1-5-80-").is_some_and(|s| {
            let p: Vec<_> = s.split('-').collect();
            p.len() == 5
                && p.iter()
                    .all(|n| n.parse::<u32>().ok().is_some_and(|v| v.to_string() == *n))
        });
    }
    if value.len() > 192 {
        return false;
    }
    let Some(body) = value.strip_prefix("S-1-") else {
        return false;
    };
    let parts: Vec<_> = body.split('-').collect();
    (2..=16).contains(&parts.len())
        && parts[0]
            .parse::<u64>()
            .ok()
            .is_some_and(|n| n <= 0xffff_ffff_ffff && n.to_string() == parts[0])
        && parts[1..]
            .iter()
            .all(|p| p.parse::<u32>().ok().is_some_and(|n| n.to_string() == *p))
}
impl Identity {
    fn validate(&self, genesis: &Genesis, id: &str) -> Result<()> {
        if self.scope != genesis.scope
            || self.genesis != genesis.genesis
            || !sid(&self.service_sid, true)
            || !sid(&self.user_sid, false)
            || !positive(self.origin_host_epoch)
            || self.session_id == 0
            || !json::digest(&self.app_image_sha256)
            || self.integrity < 0x2000
            || !self.interactive
            || self.network
            || self.app_container
            || self.user_sid == "S-1-5-18"
            || self.job_name != format!("Global\\PinvouWriter-{id}")
        {
            return Err(Error::StateCorrupt);
        }
        self.original.validate().map_err(|_| Error::StateCorrupt)
    }
    pub(super) fn participant(&self, id: &str) -> Result<Participant> {
        if self.job_name != format!("Global\\PinvouWriter-{id}") {
            return Err(Error::StateCorrupt);
        }
        let raw = encode(self)?;
        let mut participant = b"pinvou-writer-participant-v1\0".to_vec();
        participant.extend_from_slice(&raw);
        // A participant may appear in coordination DATA. Do not expose the
        // private job-name suffix as its public-facing identifier.
        Ok(Participant {
            id: json::sha256(&participant),
            instance_sha256: json::sha256(&raw),
        })
    }
}
impl Row {
    pub(super) fn reserved(id: String, identity: Identity) -> Self {
        Self {
            version: 1,
            id,
            identity,
            revision: 1,
            previous_sha256: None,
            first_record_sha256: None,
            phase: Phase::Reserved,
            drain: None,
        }
    }
    pub(super) fn validate(&self, genesis: &Genesis) -> Result<()> {
        if self.version != 1
            || !json::digest(&self.id)
            || !positive(self.revision)
            || (self.revision == 1) != self.previous_sha256.is_none()
            || (self.revision == 1) != self.first_record_sha256.is_none()
            || self
                .previous_sha256
                .as_ref()
                .is_some_and(|v| !json::digest(v))
            || self
                .first_record_sha256
                .as_ref()
                .is_some_and(|v| !json::digest(v))
            || (self.revision == 1 && self.phase != Phase::Reserved)
            || (self.phase == Phase::Draining && self.drain.is_none())
            || (matches!(self.phase, Phase::Reserved | Phase::Admitted) && self.drain.is_some())
            || match self.phase {
                Phase::Reserved => self.revision != 1,
                Phase::Admitted => self.revision != 2,
                Phase::Draining => self.revision < 3,
                Phase::Closed => {
                    self.revision < 2
                        || (self.revision == 3 && self.drain.is_some())
                        || (self.revision >= 4 && self.drain.is_none())
                }
            }
        {
            return Err(Error::StateCorrupt);
        }
        self.identity.validate(genesis, &self.id)?;
        if let Some(d) = &self.drain {
            if !positive(d.owner_epoch)
                || !positive(d.freeze_epoch)
                || d.freeze_epoch > d.owner_epoch
                || !positive(d.inventory_revision)
                || !json::digest(&d.inventory_sha256)
                || !json::identifier(&d.identity.operation_id)
                || !json::identifier(&d.identity.original_session_id)
                || !json::digest(&d.identity.original_session_sha256)
                || !json::digest(&d.identity.request_lineage_sha256)
                || d.participant != self.identity.participant(&self.id)?
                || !json::digest(&d.budget_origin.window_key_sha256)
                || d.budget_origin.first_lower_ms > d.budget_origin.first_upper_ms
                || d.budget_origin.first_upper_ms > json::SAFE_MAX
                || !positive(d.budget_origin.original_duration_ms)
                || d.budget_origin
                    .first_lower_ms
                    .checked_add(d.budget_origin.original_duration_ms)
                    != Some(d.budget_origin.original_deadline_ms)
                || d.budget_origin.original_deadline_ms > json::SAFE_MAX
                || d.budget_origin.original_session_cutoff_ms > json::SAFE_MAX
            {
                return Err(Error::StateCorrupt);
            }
        }
        Ok(())
    }
    pub(super) fn advance(&self, raw: &[u8], phase: Phase, drain: Option<Drain>) -> Result<Self> {
        let mut row = self.clone();
        row.revision = row
            .revision
            .checked_add(1)
            .filter(|n| positive(*n))
            .ok_or(Error::Unavailable)?;
        row.previous_sha256 = Some(json::sha256(raw));
        row.phase = phase;
        row.drain = drain;
        row.first_record_sha256 = if self.revision == 1 {
            Some(json::sha256(raw))
        } else {
            self.first_record_sha256.clone()
        };
        self.successor(&row, raw)?;
        Ok(row)
    }
    pub(super) fn successor(&self, later: &Self, raw: &[u8]) -> Result<()> {
        let allowed = matches!(
            (self.phase, later.phase),
            (Phase::Reserved, Phase::Admitted | Phase::Closed)
                | (Phase::Admitted, Phase::Draining | Phase::Closed)
                | (Phase::Draining, Phase::Draining | Phase::Closed)
        );
        let expected_first = if self.revision == 1 {
            Some(json::sha256(raw))
        } else {
            self.first_record_sha256.clone()
        };
        if !allowed
            || later.id != self.id
            || later.identity != self.identity
            || later.revision != self.revision.checked_add(1).ok_or(Error::StateCorrupt)?
            || later.previous_sha256 != Some(json::sha256(raw))
            || later.first_record_sha256 != expected_first
            || (self.phase == Phase::Draining
                && later.phase == Phase::Closed
                && later.drain != self.drain)
        {
            return Err(Error::StateCorrupt);
        }
        if self.phase == Phase::Draining && later.phase == Phase::Draining {
            self.drain
                .as_ref()
                .ok_or(Error::StateCorrupt)?
                .rebind_to(later.drain.as_ref().ok_or(Error::StateCorrupt)?)
                .map_err(|_| Error::StateCorrupt)?;
        }
        Ok(())
    }
    /// DATA transition used only after the storage owner derives the observed
    /// binding from actual same-TxR preparation. No supplied DTO grants rebind.
    pub(super) fn draining_for_observation(&self, raw: &[u8], observed: Drain) -> Result<Self> {
        match self.phase {
            Phase::Admitted => self.advance(raw, Phase::Draining, Some(observed)),
            Phase::Draining => {
                let prior = self.drain.as_ref().ok_or(Error::StateCorrupt)?;
                if prior == &observed {
                    return Ok(self.clone());
                }
                prior.rebind_to(&observed)?;
                self.advance(raw, Phase::Draining, Some(observed))
            }
            _ => Err(Error::ScopeConflict),
        }
    }
    pub(super) fn require_current_drain(&self, observed: &Drain) -> Result<()> {
        if self.phase != Phase::Draining || self.drain.as_ref() != Some(observed) {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
    pub(super) fn entry(&self, raw: &[u8]) -> Entry {
        Entry {
            id: self.id.clone(),
            revision: self.revision,
            sha256: json::sha256(raw),
        }
    }
}
impl Drain {
    fn rebind_to(&self, later: &Self) -> Result<()> {
        if later.identity != self.identity
            || later.inventory_revision != self.inventory_revision
            || later.inventory_sha256 != self.inventory_sha256
            || later.participant != self.participant
            || later.budget_origin != self.budget_origin
            || later.owner_epoch <= self.owner_epoch
            || later.freeze_epoch <= self.freeze_epoch
        {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(super) fn census_test_data() -> (Genesis, Row) {
    tests::data()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::FileTime;
    pub(super) fn data() -> (Genesis, Row) {
        let g = Genesis {
            version: 1,
            scope: "1".repeat(64),
            genesis: "2".repeat(64),
            bootstrap_sha256: "3".repeat(64),
        };
        let id = "a".repeat(64);
        let identity = Identity {
            scope: g.scope.clone(),
            genesis: g.genesis.clone(),
            service_sid: "S-1-5-80-1-2-3-4-5".into(),
            origin_host_epoch: 1,
            job_name: format!("Global\\PinvouWriter-{id}"),
            original: ProcessSelector {
                process_id: 123,
                creation_file_time: FileTime::from_native(1, 2),
            },
            user_sid: "S-1-5-21-1-2-3-1000".into(),
            logon: LogonId { high: 1, low: 2 },
            session_id: 1,
            app_image_sha256: "4".repeat(64),
            integrity: 0x2000,
            elevated: false,
            administrator: false,
            interactive: true,
            network: false,
            app_container: false,
        };
        (g, Row::reserved(id, identity))
    }
    #[test]
    fn canonical_closed_schema_and_immutable_identity_are_required() {
        let (g, first) = data();
        first.validate(&g).unwrap();
        let raw = encode(&first).unwrap();
        assert!(decode::<Row>(&raw).unwrap() == first);
        let mut omitted: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        omitted.as_object_mut().unwrap().remove("drain");
        assert!(decode::<Row>(&json::encode(&omitted, false).unwrap()).is_err());
        let mut changed = first.advance(&raw, Phase::Admitted, None).unwrap();
        changed.identity.origin_host_epoch = 2;
        assert!(first.successor(&changed, &raw).is_err());
        let mut changed = first.clone();
        changed.identity.user_sid = "S-1-5--21".into();
        assert!(changed.validate(&g).is_err());
    }
    #[test]
    fn closing_one_writer_does_not_exhaust_future_active_capacity() {
        let (g, first) = data();
        let genesis = encode(&g).unwrap();
        let initial = Head::initial(&genesis);
        let raw = encode(&first).unwrap();
        let one = initial
            .advance(&encode(&initial).unwrap(), &first, &raw)
            .unwrap();
        let admitted = first.advance(&raw, Phase::Admitted, None).unwrap();
        let admitted_raw = encode(&admitted).unwrap();
        let two = one
            .advance(&encode(&one).unwrap(), &admitted, &admitted_raw)
            .unwrap();
        let closed = admitted
            .advance(&admitted_raw, Phase::Closed, None)
            .unwrap();
        let three = two
            .advance(&encode(&two).unwrap(), &closed, &encode(&closed).unwrap())
            .unwrap();
        assert!(three.active.is_empty());
        let (_, mut different) = data();
        different.id = "b".repeat(64);
        different.identity.job_name = format!("Global\\PinvouWriter-{}", different.id);
        let four = three
            .advance(
                &encode(&three).unwrap(),
                &different,
                &encode(&different).unwrap(),
            )
            .unwrap();
        assert_eq!(four.active.len(), 1);
        assert!(
            three
                .advance(&encode(&three).unwrap(), &closed, &encode(&closed).unwrap())
                .is_err()
        );
    }
    #[test]
    fn participant_identity_excludes_phase_and_revision_but_includes_native_original() {
        let (_, first) = data();
        let p = first.identity.participant(&first.id).unwrap();
        let admitted = first
            .advance(&encode(&first).unwrap(), Phase::Admitted, None)
            .unwrap();
        assert!(admitted.identity.participant(&admitted.id).unwrap() == p);
        let mut changed = first.identity;
        changed.logon.low += 1;
        assert!(changed.participant(&first.id).unwrap() != p);
    }
    #[test]
    fn closed_retirement_survives_roundtrip_and_only_exact_pending_entry_can_leave() {
        // Pure journal DATA: no native zero, keeper ACK or grant is fabricated.
        let (genesis, reserved) = data();
        let initial = Head::initial(&encode(&genesis).unwrap());
        let reserved_raw = encode(&reserved).unwrap();
        let active = initial
            .advance(&encode(&initial).unwrap(), &reserved, &reserved_raw)
            .unwrap();
        let closed = reserved
            .advance(&reserved_raw, Phase::Closed, None)
            .unwrap();
        let closed_raw = encode(&closed).unwrap();
        let pending = active
            .advance(&encode(&active).unwrap(), &closed, &closed_raw)
            .unwrap();
        let restored: Head = decode(&encode(&pending).unwrap()).unwrap();
        restored.validate(&encode(&genesis).unwrap()).unwrap();
        assert!(restored.active.is_empty());
        assert_eq!(restored.pending_release.len(), 1);
        let entry = closed.entry(&closed_raw);
        let mut wrong = entry.clone();
        wrong.sha256 = "f".repeat(64);
        assert!(
            restored
                .release(&encode(&restored).unwrap(), &wrong)
                .is_err()
        );
        let released = restored
            .release(&encode(&restored).unwrap(), &entry)
            .unwrap();
        restored
            .successor(&released, &encode(&restored).unwrap(), &closed, &closed_raw)
            .unwrap();
        assert!(released.pending_release.is_empty() && released.active.is_empty());
        assert!(
            released
                .release(&encode(&released).unwrap(), &entry)
                .is_err()
        );
        let mut erased = restored.clone();
        erased.pending_release.clear();
        assert!(
            restored
                .successor(&erased, &encode(&restored).unwrap(), &closed, &closed_raw)
                .is_err()
        );
    }
    #[test]
    fn pending_custody_counts_toward_capacity_until_release_without_lifetime_quota() {
        let (genesis, fixture) = data();
        let mut head = Head::initial(&encode(&genesis).unwrap());
        let mut first_closed = None;
        for index in 0..MAX_ACTIVE {
            let mut row = fixture.clone();
            row.id = format!("{index:064x}");
            row.identity.job_name = format!("Global\\PinvouWriter-{}", row.id);
            let raw = encode(&row).unwrap();
            head = head.advance(&encode(&head).unwrap(), &row, &raw).unwrap();
            let closed = row.advance(&raw, Phase::Closed, None).unwrap();
            let raw = encode(&closed).unwrap();
            head = head
                .advance(&encode(&head).unwrap(), &closed, &raw)
                .unwrap();
            if first_closed.is_none() {
                first_closed = Some(closed.entry(&raw));
            }
        }
        assert_eq!(head.pending_release.len(), MAX_ACTIVE);
        assert!(
            head.advance(
                &encode(&head).unwrap(),
                &fixture,
                &encode(&fixture).unwrap()
            )
            .is_err()
        );
        head = head
            .release(&encode(&head).unwrap(), &first_closed.unwrap())
            .unwrap();
        // Repeated complete lifecycles reuse the freed concurrent slot; the
        // retained history/global revision never imposes a lifetime quota.
        for index in 1000..1300 {
            let mut row = fixture.clone();
            row.id = format!("{index:064x}");
            row.identity.job_name = format!("Global\\PinvouWriter-{}", row.id);
            let raw = encode(&row).unwrap();
            head = head.advance(&encode(&head).unwrap(), &row, &raw).unwrap();
            let closed = row.advance(&raw, Phase::Closed, None).unwrap();
            let raw = encode(&closed).unwrap();
            head = head
                .advance(&encode(&head).unwrap(), &closed, &raw)
                .unwrap();
            head = head
                .release(&encode(&head).unwrap(), &closed.entry(&raw))
                .unwrap();
            head.validate(&encode(&genesis).unwrap()).unwrap();
        }
        assert_eq!(head.pending_release.len(), MAX_ACTIVE - 1);
    }
    fn drain_context(row: &Row, epoch: u64) -> Drain {
        Drain {
            identity: PreparationIdentity {
                operation_id: "operation:one".into(),
                original_session_id: "session:one".into(),
                original_session_sha256: "5".repeat(64),
                request_lineage_sha256: "6".repeat(64),
                purpose: pinvou_upgrade_ownership::live::Purpose::Install,
            },
            owner_epoch: epoch,
            freeze_epoch: epoch,
            inventory_revision: 1,
            inventory_sha256: "7".repeat(64),
            participant: row.identity.participant(&row.id).unwrap(),
            budget_origin: BudgetOrigin {
                window_key_sha256: "8".repeat(64),
                first_lower_ms: 10,
                first_upper_ms: 11,
                original_duration_ms: 100,
                original_deadline_ms: 110,
                original_session_cutoff_ms: 1000,
            },
        }
    }
    #[test]
    fn same_writer_rebinds_new_owner_epochs_and_only_current_drain_can_close() {
        // Context/row DATA only. Storage derives these observations from the
        // actual native read set; this test mints no native grant or zero proof.
        let (genesis, reserved) = data();
        let first_raw = encode(&reserved).unwrap();
        let admitted = reserved.advance(&first_raw, Phase::Admitted, None).unwrap();
        let old_context = drain_context(&admitted, 1);
        let draining = admitted
            .draining_for_observation(&encode(&admitted).unwrap(), old_context.clone())
            .unwrap();
        let current_context = drain_context(&admitted, 2);
        assert!(draining.require_current_drain(&current_context).is_err());
        let rebound = draining
            .draining_for_observation(&encode(&draining).unwrap(), current_context.clone())
            .unwrap();
        rebound.validate(&genesis).unwrap();
        assert_eq!(rebound.revision, 4);
        assert!(rebound.identity == reserved.identity);
        assert_eq!(rebound.first_record_sha256, Some(json::sha256(&first_raw)));
        assert!(rebound.require_current_drain(&old_context).is_err());
        rebound.require_current_drain(&current_context).unwrap();
        let closed = rebound
            .advance(
                &encode(&rebound).unwrap(),
                Phase::Closed,
                Some(current_context.clone()),
            )
            .unwrap();
        closed.validate(&genesis).unwrap();
        assert_eq!(closed.revision, 5);
        // ACK/budget-head revision changes carry no drain fields: an identical
        // observed context is a no-op rather than a fabricated epoch/CAS update.
        let same = rebound
            .draining_for_observation(&encode(&rebound).unwrap(), current_context)
            .unwrap();
        assert!(same == rebound);
        // Several legitimate resumes do not exhaust a fixed version quota.
        let mut later = rebound;
        for epoch in 3..=10 {
            later = later
                .draining_for_observation(&encode(&later).unwrap(), drain_context(&admitted, epoch))
                .unwrap();
            later.validate(&genesis).unwrap();
        }
        assert!(later.revision > 4);
        assert_eq!(later.first_record_sha256, Some(json::sha256(&first_raw)));
    }
    #[test]
    fn draining_rebind_refuses_foreign_identity_inventory_cutoff_or_partial_epoch_transfer() {
        let (_, reserved) = data();
        let admitted = reserved
            .advance(&encode(&reserved).unwrap(), Phase::Admitted, None)
            .unwrap();
        let old = drain_context(&admitted, 1);
        let draining = admitted
            .draining_for_observation(&encode(&admitted).unwrap(), old)
            .unwrap();
        let current = drain_context(&admitted, 2);
        let mut changed = current.clone();
        changed.identity.operation_id = "different-operation".into();
        assert!(
            draining
                .draining_for_observation(&encode(&draining).unwrap(), changed)
                .is_err()
        );
        let mut changed = current.clone();
        changed.inventory_revision += 1;
        assert!(
            draining
                .draining_for_observation(&encode(&draining).unwrap(), changed)
                .is_err()
        );
        let mut changed = current.clone();
        changed.participant.instance_sha256 = "9".repeat(64);
        assert!(
            draining
                .draining_for_observation(&encode(&draining).unwrap(), changed)
                .is_err()
        );
        let mut changed = current.clone();
        changed.budget_origin.original_session_cutoff_ms += 1;
        assert!(
            draining
                .draining_for_observation(&encode(&draining).unwrap(), changed)
                .is_err()
        );
        let mut changed = current;
        changed.freeze_epoch = 1;
        assert!(
            draining
                .draining_for_observation(&encode(&draining).unwrap(), changed)
                .is_err()
        );
    }
}
impl Head {
    pub(super) fn initial(genesis_raw: &[u8]) -> Self {
        Self {
            version: 1,
            genesis_sha256: json::sha256(genesis_raw),
            revision: 1,
            previous_sha256: None,
            active: Vec::new(),
            pending_release: Vec::new(),
            changed: None,
            released: false,
        }
    }
    pub(super) fn validate(&self, genesis_raw: &[u8]) -> Result<()> {
        if self.version != 1
            || self.genesis_sha256 != json::sha256(genesis_raw)
            || !positive(self.revision)
            || (self.revision == 1) != self.previous_sha256.is_none()
            || (self.revision == 1) != self.changed.is_none()
            || self
                .previous_sha256
                .as_ref()
                .is_some_and(|v| !json::digest(v))
            || self.active.len() + self.pending_release.len() > MAX_ACTIVE
            || (self.revision == 1
                && (!self.active.is_empty() || !self.pending_release.is_empty() || self.released))
            || self.active.windows(2).any(|p| p[0].id >= p[1].id)
            || self.pending_release.windows(2).any(|p| p[0].id >= p[1].id)
            || self
                .pending_release
                .iter()
                .any(|p| self.active.iter().any(|a| a.id == p.id))
            || self
                .active
                .iter()
                .chain(self.pending_release.iter())
                .chain(self.changed.iter())
                .any(|r| !json::digest(&r.id) || !positive(r.revision) || !json::digest(&r.sha256))
        {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }
    pub(super) fn advance(&self, raw: &[u8], row: &Row, row_raw: &[u8]) -> Result<Self> {
        if self.pending_release.iter().any(|entry| entry.id == row.id) {
            return Err(Error::StateCorrupt);
        }
        match self.active.iter().find(|entry| entry.id == row.id) {
            None if row.revision == 1 && row.phase == Phase::Reserved => (),
            Some(entry)
                if row.revision == entry.revision.checked_add(1).ok_or(Error::StateCorrupt)?
                    && row.previous_sha256.as_ref() == Some(&entry.sha256) =>
            {
                ()
            }
            _ => return Err(Error::StateCorrupt),
        }
        let mut head = self.clone();
        head.revision = head
            .revision
            .checked_add(1)
            .filter(|n| positive(*n))
            .ok_or(Error::Unavailable)?;
        head.previous_sha256 = Some(json::sha256(raw));
        let entry = row.entry(row_raw);
        head.changed = Some(entry.clone());
        head.released = false;
        head.active.retain(|r| r.id != row.id);
        if row.phase != Phase::Closed {
            head.active.push(entry);
        } else {
            head.pending_release.push(entry);
        }
        head.active.sort_by(|a, b| a.id.cmp(&b.id));
        head.pending_release.sort_by(|a, b| a.id.cmp(&b.id));
        if head.active.len() + head.pending_release.len() > MAX_ACTIVE {
            return Err(Error::Busy);
        }
        Ok(head)
    }
    pub(super) fn release(&self, raw: &[u8], entry: &Entry) -> Result<Self> {
        if !self.pending_release.contains(entry) {
            return Err(Error::ScopeConflict);
        }
        let mut head = self.clone();
        head.revision = head
            .revision
            .checked_add(1)
            .filter(|n| positive(*n))
            .ok_or(Error::Unavailable)?;
        head.previous_sha256 = Some(json::sha256(raw));
        head.changed = Some(entry.clone());
        head.released = true;
        head.pending_release.retain(|r| r.id != entry.id);
        Ok(head)
    }
    pub(super) fn successor(
        &self,
        later: &Self,
        raw: &[u8],
        row: &Row,
        row_raw: &[u8],
    ) -> Result<()> {
        let expected = if later.released {
            if row.phase != Phase::Closed {
                return Err(Error::StateCorrupt);
            }
            self.release(raw, &row.entry(row_raw))?
        } else {
            self.advance(raw, row, row_raw)?
        };
        if expected != *later {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }
}
