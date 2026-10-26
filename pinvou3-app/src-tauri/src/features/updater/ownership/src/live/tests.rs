use super::*;
use serde_json::{Value, json};

fn hash(c: char) -> String {
    c.to_string().repeat(64)
}
fn binding() -> BootstrapBinding {
    BootstrapBinding::new(
        &hash('1'),
        "pinvou",
        "agent",
        "windows-x86_64-nsis-perMachine",
        "production",
        "stable-installation",
        &hash('2'),
    )
    .unwrap()
}
fn inventory() -> Inventory {
    Inventory {
        revision: 1,
        sha256: hash('3'),
        participants: vec![
            Participant {
                id: "background:one".into(),
                instance_sha256: hash('4'),
            },
            Participant {
                id: "user:disconnected".into(),
                instance_sha256: hash('5'),
            },
        ],
    }
}
fn identity(purpose: Purpose) -> PreparationIdentity {
    PreparationIdentity {
        operation_id: "operation:1".into(),
        original_session_id: "session:original".into(),
        original_session_sha256: hash('6'),
        request_lineage_sha256: hash('7'),
        purpose,
    }
}
fn budget() -> BudgetObservation {
    BudgetObservation {
        origin: BudgetOrigin {
            window_key_sha256: hash('8'),
            first_lower_ms: 100,
            first_upper_ms: 101,
            original_duration_ms: 120_000,
            original_deadline_ms: 120_100,
            original_session_cutoff_ms: 100_000,
        },
        window_revision: 1,
        spent_max_ms: 1,
        session_cutoff_ms: 100_000,
        assessment: BudgetAssessment::Before {
            remaining_ms: 99_899,
        },
    }
}
fn context<'a>(binding: &'a BootstrapBinding, inventory: &'a Inventory) -> Context<'a> {
    Context {
        binding,
        inventory,
        other_occupancy: OtherOccupancy::Clear,
    }
}
fn empty(operation_id: &str) -> LiveOwnership {
    LiveOwnership::load(
        &ScopeHead::initial(&binding()).unwrap(),
        operation_id,
        ExistingOperation::NoPrior,
        BudgetPresence::Absent,
    )
    .unwrap()
}
fn acquire(purpose: Purpose) -> LiveOwnership {
    let b = binding();
    let i = inventory();
    empty("operation:1")
        .acquire(0, &context(&b, &i), identity(purpose), budget())
        .unwrap()
}
fn acknowledgement(participant: &Participant) -> ParticipantAck {
    ParticipantAck {
        owner_epoch: 1,
        freeze_epoch: 1,
        inventory_revision: 1,
        inventory_sha256: hash('3'),
        participant: participant.clone(),
        status: AckStatus::SavedAndDrained {
            receipt_sha256: hash('9'),
        },
    }
}
fn all_acknowledged(mut state: LiveOwnership) -> LiveOwnership {
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    for p in &i.participants {
        state = state
            .acknowledge(state.token().unwrap(), &ctx, acknowledgement(p), &budget())
            .unwrap();
    }
    state
}
fn frozen() -> LiveOwnership {
    let state = all_acknowledged(acquire(Purpose::Install));
    let b = binding();
    let i = inventory();
    state
        .mark_frozen(
            &context(&b, &i),
            FenceObservation {
                token: state.token().unwrap(),
                inventory: i.clone(),
                fence_receipt_sha256: hash('a'),
            },
            &budget(),
        )
        .unwrap()
}
fn stop(state: &LiveOwnership, boundary: Boundary) -> StopObservation {
    let i = inventory();
    let stopped = if state.current().unwrap().identity().purpose == Purpose::Preinstall {
        Vec::new()
    } else {
        i.participants
            .iter()
            .map(|p| ParticipantReceipt {
                participant: p.clone(),
                receipt_sha256: hash('b'),
            })
            .collect()
    };
    StopObservation {
        token: state.token().unwrap(),
        inventory: i,
        boundary,
        stopped,
        safety_receipt_sha256: hash('c'),
    }
}
fn close(state: LiveOwnership) -> LiveOwnership {
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    let releasing = state
        .begin_cancel(&ctx, stop(&state, Boundary::NotCrossed))
        .unwrap();
    releasing
        .complete_cancel(
            &ctx,
            ReleaseObservation {
                safety: stop(&releasing, Boundary::NotCrossed),
                release_receipt_sha256: hash('d'),
            },
        )
        .unwrap()
}

