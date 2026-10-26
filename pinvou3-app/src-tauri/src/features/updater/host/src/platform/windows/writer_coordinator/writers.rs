//! Per-original writer transitions. Stack-only native admission tokens protect
//! the journal and keeper retain exchange through final publication/readback.
use super::super::{
    registry::writers::{ProtectedAdmissionIntent, WriterJobIntentPhase},
    writer_keeper::{acknowledge_query_copy, release_closed_query},
};
use super::*;
use crate::error::Result as NativeResult;

pub(super) fn enroll(
    host: &Arc<StartedServiceContext>,
    keeper: &mut KeeperState,
    slot: &mut WriterSlot,
    peer: &Peer,
    pipe: &Pipe,
    stop: &Event,
) -> Result<Participant> {
    check_stop(stop).map_err(CoordinatorFailure::unknown)?;
    if slot.close_attempted || !same_original(&slot.original, peer) {
        return Err(CoordinatorFailure::unknown(Error::PeerRefused));
    }
    let mut intent = match &mut slot.custody {
        WriterCustody::Reserved(job) => {
            // The owner checks actual protected absence/exact original in the
            // same guard/TxR. A generic error never authorizes a fresh nonce.
            Store::resume_or_begin_writer_original(host, job, peer, pipe)
        }
        WriterCustody::Pending(job) => Store::resume_writer_pending(host, job),
        WriterCustody::Assigned(job) => Store::resume_writer_custody(host, job),
        WriterCustody::Transition => return Err(CoordinatorFailure::unknown(Error::StateCorrupt)),
    }
    .map_err(CoordinatorFailure::unknown)?;
    slot.reference = Some(intent.reference().map_err(CoordinatorFailure::unknown)?);
    // This SAME guard remains owned across create, assign, native keeper retain,
    // protected Admitted commit/readback, and final peer/job/keeper validation.
    for _ in 0..4 {
        check_stop(stop).map_err(CoordinatorFailure::unknown)?;
        peer.recheck(pipe).map_err(CoordinatorFailure::unknown)?;
        match &mut slot.custody {
            WriterCustody::Reserved(_) => {
                let owned = std::mem::replace(&mut slot.custody, WriterCustody::Transition);
                let job = match owned {
                    WriterCustody::Reserved(job) => job,
                    other => {
                        slot.custody = other;
                        return Err(CoordinatorFailure::unknown(Error::StateCorrupt));
                    }
                };
                match job.create(host, peer, pipe, &intent) {
                    Ok(job) => slot.custody = WriterCustody::Pending(job),
                    Err(failure) => {
                        slot.custody = WriterCustody::Reserved(failure.reservation);
                        return Err(CoordinatorFailure::unknown(failure.error));
                    }
                }
            }
            WriterCustody::Pending(_) => {
                let owned = std::mem::replace(&mut slot.custody, WriterCustody::Transition);
                let job = match owned {
                    WriterCustody::Pending(job) => job,
                    other => {
                        slot.custody = other;
                        return Err(CoordinatorFailure::unknown(Error::StateCorrupt));
                    }
                };
                match job.assign(host, peer, pipe, &intent) {
                    Ok(job) => slot.custody = WriterCustody::Assigned(job),
                    Err(failure) => {
                        slot.custody = WriterCustody::Pending(failure.pending);
                        return Err(CoordinatorFailure::unknown(failure.error));
                    }
                }
            }
            WriterCustody::Assigned(job) => {
                return admit_assigned(
                    host,
                    keeper,
                    job,
                    &mut intent,
                    &mut slot.keeper_copy,
                    &mut slot.keeper_ack,
                    peer,
                    pipe,
                    stop,
                )
                .map_err(CoordinatorFailure::unknown);
            }
            WriterCustody::Transition => {
                return Err(CoordinatorFailure::unknown(Error::StateCorrupt));
            }
        }
    }
    Err(CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))
}

