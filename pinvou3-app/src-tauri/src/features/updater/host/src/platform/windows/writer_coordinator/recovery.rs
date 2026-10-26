//! Older protected writer selections and Closed pending releases survive service
//! restart. No current-epoch PID reconstruction, name recreation, empty-map
//! inference, unbounded history scan, save receipt or freeze grant occurs here.
use super::*;

pub(super) fn discover(
    host: &StartedServiceContext,
    state: &mut State,
    stop: &Event,
) -> Result<()> {
    check_stop(stop).map_err(CoordinatorFailure::unknown)?;
    // These flags describe completed native reads in this coordinator only.
    // They cannot create a protected reference, zero proof, or grant capability.
    // Root routes every writer mutation through this mutex; preparation remains
    // independently serialized by the native BootstrapGuard/TxR owner.
    if !state.recovery_active_loaded {
        let census = Store::writer_recovery_census(host).map_err(CoordinatorFailure::unknown)?;
        let references = census.into_references();
        if references.len() > MAX_HELD_WRITERS {
            return Err(CoordinatorFailure::unknown(Error::StateCorrupt));
        }
        for reference in references {
            state.recovered.push(RecoveredSlot {
                reference,
                job: None,
                close_attempted: false,
                release_ack: None,
            });
        }
        state.recovery_active_loaded = true;
    }
    if !state.recovery_pending_loaded {
        check_stop(stop).map_err(CoordinatorFailure::unknown)?;
        let references =
            Store::writer_pending_release_census(host).map_err(CoordinatorFailure::unknown)?;
        if state.recovered.len() + references.len() > MAX_HELD_WRITERS {
            // Retain earlier native selections. No admission proceeds from an
            // inconsistent/over-capacity census; storage remains authoritative.
            return Err(CoordinatorFailure::unknown(Error::StateCorrupt));
        }
        for reference in references {
            state.recovered.push(RecoveredSlot {
                reference,
                job: None,
                close_attempted: true,
                release_ack: None,
            });
        }
        state.recovery_pending_loaded = true;
    }
    Ok(())
}

pub(super) fn close_and_release(
    host: &Arc<StartedServiceContext>,
    keeper: &mut KeeperState,
    slot: &mut RecoveredSlot,
    stop: &Event,
) -> Result<()> {
    check_stop(stop).map_err(CoordinatorFailure::unknown)?;
    if slot.close_attempted {
        match Store::confirm_writer_closed(host, &slot.reference) {
            Ok(closed) => {
                return writers::release_closed(host, keeper, &closed, &mut slot.release_ack, stop)
                    .map_err(CoordinatorFailure::unknown);
            }
            Err(Error::ScopeConflict) => {}
            Err(error) => return Err(CoordinatorFailure::unknown(error)),
        }
    }
    {
        let intent =
            Store::recover_writer(host, &slot.reference).map_err(CoordinatorFailure::unknown)?;
        // The actual recovery factory independently requires origin < current
        // service epoch; no caller epoch or "recovered" flag substitutes for it.
        if slot.job.is_none() {
            match RecoveredWriterJob::open_existing(host, &intent) {
                Ok(job) => slot.job = Some(job),
                Err(failure) => {
                    // Post-open failure retains the SAME actual job handle.
                    // Subsequent maintenance rechecks it, never opens a new one.
                    slot.job = failure.custody;
                    return Err(CoordinatorFailure::unknown(failure.error));
                }
            }
        }
        let job = slot
            .job
            .as_ref()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        let zero = job
            .prove_zero(host, &intent)
            .map_err(CoordinatorFailure::unknown)?;
        check_stop(stop).map_err(CoordinatorFailure::unknown)?;
        slot.close_attempted = true;
        intent
            .close_recovered(&zero)
            .map_err(CoordinatorFailure::unknown)?;
    }
    // Intent and its native guard have dropped before acquiring Closed's guard.
    // The original reference and opened native handle remain in owned state.
    let closed =
        Store::confirm_writer_closed(host, &slot.reference).map_err(CoordinatorFailure::unknown)?;
    writers::release_closed(host, keeper, &closed, &mut slot.release_ack, stop)
        .map_err(CoordinatorFailure::unknown)
}