#[test]
fn canonical_closed_schema_accepts_colon_ids_and_rejects_ambiguous_bytes() {
    let state = frozen();
    let intent = state.write_intent().unwrap();
    let raw = intent.operation_write.record_bytes().unwrap();
    assert_eq!(
        OperationRecord::parse(&raw, &binding(), "operation:1").unwrap(),
        intent.operation_write
    );
    let original: Value = serde_json::from_slice(&raw).unwrap();
    let mut extra = original.clone();
    extra["extra"] = json!(true);
    let mut nested = original.clone();
    nested["preparation"]["identity"]["operationId"] = json!("bad/path");
    let mut unsafe_epoch = original.clone();
    unsafe_epoch["preparation"]["ownerEpoch"] = json!(SAFE_MAX + 1);
    let mut partial = original.clone();
    partial["preparation"]
        .as_object_mut()
        .unwrap()
        .remove("budget");
    let mut omitted_null = original.clone();
    omitted_null["preparation"]
        .as_object_mut()
        .unwrap()
        .remove("releasingSafetySha256");
    let canonical = |v: &Value| {
        Document::parse(&serde_json::to_vec(v).unwrap(), InputKind::Metadata)
            .unwrap()
            .canonical_bytes()
            .unwrap()
    };
    for bad in [
        canonical(&extra),
        canonical(&nested),
        canonical(&unsafe_epoch),
        canonical(&partial),
        canonical(&omitted_null),
        serde_json::to_vec_pretty(&original).unwrap(),
        br#"{"revision":0,"revision":0}"#.to_vec(),
        vec![b' '; LIVE_RECORD_MAXIMUM + 1],
    ] {
        assert!(OperationRecord::parse(&bad, &binding(), "operation:1").is_err());
    }
    let head_raw = intent.next_head.record_bytes().unwrap();
    assert_eq!(
        ScopeHead::parse(&head_raw, &binding()).unwrap(),
        intent.next_head
    );
    let mut missing: Value = serde_json::from_slice(
        &ScopeHead::initial(&binding())
            .unwrap()
            .record_bytes()
            .unwrap(),
    )
    .unwrap();
    missing.as_object_mut().unwrap().remove("current");
    assert!(ScopeHead::parse(&canonical(&missing), &binding()).is_err());
    let other = BootstrapBinding::new(
        &hash('e'),
        "pinvou",
        "agent",
        "windows-x86_64-nsis-perMachine",
        "production",
        "stable-installation",
        &hash('2'),
    )
    .unwrap();
    assert_eq!(
        OperationRecord::parse(&raw, &other, "operation:1"),
        Err(Error::BindingMismatch)
    );
}

#[test]
fn acquisition_is_cas_exact_duplicate_and_cannot_take_other_occupancy() {
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    let empty = empty("operation:1");
    let first = empty
        .acquire(0, &ctx, identity(Purpose::Install), budget())
        .unwrap();
    assert_eq!(
        first
            .acquire(first.revision(), &ctx, identity(Purpose::Install), budget())
            .unwrap(),
        first
    );
    assert_eq!(
        first.acquire(0, &ctx, identity(Purpose::Install), budget()),
        Err(Error::Stale)
    );
    let mut competitor = identity(Purpose::Install);
    competitor.operation_id = "operation:other".into();
    assert_eq!(
        first.acquire(first.revision(), &ctx, competitor, budget()),
        Err(Error::OriginalIdentityMismatch)
    );
    for occupancy in [
        OtherOccupancy::Execution,
        OtherOccupancy::Reconciliation,
        OtherOccupancy::Unknown,
    ] {
        let blocked = Context {
            other_occupancy: occupancy,
            ..context(&b, &i)
        };
        assert_eq!(
            empty.acquire(0, &blocked, identity(Purpose::Install), budget()),
            Err(Error::Occupied)
        );
        assert_eq!(
            first.begin_cancel(&blocked, stop(&first, Boundary::NotCrossed)),
            Err(Error::Occupied)
        );
    }
    assert_eq!(first.token().unwrap().owner_epoch, 1);
    assert_eq!(first.token().unwrap().freeze_epoch, Some(1));
}