fn admit_assigned<'host>(
    host: &'host Arc<StartedServiceContext>,
    keeper: &mut KeeperState,
    job: &WriterJob,
    intent: &mut ProtectedAdmissionIntent<'host>,
    copy: &mut Option<ParkedQueryCopy>,
    ack: &mut Option<KeeperJobAck>,
    peer: &Peer,
    pipe: &Pipe,
    stop: &Event,
) -> NativeResult<Participant> {
    check_stop(stop)?;
    if !matches!(
        intent.phase(),
        WriterJobIntentPhase::Reserved | WriterJobIntentPhase::Admitted
    ) {
        return Err(Error::ScopeConflict);
    }
    retain_query(host, keeper, job, intent, copy, ack, stop)?;
    check_stop(stop)?;
    let ack = ack.as_ref().ok_or(Error::HandoffOutcomeUnknown)?;
    let mut live = keeper.peer().map_err(|failure| failure.error)?;
    // Immutable original identity DATA, verified under the same guard. The
    // publication below rechecks that exact identity before and after commit.
    let participant = intent.participant()?;
    let published = match intent.phase() {
        WriterJobIntentPhase::Reserved => intent.transition_admitted(job, ack, &mut live)?,
        WriterJobIntentPhase::Admitted => intent.published_admission(job, ack, &mut live)?,
        _ => return Err(Error::ScopeConflict),
    };
    // Final native checks follow the DATA getter and protected publication.
    peer.recheck(pipe)?;
    if !same_original(job.original(), peer) {
        return Err(Error::PeerRefused);
    }
    published.recheck(job, ack, &mut live)?;
    check_stop(stop)?;
    Ok(participant)
}

fn retain_query<'host>(
    host: &'host Arc<StartedServiceContext>,
    keeper: &mut KeeperState,
    job: &WriterJob,
    intent: &ProtectedAdmissionIntent<'host>,
    copy: &mut Option<ParkedQueryCopy>,
    ack: &mut Option<KeeperJobAck>,
    stop: &Event,
) -> NativeResult<()> {
    check_stop(stop)?;
    let source = job.keeper_query_borrow(host, intent)?;
    let live = keeper.peer().map_err(|failure| failure.error)?;
    let mut pending = match copy.take() {
        Some(parked) => match parked.resume(live, &source) {
            Ok(pending) => pending,
            Err(failure) => {
                // Reinstate the original local backup/destination/remote value
                // before returning. This path never makes another remote copy.
                *copy = Some(failure.parked);
                return Err(failure.error);
            }
        },
        None => {
            if ack.is_some() {
                return Err(Error::StateCorrupt);
            }
            match live.duplicate_job_query(&source) {
                Ok(pending) => pending,
                Err(failure) => {
                    *copy = failure.pending.map(|pending| pending.park());
                    return Err(failure.error);
                }
            }
        }
    };
    // All fallible cancellation while Pending borrows the keeper is contained
    // in this closure; the original copy is ALWAYS parked below before return.
    let result = (|| -> NativeResult<()> {
        check_stop(stop)?;
        if ack.is_none() {
            match acknowledge_query_copy(host, intent, &mut pending, stop) {
                Ok(receipt) => {
                    // Keep the actual Retain ACK before Admitted publication;
                    // actual live gates still run at publication and reply.
                    *ack = Some(receipt);
                    Ok(())
                }
                Err(error) => Err(error),
            }
        } else {
            pending.recheck()
        }?;
        check_stop(stop)
    })();
    // Infallible park removes the keeper borrow. Both success and failure retain
    // the source backup plus exact original remote address, including remote 0.
    *copy = Some(pending.park());
    result
}

