//! Keeper custody transitions. Every consuming native call returns either its
//! successor or its complete failure custody before an error reaches the caller.
use super::super::{
    registry::keeper::KeeperIntentPhase,
    writer_keeper::{acknowledge_initial_ready, authenticate_connection},
};
use super::*;
use crate::error::Result as NativeResult;

pub(super) fn ensure_active(
    host: &Arc<StartedServiceContext>,
    state: &mut KeeperState,
    stop: &Event,
) -> Result<()> {
    // A fresh path has four stages. This is a bound on synchronous work, not a
    // lifetime retry counter or permission to replace a failed original keeper.
    for _ in 0..8 {
        check_stop(stop).map_err(CoordinatorFailure::unknown)?;
        let owned = std::mem::replace(state, KeeperState::Transition);
        let (next, result) = advance(host, owned, stop);
        *state = next;
        let active = result?;
        check_stop(stop).map_err(CoordinatorFailure::unknown)?;
        if active {
            return Ok(());
        }
    }
    Err(CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))
}

fn advance(
    host: &Arc<StartedServiceContext>,
    state: KeeperState,
    stop: &Event,
) -> (KeeperState, Result<bool>) {
    if let Err(error) = check_stop(stop) {
        return (state, Err(CoordinatorFailure::unknown(error)));
    }
    match state {
        KeeperState::Unselected => {
            match Store::keeper_reference(host).map_err(CoordinatorFailure::unknown) {
                Err(error) => (KeeperState::Unselected, Err(error)),
                Ok(Some(reference)) => {
                    // Reopen itself requires the exact protected Active phase.
                    // Non-Active/missing/foreign state is not a new bootstrap.
                    reopen(Arc::clone(host), reference)
                }
                Ok(None) => match PreparedKeeperSpawn::prepare(Arc::clone(host)) {
                    Ok(prepared) => (KeeperState::Prepared(prepared), Ok(false)),
                    Err(error) => (
                        KeeperState::Unselected,
                        Err(CoordinatorFailure::unknown(error)),
                    ),
                },
            }
        }
        KeeperState::Prepared(prepared) => {
            // The owner resumes the original nonce or confirms protected Idle
            // before first publication. Generic errors never authorize begin.
            let intent = Store::resume_or_begin_keeper_reservation(Arc::clone(host), &prepared);
            let intent = match intent {
                Ok(intent) => intent,
                Err(error) => {
                    return (
                        KeeperState::Prepared(prepared),
                        Err(CoordinatorFailure::unknown(error)),
                    );
                }
            };
            if let Err(error) = check_stop(stop) {
                return (
                    KeeperState::Prepared(prepared),
                    Err(CoordinatorFailure::unknown(error)),
                );
            }
            // No fallible work between moving Prepared and installing the
            // native success/failure carrier in the returned owned state.
            match prepared.create_suspended(&intent) {
                Ok(child) => (KeeperState::Child(child), Ok(false)),
                Err(failure) => {
                    let error = CoordinatorFailure::unknown(failure.error);
                    (KeeperState::SpawnFailed(failure), Err(error))
                }
            }
        }
        KeeperState::Child(mut child) => {
            let mut intent = match Store::resume_keeper_child(Arc::clone(host), &mut child) {
                Ok(intent) => intent,
                Err(error) => {
                    return (
                        KeeperState::Child(child),
                        Err(CoordinatorFailure::unknown(error)),
                    );
                }
            };
            // This transition is idempotent only for the same actual child and
            // exact original protected Suspended row, checked by its owner.
            if let Err(error) = intent.record_child(&mut child) {
                return (
                    KeeperState::Child(child),
                    Err(CoordinatorFailure::unknown(error)),
                );
            }
            if let Err(error) = check_stop(stop) {
                return (
                    KeeperState::Child(child),
                    Err(CoordinatorFailure::unknown(error)),
                );
            }
            match child.resume(&intent) {
                Ok(keeper) => (
                    KeeperState::Running {
                        keeper,
                        ready: None,
                    },
                    Ok(false),
                ),
                Err(failure) => (
                    KeeperState::Child(failure.child),
                    Err(CoordinatorFailure::unknown(failure.error)),
                ),
            }
        }
        KeeperState::Running {
            mut keeper,
            mut ready,
        } => {
            let result = activate_running(host, &mut keeper, &mut ready, stop)
                .map_err(CoordinatorFailure::unknown);
            (
                KeeperState::Running { keeper, ready },
                result.map(|()| true),
            )
        }
        KeeperState::Recovery(KeeperRecoveryCustody::Selection { host, reference }) => {
            reopen(host, reference)
        }
        KeeperState::Recovery(KeeperRecoveryCustody::Opened(keeper)) => {
            (KeeperState::Recovered(keeper), Ok(false))
        }
        KeeperState::Recovered(mut keeper) => {
            let result = (|| -> NativeResult<()> {
                check_stop(stop)?;
                let intent = Store::recover_keeper_custody(Arc::clone(host), &mut keeper)?;
                intent.require_active(host, keeper.reservation_nonce(), keeper.selector()?)?;
                drop(intent);
                let mut peer = KeeperPeer::Recovered(&mut keeper);
                // An authenticated nonempty map is expected after service
                // restart. Initial empty Ready is never requested on this path.
                authenticate_connection(host, &mut peer, stop)?;
                peer.recheck()?;
                check_stop(stop)
            })();
            (
                KeeperState::Recovered(keeper),
                result.map(|()| true).map_err(CoordinatorFailure::unknown),
            )
        }
        KeeperState::SpawnFailed(failure) => {
            // Some failed spawn carriers own an ambiguous staging job/child.
            // No public API safely reconciles all of them. Preserve the ENTIRE
            // carrier; no new nonce, guessed exit, retry spawn, or handle drop.
            let error = CoordinatorFailure::unknown(failure.error);
            (KeeperState::SpawnFailed(failure), Err(error))
        }
        KeeperState::Transition => (
            KeeperState::Transition,
            Err(CoordinatorFailure::unknown(Error::StateCorrupt)),
        ),
    }
}