#[test]
fn exact_inventory_and_every_observed_ack_are_required_for_freeze() {
    let state = acquire(Purpose::Install);
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    let fence = |s: &LiveOwnership| FenceObservation {
        token: s.token().unwrap(),
        inventory: i.clone(),
        fence_receipt_sha256: hash('a'),
    };
    assert_eq!(
        state.mark_frozen(&ctx, fence(&state), &budget()),
        Err(Error::Incomplete)
    );
    let first = state
        .acknowledge(
            state.token().unwrap(),
            &ctx,
            acknowledgement(&i.participants[0]),
            &budget(),
        )
        .unwrap();
    assert_eq!(
        first.mark_frozen(&ctx, fence(&first), &budget()),
        Err(Error::Incomplete)
    );
    assert_eq!(
        first.acknowledge(
            state.token().unwrap(),
            &ctx,
            acknowledgement(&i.participants[1]),
            &budget()
        ),
        Err(Error::Stale)
    );
    let mut changed = i.clone();
    changed.revision += 1;
    assert_eq!(
        first.mark_frozen(&context(&b, &changed), fence(&first), &budget()),
        Err(Error::OriginalIdentityMismatch)
    );
    let mut replaced = acknowledgement(&i.participants[1]);
    replaced.participant.instance_sha256 = hash('f');
    assert_eq!(
        first.acknowledge(first.token().unwrap(), &ctx, replaced, &budget()),
        Err(Error::OriginalIdentityMismatch)
    );
    let complete = all_acknowledged(state);
    assert_eq!(
        complete
            .mark_frozen(&ctx, fence(&complete), &budget())
            .unwrap()
            .current()
            .unwrap()
            .phase(),
        Phase::Frozen
    );
}

#[test]
fn refused_or_unknown_writer_keeps_ownership_and_cannot_be_overwritten_by_late_ack() {
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    for status in [
        AckStatus::Refused {
            reason: Refusal::SaveFailed,
        },
        AckStatus::Unknown,
    ] {
        let state = acquire(Purpose::Install);
        let held = state
            .acknowledge(
                state.token().unwrap(),
                &ctx,
                ParticipantAck {
                    owner_epoch: 1,
                    freeze_epoch: 1,
                    inventory_revision: 1,
                    inventory_sha256: hash('3'),
                    participant: i.participants[0].clone(),
                    status,
                },
                &budget(),
            )
            .unwrap();
        assert_eq!(
            held.current().unwrap().phase(),
            Phase::Holding {
                reason: HoldReason::WriterRefused
            }
        );
        assert_eq!(
            held.token().unwrap().owner_epoch,
            state.token().unwrap().owner_epoch
        );
        assert_eq!(
            held.acknowledge(
                held.token().unwrap(),
                &ctx,
                acknowledgement(&i.participants[0]),
                &budget()
            ),
            Err(Error::WrongPhase)
        );
    }
}

#[test]
fn unknown_budget_holds_existing_fence_and_known_time_does_not_auto_resume() {
    let state = frozen();
    let token = state.token().unwrap();
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    let mut unknown = budget();
    unknown.window_revision += 1;
    unknown.assessment = BudgetAssessment::Unknown;
    let held = state.observe_budget(token, &ctx, &unknown).unwrap();
    assert_eq!(
        held.current().unwrap().phase(),
        Phase::Holding {
            reason: HoldReason::TimeUnknown
        }
    );
    assert_eq!(
        held.current().unwrap().fence_receipt_sha256,
        state.current().unwrap().fence_receipt_sha256
    );
    let mut known = unknown.clone();
    known.window_revision += 1;
    known.assessment = budget().assessment;
    let still_held = held
        .observe_budget(held.token().unwrap(), &ctx, &known)
        .unwrap();
    assert_eq!(
        still_held.current().unwrap().phase(),
        Phase::Holding {
            reason: HoldReason::RevalidationRequired
        }
    );
    let resumed = still_held
        .resume(
            &ctx,
            RecoveryObservation {
                token: still_held.token().unwrap(),
                inventory: i.clone(),
                takeover_receipt_sha256: hash('a'),
            },
            &known,
        )
        .unwrap();
    assert_eq!(resumed.current().unwrap().phase(), Phase::Coordinating);
    assert!(resumed.current().unwrap().acknowledgements().is_empty());
    assert!(resumed.current().unwrap().fence_receipt_sha256.is_none());
    assert_eq!(
        resumed.current().unwrap().budget().origin,
        state.current().unwrap().budget().origin
    );
}