/// Native closure is terminal for this original writer, but does not discharge
/// any preparation participant or Saved receipt. The service keeps local query
/// backup and original/job custody through a real guarded Closed read and the
/// keeper's actual Release ACK, including an unregistered zero-copy release.
pub(super) fn close_and_release(
    host: &Arc<StartedServiceContext>,
    keeper: &mut KeeperState,
    slot: &mut WriterSlot,
    stop: &Event,
) -> Result<()> {
    check_stop(stop).map_err(CoordinatorFailure::unknown)?;
    if !slot
        .original
        .exited()
        .map_err(CoordinatorFailure::unknown)?
    {
        return Err(CoordinatorFailure::unknown(Error::Busy));
    }
    if let WriterCustody::Reserved(job) = &slot.custody {
        if slot.keeper_copy.is_some() || slot.keeper_ack.is_some() || slot.release_ack.is_some() {
            return Err(CoordinatorFailure::unknown(Error::StateCorrupt));
        }
        let exited = job
            .prove_unassigned_exit(host)
            .map_err(CoordinatorFailure::unknown)?;
        check_stop(stop).map_err(CoordinatorFailure::unknown)?;
        return Store::retire_unassigned_writer(host, &exited).map_err(CoordinatorFailure::unknown);
    }
    if slot.close_attempted {
        let reference = slot
            .reference
            .as_ref()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::StateCorrupt))?;
        match Store::confirm_writer_closed(host, reference) {
            Ok(closed) => {
                return release_closed(host, keeper, &closed, &mut slot.release_ack, stop)
                    .map_err(CoordinatorFailure::unknown);
            }
            // A still-active exact row may be retried ONLY through its original
            // native custody factory below. No missing-row/foreign error heals.
            Err(Error::ScopeConflict) => {}
            Err(error) => return Err(CoordinatorFailure::unknown(error)),
        }
    }
    match &slot.custody {
        WriterCustody::Assigned(job) => {
            let intent =
                Store::resume_writer_custody(host, job).map_err(CoordinatorFailure::unknown)?;
            let zero = job.prove_zero(host).map_err(CoordinatorFailure::unknown)?;
            check_stop(stop).map_err(CoordinatorFailure::unknown)?;
            slot.reference = Some(intent.reference().map_err(CoordinatorFailure::unknown)?);
            slot.close_attempted = true;
            intent
                .close_live(&zero)
                .map_err(CoordinatorFailure::unknown)?;
        }
        WriterCustody::Pending(job) => {
            let intent =
                Store::resume_writer_pending(host, job).map_err(CoordinatorFailure::unknown)?;
            let zero = job
                .prove_zero(host, &intent)
                .map_err(CoordinatorFailure::unknown)?;
            check_stop(stop).map_err(CoordinatorFailure::unknown)?;
            slot.reference = Some(intent.reference().map_err(CoordinatorFailure::unknown)?);
            slot.close_attempted = true;
            intent
                .close_pending(&zero)
                .map_err(CoordinatorFailure::unknown)?;
        }
        // The separate typed no-assignment/exit path above is mandatory.
        WriterCustody::Reserved(_) => {
            return Err(CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown));
        }
        WriterCustody::Transition => return Err(CoordinatorFailure::unknown(Error::StateCorrupt)),
    }
    // The prior Intent/BootstrapGuard has dropped. This factory acquires its
    // own guard and retains it through the entire release exchange.
    let reference = slot
        .reference
        .as_ref()
        .ok_or_else(|| CoordinatorFailure::unknown(Error::StateCorrupt))?;
    let closed =
        Store::confirm_writer_closed(host, reference).map_err(CoordinatorFailure::unknown)?;
    release_closed(host, keeper, &closed, &mut slot.release_ack, stop)
        .map_err(CoordinatorFailure::unknown)
}

pub(super) fn release_closed(
    host: &Arc<StartedServiceContext>,
    keeper: &mut KeeperState,
    closed: &super::super::registry::writers::ClosedWriterRead,
    ack: &mut Option<KeeperReleasedAck>,
    stop: &Event,
) -> NativeResult<()> {
    check_stop(stop)?;
    let mut live = keeper.peer().map_err(|failure| failure.error)?;
    let result = (|| -> NativeResult<()> {
        live.recheck()?;
        if ack.is_none() {
            // Preserve the native ACK BEFORE pending-release publication. An
            // Unknown retry must reuse it, not require another actor release.
            *ack = Some(release_closed_query(host, closed, &mut live, stop)?);
        }
        check_stop(stop)?;
        Store::finish_writer_release(
            host,
            closed,
            ack.as_ref().ok_or(Error::HandoffOutcomeUnknown)?,
            &mut live,
        )
    })();
    if let Err(error) = result {
        // Stop cancels the exchange; do not enter a new fallback operation or
        // another slot. Cached ACK/copy/job/native original all stay in state.
        if error == Error::RestartCanceled {
            return Err(error);
        }
        check_stop(stop)?;
        // A real held-original exit is a separate typed terminal proof: all
        // remote handles in that actual process are gone. A failed wire call,
        // absent name, census, or saved boolean cannot construct this proof.
        // The protected owner rechecks exact Active keeper identity and Closed
        // pending membership under THIS still-held ClosedWriterRead guard.
        let exited = match live.observe_exit() {
            Ok(exited) => exited,
            Err(_) => return Err(error),
        };
        Store::finish_writer_release_after_keeper_exit(host, closed, &exited)?;
    }
    closed.recheck(host)
}