fn reopen(
    host: Arc<StartedServiceContext>,
    reference: super::super::registry::keeper::KeeperReference,
) -> (KeeperState, Result<bool>) {
    match RecoveredKeeper::reopen(host, reference) {
        Ok(keeper) => (KeeperState::Recovered(keeper), Ok(false)),
        Err(failure) => (
            KeeperState::Recovery(failure.custody),
            Err(CoordinatorFailure::unknown(failure.error)),
        ),
    }
}

fn activate_running(
    host: &Arc<StartedServiceContext>,
    keeper: &mut RunningKeeper,
    ready: &mut Option<KeeperReadyAck>,
    stop: &Event,
) -> NativeResult<()> {
    check_stop(stop)?;
    let mut intent = Store::resume_keeper_custody(Arc::clone(host), keeper)?;
    match intent.phase()? {
        KeeperIntentPhase::Suspended => {
            if ready.is_none() {
                // Store the typed ACK BEFORE any protected Ready publication.
                // A readback-unknown retry must reuse this exact owned ACK.
                *ready = Some(acknowledge_initial_ready(host, &intent, keeper, stop)?);
            }
            let ack = ready.as_ref().ok_or(Error::HandoffOutcomeUnknown)?;
            check_stop(stop)?;
            intent.record_ready(keeper, ack)?;
        }
        KeeperIntentPhase::Ready | KeeperIntentPhase::Active => {}
        KeeperIntentPhase::Reserved => return Err(Error::ScopeConflict),
    }
    match intent.phase()? {
        KeeperIntentPhase::Ready | KeeperIntentPhase::Active => {
            let ack = ready.as_ref().ok_or(Error::HandoffOutcomeUnknown)?;
            check_stop(stop)?;
            // Active retries consume the original ACK and a fresh actual
            // detached-limits receipt too; the DATA phase is never sufficient.
            // detach_staging's Active path only verifies, never relaxes limits.
            let receipt = keeper.detach_staging(&intent, ack)?;
            check_stop(stop)?;
            intent.record_active(receipt, ack)?;
        }
        _ => return Err(Error::ScopeConflict),
    }
    intent.require_active(host, keeper.reservation_nonce(), keeper.selector()?)?;
    drop(intent);
    let mut peer = KeeperPeer::Running(keeper);
    authenticate_connection(host, &mut peer, stop)?;
    peer.recheck()?;
    check_stop(stop)
}

/// Called only with no in-memory writer custody. The protected owner separately
/// requires its full durable writer census empty in the same TxR, so recovered
/// old rows cannot be erased by an empty coordinator Vec. An actually live
/// keeper is left alone. No timeout/disconnect/name absence becomes exit.
pub(super) fn retire_observed_exit(
    host: &Arc<StartedServiceContext>,
    state: &mut KeeperState,
    stop: &Event,
) -> Result<()> {
    check_stop(stop).map_err(CoordinatorFailure::unknown)?;
    let retired = match state {
        KeeperState::Running { keeper, .. } => {
            if keeper.recheck().is_ok() {
                return Ok(());
            }
            match keeper.observe_exit() {
                Ok(exited) => Store::retire_exited_keeper(Arc::clone(host), &exited),
                Err(error) => return Err(CoordinatorFailure::unknown(error)),
            }
        }
        KeeperState::Recovered(keeper) => {
            if keeper.recheck().is_ok() {
                return Ok(());
            }
            match keeper.observe_exit() {
                Ok(exited) => Store::retire_exited_keeper(Arc::clone(host), &exited),
                Err(error) => return Err(CoordinatorFailure::unknown(error)),
            }
        }
        _ => return Ok(()),
    };
    retired.map_err(CoordinatorFailure::unknown)?;
    // Only a completed protected Idle commit/readback authorizes discarding the
    // exact exited keeper. StateCommitUnknown retains the original above.
    *state = KeeperState::Unselected;
    Ok(())
}