#[test]
fn explicit_resume_advances_epochs_so_delayed_saved_ack_cannot_satisfy_fresh_cycle() {
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    let acquired = acquire(Purpose::Install);
    let old_ack = acknowledgement(&i.participants[0]);
    let saved = acquired
        .acknowledge(acquired.token().unwrap(), &ctx, old_ack.clone(), &budget())
        .unwrap();
    let old_token = saved.token().unwrap();
    let mut unknown = budget();
    unknown.window_revision = 2;
    unknown.assessment = BudgetAssessment::Unknown;
    let held = saved.observe_budget(old_token, &ctx, &unknown).unwrap();
    let mut known = unknown;
    known.window_revision = 3;
    known.assessment = budget().assessment;
    let resumed = held
        .resume(
            &ctx,
            RecoveryObservation {
                token: held.token().unwrap(),
                inventory: i.clone(),
                takeover_receipt_sha256: hash('a'),
            },
            &known,
        )
        .unwrap();
    let fresh_token = resumed.token().unwrap();
    assert_eq!(fresh_token.owner_epoch, old_token.owner_epoch + 1);
    assert_eq!(
        fresh_token.freeze_epoch,
        old_token.freeze_epoch.map(|n| n + 1)
    );
    assert_eq!(resumed.current().unwrap().budget(), &known);
    assert_eq!(resumed.current().unwrap().budget().origin, budget().origin);
    assert!(resumed.current().unwrap().acknowledgements().is_empty());
    assert_eq!(
        resumed.acknowledge(fresh_token, &ctx, old_ack.clone(), &known),
        Err(Error::Stale)
    );
    let mut fresh_ack = old_ack;
    fresh_ack.owner_epoch = fresh_token.owner_epoch;
    fresh_ack.freeze_epoch = fresh_token.freeze_epoch.unwrap();
    fresh_ack.status = AckStatus::SavedAndDrained {
        receipt_sha256: hash('e'),
    };
    let observed = resumed
        .acknowledge(fresh_token, &ctx, fresh_ack.clone(), &known)
        .unwrap();
    assert_eq!(observed.current().unwrap().acknowledgements(), &[fresh_ack]);
}

#[test]
fn exhausted_budget_is_not_refreshed_by_retry_or_recovery() {
    let state = acquire(Purpose::Install);
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    let mut exhausted = budget();
    exhausted.window_revision = 2;
    exhausted.spent_max_ms = 99_900;
    exhausted.assessment = BudgetAssessment::Reached;
    let held = state
        .observe_budget(state.token().unwrap(), &ctx, &exhausted)
        .unwrap();
    assert_eq!(
        held.current().unwrap().phase(),
        Phase::Holding {
            reason: HoldReason::BudgetExhausted
        }
    );
    let recovered = held
        .recover(
            &ctx,
            RecoveryObservation {
                token: held.token().unwrap(),
                inventory: i.clone(),
                takeover_receipt_sha256: hash('a'),
            },
            &exhausted,
        )
        .unwrap();
    assert_eq!(recovered.current().unwrap().budget(), &exhausted);
    let mut renewed = exhausted.clone();
    renewed.window_revision += 1;
    renewed.assessment = budget().assessment;
    assert_eq!(
        recovered.observe_budget(recovered.token().unwrap(), &ctx, &renewed),
        Err(Error::BudgetMismatch)
    );
    let resumed = recovered
        .resume(
            &ctx,
            RecoveryObservation {
                token: recovered.token().unwrap(),
                inventory: i.clone(),
                takeover_receipt_sha256: hash('a'),
            },
            &exhausted,
        )
        .unwrap();
    assert!(matches!(
        resumed.current().unwrap().phase(),
        Phase::Holding { .. }
    ));
}

#[test]
fn recovery_advances_both_epochs_and_late_owner_cannot_ack_or_cancel() {
    let state = frozen();
    let old = state.token().unwrap();
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    let recovered = state
        .recover(
            &ctx,
            RecoveryObservation {
                token: old,
                inventory: i.clone(),
                takeover_receipt_sha256: hash('e'),
            },
            &budget(),
        )
        .unwrap();
    let token = recovered.token().unwrap();
    assert_eq!(token.owner_epoch, old.owner_epoch + 1);
    assert_eq!(token.freeze_epoch, old.freeze_epoch.map(|n| n + 1));
    assert_eq!(
        recovered.current().unwrap().identity(),
        state.current().unwrap().identity()
    );
    assert!(recovered.current().unwrap().acknowledgements().is_empty());
    assert_eq!(
        recovered.acknowledge(old, &ctx, acknowledgement(&i.participants[0]), &budget()),
        Err(Error::Stale)
    );
    let resumed = recovered
        .resume(
            &ctx,
            RecoveryObservation {
                token,
                inventory: i.clone(),
                takeover_receipt_sha256: hash('e'),
            },
            &budget(),
        )
        .unwrap();
    // CAS is current, but the actual ACK still names the old owner/freeze.
    assert_eq!(
        resumed.acknowledge(
            resumed.token().unwrap(),
            &ctx,
            acknowledgement(&i.participants[0]),
            &budget()
        ),
        Err(Error::Stale)
    );
    assert_eq!(
        recovered.begin_cancel(&ctx, stop(&state, Boundary::NotCrossed)),
        Err(Error::Stale)
    );
    assert_eq!(
        recovered.current().unwrap().budget(),
        state.current().unwrap().budget()
    );
}

#[test]
fn cancel_is_two_phase_and_unknown_crossed_incomplete_or_wrong_owner_refuses() {
    let state = frozen();
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    for boundary in [Boundary::Unknown, Boundary::Crossed] {
        assert_eq!(
            state.begin_cancel(&ctx, stop(&state, boundary)),
            Err(Error::UnsafeBoundary)
        );
    }
    let mut incomplete = stop(&state, Boundary::NotCrossed);
    incomplete.stopped.pop();
    assert_eq!(state.begin_cancel(&ctx, incomplete), Err(Error::Incomplete));
    assert_eq!(
        state.complete_cancel(
            &ctx,
            ReleaseObservation {
                safety: stop(&state, Boundary::NotCrossed),
                release_receipt_sha256: hash('d')
            }
        ),
        Err(Error::WrongPhase)
    );
    let releasing = state
        .begin_cancel(&ctx, stop(&state, Boundary::NotCrossed))
        .unwrap();
    assert_eq!(releasing.current().unwrap().phase(), Phase::Releasing);
    assert_eq!(
        releasing.token().unwrap().owner_epoch,
        state.token().unwrap().owner_epoch
    );
    assert_eq!(
        releasing.token().unwrap().freeze_epoch,
        state.token().unwrap().freeze_epoch
    );
    assert_eq!(
        releasing.current().unwrap().fence_receipt_sha256,
        state.current().unwrap().fence_receipt_sha256
    );
    assert_eq!(
        releasing.complete_cancel(
            &ctx,
            ReleaseObservation {
                safety: stop(&releasing, Boundary::Unknown),
                release_receipt_sha256: hash('d')
            }
        ),
        Err(Error::UnsafeBoundary)
    );
    let completed = close(state);
    assert!(completed.current().is_none());
    assert!(completed.closed().is_some());
    let intent = completed.write_intent().unwrap();
    assert_eq!(
        OperationRecord::parse(
            &intent.operation_write.record_bytes().unwrap(),
            &b,
            "operation:1"
        )
        .unwrap(),
        intent.operation_write
    );
}

#[test]
fn cancellation_completion_binds_published_safety_and_accepts_separate_fresh_release_receipt() {
    let state = frozen();
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    let releasing = state
        .begin_cancel(&ctx, stop(&state, Boundary::NotCrossed))
        .unwrap();
    let before = releasing
        .write_intent()
        .unwrap()
        .operation_write
        .record_bytes()
        .unwrap();
    let mut substituted = stop(&releasing, Boundary::NotCrossed);
    substituted.safety_receipt_sha256 = hash('e');
    assert_eq!(
        releasing.complete_cancel(
            &ctx,
            ReleaseObservation {
                safety: substituted,
                release_receipt_sha256: hash('f')
            }
        ),
        Err(Error::OriginalIdentityMismatch)
    );
    assert_eq!(
        releasing
            .write_intent()
            .unwrap()
            .operation_write
            .record_bytes()
            .unwrap(),
        before
    );
    let closed = releasing
        .complete_cancel(
            &ctx,
            ReleaseObservation {
                safety: stop(&releasing, Boundary::NotCrossed),
                release_receipt_sha256: hash('f'),
            },
        )
        .unwrap();
    assert_eq!(
        closed.closed().unwrap().status(),
        &OperationStatus::Closed {
            release_receipt_sha256: hash('f')
        }
    );
    assert_eq!(
        closed
            .closed()
            .unwrap()
            .preparation
            .releasing_safety_sha256
            .as_deref(),
        Some(hash('c').as_str())
    );
}

#[test]
fn closed_operation_cannot_receive_a_fresh_session_or_budget_and_next_epochs_increase() {
    let state = acquire(Purpose::Install);
    let completed = close(state);
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    let mut new_session = identity(Purpose::Install);
    new_session.original_session_id = "session:new".into();
    let mut refreshed = budget();
    refreshed.origin.first_lower_ms += 100;
    refreshed.origin.first_upper_ms += 100;
    refreshed.origin.original_deadline_ms += 100;
    refreshed.assessment = BudgetAssessment::Before {
        remaining_ms: 99_000,
    };
    assert_eq!(
        completed.acquire(
            completed.revision(),
            &ctx,
            new_session.clone(),
            refreshed.clone()
        ),
        Err(Error::OriginalIdentityMismatch)
    );
    new_session.operation_id = "operation:2".into();
    let head = completed.write_intent().unwrap().next_head;
    let selected = LiveOwnership::load(
        &head,
        "operation:2",
        ExistingOperation::NoPrior,
        BudgetPresence::Absent,
    )
    .unwrap();
    let next = selected
        .acquire(selected.revision(), &ctx, new_session, refreshed)
        .unwrap();
    assert_eq!(next.token().unwrap().owner_epoch, 2);
    assert_eq!(next.token().unwrap().freeze_epoch, Some(2));
}

#[test]
fn preinstall_never_allocates_freeze_or_coordinates_active_business() {
    let state = acquire(Purpose::Preinstall);
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    assert_eq!(state.token().unwrap().freeze_epoch, None);
    assert_eq!(state.freeze_epoch, 0);
    assert_eq!(
        state.acknowledge(
            state.token().unwrap(),
            &ctx,
            acknowledgement(&i.participants[0]),
            &budget()
        ),
        Err(Error::WrongPhase)
    );
    assert_eq!(
        state.mark_frozen(
            &ctx,
            FenceObservation {
                token: state.token().unwrap(),
                inventory: i.clone(),
                fence_receipt_sha256: hash('a')
            },
            &budget()
        ),
        Err(Error::WrongPhase)
    );
    assert!(close(state).current().is_none());
}

#[test]
fn retry_cannot_replace_origin_widen_cutoff_decrease_spend_or_change_original_session() {
    let state = acquire(Purpose::Install);
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    let mut variants = Vec::new();
    let mut changed = budget();
    changed.origin.window_key_sha256 = hash('9');
    variants.push(changed);
    let mut changed = budget();
    changed.origin.original_duration_ms += 1;
    changed.origin.original_deadline_ms += 1;
    variants.push(changed);
    let mut changed = budget();
    changed.origin.original_session_cutoff_ms += 1;
    variants.push(changed);
    let mut changed = budget();
    changed.spent_max_ms = 0;
    variants.push(changed);
    let mut changed = budget();
    changed.session_cutoff_ms += 1;
    variants.push(changed);
    for changed in variants {
        assert!(
            state
                .acquire(state.revision(), &ctx, identity(Purpose::Install), changed)
                .is_err()
        );
    }
    let mut session = identity(Purpose::Install);
    session.original_session_id = "session:retry".into();
    assert_eq!(
        state.acquire(state.revision(), &ctx, session, budget()),
        Err(Error::Occupied)
    );
    let mut tightened = budget();
    tightened.window_revision += 1;
    tightened.spent_max_ms = 2;
    tightened.session_cutoff_ms -= 1;
    tightened.assessment = BudgetAssessment::Before {
        remaining_ms: 99_897,
    };
    let progressed = state
        .observe_budget(state.token().unwrap(), &ctx, &tightened)
        .unwrap();
    assert_eq!(
        progressed.current().unwrap().budget().origin,
        state.current().unwrap().budget().origin
    );
}

#[test]
fn closed_replay_after_many_operations_has_no_lifetime_array_or_attempt_cap() {
    let b = binding();
    let i = inventory();
    let ctx = context(&b, &i);
    let first = close(acquire(Purpose::Preinstall)).write_intent().unwrap();
    let original_closed = first.operation_write;
    let mut head = first.next_head;
    for n in 2..=300 {
        let op = format!("operation:{n}");
        let loaded = LiveOwnership::load(
            &head,
            &op,
            ExistingOperation::NoPrior,
            BudgetPresence::Absent,
        )
        .unwrap();
        let mut id = identity(Purpose::Preinstall);
        id.operation_id = op;
        let active = loaded
            .acquire(loaded.revision(), &ctx, id, budget())
            .unwrap();
        head = close(active).write_intent().unwrap().next_head;
    }
    let old = LiveOwnership::load(
        &head,
        "operation:1",
        ExistingOperation::Closed(&original_closed),
        BudgetPresence::Present(&budget()),
    )
    .unwrap();
    let mut replay = identity(Purpose::Preinstall);
    replay.original_session_id = "session:new".into();
    assert_eq!(
        old.acquire(old.revision(), &ctx, replay, budget()),
        Err(Error::OriginalIdentityMismatch)
    );
    assert_eq!(head.owner_epoch, 300);
    assert_eq!(head.freeze_epoch, 0);
    assert!(head.record_bytes().unwrap().len() < 1024);
}

#[test]
fn operation_and_original_budget_partial_or_mismatched_pairs_never_heal() {
    let initial = ScopeHead::initial(&binding()).unwrap();
    let active = acquire(Purpose::Install).write_intent().unwrap();
    let closed = close(acquire(Purpose::Install)).write_intent().unwrap();
    assert_eq!(
        LiveOwnership::load(
            &initial,
            "operation:1",
            ExistingOperation::NoPrior,
            BudgetPresence::Present(&budget())
        ),
        Err(Error::CorruptReadSet)
    );
    assert_eq!(
        LiveOwnership::load(
            &active.next_head,
            "operation:1",
            ExistingOperation::Active(&active.operation_write),
            BudgetPresence::Absent
        ),
        Err(Error::CorruptReadSet)
    );
    assert_eq!(
        LiveOwnership::load(
            &closed.next_head,
            "operation:1",
            ExistingOperation::Closed(&closed.operation_write),
            BudgetPresence::Absent
        ),
        Err(Error::CorruptReadSet)
    );
    assert_eq!(
        LiveOwnership::load(
            &active.next_head,
            "operation:1",
            ExistingOperation::NoPrior,
            BudgetPresence::Absent
        ),
        Err(Error::CorruptReadSet)
    );
    assert_eq!(
        LiveOwnership::load(
            &initial,
            "operation:1",
            ExistingOperation::Active(&active.operation_write),
            BudgetPresence::Present(&budget())
        ),
        Err(Error::CorruptReadSet)
    );
    assert_eq!(
        LiveOwnership::load(
            &closed.next_head,
            "operation:1",
            ExistingOperation::Active(&closed.operation_write),
            BudgetPresence::Present(&budget())
        ),
        Err(Error::CorruptReadSet)
    );
    let mut wrong = budget();
    wrong.origin.window_key_sha256 = hash('f');
    assert_eq!(
        LiveOwnership::load(
            &active.next_head,
            "operation:1",
            ExistingOperation::Active(&active.operation_write),
            BudgetPresence::Present(&wrong)
        ),
        Err(Error::CorruptReadSet)
    );
    let roundtrip = LiveOwnership::load(
        &active.next_head,
        "operation:1",
        ExistingOperation::Active(&active.operation_write),
        BudgetPresence::Present(&budget()),
    )
    .unwrap();
    assert_eq!(
        roundtrip.current().unwrap(),
        &active.operation_write.preparation
    );
}

#[test]
fn per_record_participant_and_safe_epoch_caps_refuse_without_lifetime_limit() {
    let b = binding();
    let oversized = Inventory {
        revision: 1,
        sha256: hash('a'),
        participants: (0..=MAX_PARTICIPANTS)
            .map(|n| Participant {
                id: format!("writer:{n:04}"),
                instance_sha256: hash('b'),
            })
            .collect(),
    };
    assert_eq!(
        empty("operation:1").acquire(
            0,
            &context(&b, &oversized),
            identity(Purpose::Install),
            budget()
        ),
        Err(Error::Capacity)
    );
    let mut head = ScopeHead::initial(&b).unwrap();
    head.revision = SAFE_MAX;
    head.owner_epoch = SAFE_MAX;
    head.freeze_epoch = SAFE_MAX;
    let loaded = LiveOwnership::load(
        &head,
        "operation:1",
        ExistingOperation::NoPrior,
        BudgetPresence::Absent,
    )
    .unwrap();
    assert_eq!(
        loaded.acquire(
            SAFE_MAX,
            &context(&b, &inventory()),
            identity(Purpose::Install),
            budget()
        ),
        Err(Error::Capacity)
    );
}
